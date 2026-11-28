//! The night light: a warm tint on the screen at night.
//!
//! A timer works out the temperature from the wall clock every 30 seconds,
//! instead of sleeping until the next sunset, so clock changes, daylight
//! saving and suspend need no special cases. Big changes fade in over a
//! second. The temperature reaches the screen through the display's gamma
//! table (see [`crate::gamma`]), which only the DRM backend has; nested,
//! the schedule runs but nothing is tinted.
//!
//! Super+n overrides the schedule until morning, as GNOME's "Disable until
//! tomorrow" does: off at night, or on by day.

pub mod schedule;
pub mod sun;
pub mod temperature;

use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use smithay::reexports::calloop::{
    RegistrationToken,
    timer::{TimeoutAction, Timer},
};
use tracing::{debug, info, warn};

use self::{
    schedule::Schedule,
    temperature::{NEUTRAL, blend},
};
use crate::{config::NightLightConfig, state::State, udev};

/// How often the schedule is looked at.
const TICK: Duration = Duration::from_secs(30);
/// How long a big change takes.
const FADE: Duration = Duration::from_secs(1);
const FADE_STEP: Duration = Duration::from_millis(100);
/// Changes bigger than this fade in rather than jump.
const JUMP_KELVIN: u32 = 250;

/// The user's choice, which wins over the schedule until `until`: the end
/// of the night it was made in, or of the next one when made by day. `None`
/// when no morning was found, as in polar day and night; it then lasts until
/// toggled again. In UTC, so it still ends after a long suspend or a clock
/// change.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Manual {
    on: bool,
    until: Option<DateTime<Utc>>,
}

/// A fade from `from` towards the current target.
#[derive(Debug, Clone, Copy)]
struct Fade {
    from: u32,
    started: Instant,
}

/// What the night light shows, and the timer that updates it.
#[derive(Debug)]
pub struct NightLight {
    manual: Option<Manual>,
    fade: Option<Fade>,
    /// The temperature on screen.
    shown: u32,
    timer: Option<RegistrationToken>,
}

impl Default for NightLight {
    fn default() -> Self {
        Self {
            manual: None,
            fade: None,
            shown: NEUTRAL,
            timer: None,
        }
    }
}

impl NightLight {
    /// The temperature the screen should show now.
    pub fn kelvin(&self) -> u32 {
        self.shown
    }

    /// Moves the shown temperature towards `scheduled`, the schedule's
    /// temperature at `now`, or towards `night` or neutral while overridden,
    /// fading when the step is big. `instant` times the fade. Returns the
    /// new temperature if it changed.
    fn update(
        &mut self,
        now: DateTime<Utc>,
        instant: Instant,
        scheduled: u32,
        night: u32,
    ) -> Option<u32> {
        self.expire(now);
        let target = match self.manual {
            Some(Manual { on: true, .. }) => night,
            Some(Manual { on: false, .. }) => NEUTRAL,
            None => scheduled,
        };
        if self.fade.is_none() && target.abs_diff(self.shown) > JUMP_KELVIN {
            self.fade = Some(Fade {
                from: self.shown,
                started: instant,
            });
        }
        let next = match self.fade {
            Some(fade) => {
                let progress = instant
                    .saturating_duration_since(fade.started)
                    .as_secs_f64()
                    / FADE.as_secs_f64();
                if progress >= 1.0 {
                    self.fade = None;
                    target
                } else {
                    // The target may move during the fade, as when the
                    // schedule ramps; the fade simply follows it.
                    lerp(fade.from, target, progress)
                }
            }
            None => target,
        };
        if next == self.shown {
            return None;
        }
        self.shown = next;
        Some(next)
    }

    /// Drops an override whose morning has come.
    fn expire(&mut self, now: DateTime<Utc>) {
        if self
            .manual
            .is_some_and(|m| m.until.is_some_and(|until| until <= now))
        {
            self.manual = None;
        }
    }

    /// Turns the night light the other way from what it shows now, until
    /// `morning`. `scheduled_on` is whether the schedule has it on now.
    /// Going back to what the schedule says ends the override instead.
    fn toggle(&mut self, scheduled_on: bool, morning: Option<DateTime<Utc>>) {
        let on_now = self.manual.map(|m| m.on).unwrap_or(scheduled_on);
        let want = !on_now;
        self.manual = if want == scheduled_on {
            None
        } else {
            Some(Manual {
                on: want,
                until: morning,
            })
        };
    }

    /// Goes back to the schedule.
    fn clear_manual(&mut self) {
        self.manual = None;
    }

    /// How long until the next update: short steps while fading.
    fn next_tick(&self) -> Duration {
        if self.fade.is_some() { FADE_STEP } else { TICK }
    }
}

fn lerp(from: u32, to: u32, progress: f64) -> u32 {
    (from as f64 + (to as f64 - from as f64) * progress).round() as u32
}

impl State {
    /// Starts the timer that keeps the night light up to date. The first
    /// update runs as soon as the event loop does.
    pub fn start_night_light(&mut self) {
        warn_if_unlocated(self);
        self.schedule_night_light(Duration::ZERO);
    }

    /// The config's night light section changed. An override made under
    /// the old schedule no longer means anything.
    pub fn night_light_config_changed(&mut self) {
        warn_if_unlocated(self);
        self.night_light.clear_manual();
        self.wake_night_light();
    }

    /// Super+n: off until morning at night, on until morning by day.
    pub fn toggle_night_light(&mut self) {
        let now = Local::now();
        let (scheduled_on, morning) = scheduled_now(&self.config.night_light, &now);
        self.night_light.expire(now.with_timezone(&Utc));
        self.night_light.toggle(scheduled_on, morning);
        match self.night_light.manual {
            Some(manual) => info!(
                "night light {}{}",
                if manual.on { "on" } else { "off" },
                until_text(manual.until)
            ),
            None => info!("night light follows the schedule again"),
        }
        self.wake_night_light();
    }

    /// Updates the night light now rather than at the next tick, so a
    /// change starts fading at once.
    fn wake_night_light(&mut self) {
        // Never called from the night light's own timer callback, so the
        // source can be removed here.
        if let Some(token) = self.night_light.timer.take() {
            self.loop_handle.remove(token);
        }
        self.schedule_night_light(Duration::ZERO);
    }

    fn schedule_night_light(&mut self, delay: Duration) {
        let timer = self
            .loop_handle
            .insert_source(Timer::from_duration(delay), |_, _, state| {
                TimeoutAction::ToDuration(state.tick_night_light())
            });
        match timer {
            Ok(token) => self.night_light.timer = Some(token),
            Err(err) => warn!(%err, "failed to start the night light"),
        }
    }

    /// Works out the temperature from the clock and applies it. Returns
    /// when to run again.
    fn tick_night_light(&mut self) -> Duration {
        let now = Local::now();
        let config = &self.config.night_light;
        let night = config.temperature;
        let scheduled = if config.enabled {
            blend(
                config.temperature,
                Schedule::from_config(config).level(&now),
            )
        } else {
            NEUTRAL
        };
        if let Some(kelvin) =
            self.night_light
                .update(now.with_timezone(&Utc), Instant::now(), scheduled, night)
        {
            debug!(kelvin, "night light");
        }
        // Every tick rather than only on a change, so a write that failed,
        // such as while the monitor was off, is retried. Unchanged values
        // cost nothing.
        udev::apply_gamma(self);
        self.night_light.next_tick()
    }
}

/// Whether the schedule has the night light on at `now`, and when the
/// coming morning is. The morning is worked out even with the schedule
/// off, since Super+n still lasts until then.
fn scheduled_now(
    config: &NightLightConfig,
    now: &DateTime<Local>,
) -> (bool, Option<DateTime<Utc>>) {
    let schedule = Schedule::from_config(config);
    let on = config.enabled && schedule.level(now) > 0.0;
    let morning = schedule
        .next_morning(now)
        .map(|morning| morning.with_timezone(&Utc));
    (on, morning)
}

/// " until 06:00" in local time, or nothing without a morning.
fn until_text(until: Option<DateTime<Utc>>) -> String {
    match until {
        Some(until) => format!(" until {}", until.with_timezone(&Local).format("%H:%M")),
        None => String::new(),
    }
}

fn warn_if_unlocated(state: &State) {
    if schedule::unlocated(&state.config.night_light) {
        warn!(
            "night_light.schedule is \"sunset\" but latitude and longitude are not set; \
             using the from and to times"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NIGHT: u32 = 2700;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn hours(h: i64) -> chrono::TimeDelta {
        chrono::TimeDelta::hours(h)
    }

    /// The temperature after any fade has finished.
    fn settled(light: &mut NightLight, at: DateTime<Utc>, scheduled: u32) -> u32 {
        let start = Instant::now();
        light.update(at, start, scheduled, NIGHT);
        light.update(at, start + FADE, scheduled, NIGHT);
        light.kelvin()
    }

    #[test]
    fn toggle_at_night_pauses_until_morning() {
        let mut light = NightLight::default();
        let morning = Some(now() + hours(6));
        assert_eq!(settled(&mut light, now(), NIGHT), NIGHT);
        light.toggle(true, morning);
        assert_eq!(
            light.manual,
            Some(Manual {
                on: false,
                until: morning
            })
        );
        assert_eq!(settled(&mut light, now(), NIGHT), NEUTRAL);
        // Again: back to the schedule.
        light.toggle(true, morning);
        assert_eq!(light.manual, None);
        assert_eq!(settled(&mut light, now(), NIGHT), NIGHT);
    }

    #[test]
    fn toggle_by_day_turns_it_on() {
        let mut light = NightLight::default();
        let morning = Some(now() + hours(18));
        light.toggle(false, morning);
        assert_eq!(
            light.manual,
            Some(Manual {
                on: true,
                until: morning
            })
        );
        assert_eq!(settled(&mut light, now(), NEUTRAL), NIGHT);
        light.toggle(false, morning);
        assert_eq!(light.manual, None);
    }

    #[test]
    fn toggle_after_a_day_override_ran_into_the_night() {
        let mut light = NightLight::default();
        let morning = Some(now() + hours(18));
        light.toggle(false, morning);
        // Night has come and the schedule is on too; the toggle still turns
        // the light off rather than doing nothing.
        light.toggle(true, morning);
        assert_eq!(
            light.manual,
            Some(Manual {
                on: false,
                until: morning
            })
        );
    }

    #[test]
    fn override_ends_in_the_morning() {
        let mut light = NightLight::default();
        let morning = now() + hours(6);
        light.toggle(true, Some(morning));
        assert_eq!(settled(&mut light, morning - hours(1), NIGHT), NEUTRAL);
        assert_eq!(settled(&mut light, morning, NEUTRAL), NEUTRAL);
        assert_eq!(light.manual, None);
        // Also after a jump far past it, as after a long suspend.
        light.toggle(true, Some(morning));
        settled(&mut light, morning + hours(24 * 30), NIGHT);
        assert_eq!(light.manual, None);
        assert_eq!(light.kelvin(), NIGHT);
    }

    #[test]
    fn override_without_a_morning_lasts() {
        let mut light = NightLight::default();
        light.toggle(true, None);
        settled(&mut light, now() + hours(24 * 30), NIGHT);
        assert_eq!(light.kelvin(), NEUTRAL);
        assert!(light.manual.is_some());
    }

    #[test]
    fn small_steps_apply_at_once() {
        let mut light = NightLight::default();
        let start = Instant::now();
        assert_eq!(light.update(now(), start, NEUTRAL, NIGHT), None);
        assert_eq!(light.update(now(), start, 6400, NIGHT), Some(6400));
        assert_eq!(light.next_tick(), TICK);
    }

    #[test]
    fn big_steps_fade_over_a_second() {
        let mut light = NightLight::default();
        let start = Instant::now();
        // The fade starts from what is shown.
        assert_eq!(light.update(now(), start, 2700, NIGHT), None);
        assert_eq!(light.next_tick(), FADE_STEP);
        let halfway = light.update(now(), start + FADE / 2, 2700, NIGHT).unwrap();
        assert_eq!(halfway, 4600);
        assert_eq!(light.update(now(), start + FADE, 2700, NIGHT), Some(2700));
        assert_eq!(light.next_tick(), TICK);
        assert_eq!(light.kelvin(), 2700);
    }
}
