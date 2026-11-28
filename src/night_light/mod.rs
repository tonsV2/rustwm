//! The night light: a warm tint on the screen at night.
//!
//! A timer works out the temperature from the wall clock every 30 seconds,
//! instead of sleeping until the next sunset, so clock changes, daylight
//! saving and suspend need no special cases. Big changes fade in over a
//! second. The temperature reaches the screen through the display's gamma
//! table (see [`crate::gamma`]), which only the DRM backend has; nested,
//! the schedule runs but nothing is tinted.

pub mod schedule;
pub mod sun;
pub mod temperature;

use std::time::{Duration, Instant};

use chrono::Local;
use smithay::reexports::calloop::{
    RegistrationToken,
    timer::{TimeoutAction, Timer},
};
use tracing::{debug, warn};

use self::{
    schedule::Schedule,
    temperature::{NEUTRAL, blend},
};
use crate::{state::State, udev};

/// How often the schedule is looked at.
const TICK: Duration = Duration::from_secs(30);
/// How long a big change takes.
const FADE: Duration = Duration::from_secs(1);
const FADE_STEP: Duration = Duration::from_millis(100);
/// Changes bigger than this fade in rather than jump.
const JUMP_KELVIN: u32 = 250;

/// A fade from `from` towards the current target.
#[derive(Debug, Clone, Copy)]
struct Fade {
    from: u32,
    started: Instant,
}

/// What the night light shows, and the timer that updates it.
#[derive(Debug)]
pub struct NightLight {
    fade: Option<Fade>,
    /// The temperature on screen.
    shown: u32,
    timer: Option<RegistrationToken>,
}

impl Default for NightLight {
    fn default() -> Self {
        Self {
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
    /// temperature now, fading when the step is big. `instant` times the
    /// fade. Returns the new temperature if it changed.
    fn update(&mut self, instant: Instant, scheduled: u32) -> Option<u32> {
        let target = scheduled;
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

    /// The config's night light section changed.
    pub fn night_light_config_changed(&mut self) {
        warn_if_unlocated(self);
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
        let scheduled = if config.enabled {
            blend(
                config.temperature,
                Schedule::from_config(config).level(&now),
            )
        } else {
            NEUTRAL
        };
        if let Some(kelvin) = self.night_light.update(Instant::now(), scheduled) {
            debug!(kelvin, "night light");
        }
        // Every tick rather than only on a change, so a write that failed,
        // such as while the monitor was off, is retried. Unchanged values
        // cost nothing.
        udev::apply_gamma(self);
        self.night_light.next_tick()
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

    #[test]
    fn small_steps_apply_at_once() {
        let mut light = NightLight::default();
        let start = Instant::now();
        assert_eq!(light.update(start, NEUTRAL), None);
        assert_eq!(light.update(start, 6400), Some(6400));
        assert_eq!(light.next_tick(), TICK);
    }

    #[test]
    fn big_steps_fade_over_a_second() {
        let mut light = NightLight::default();
        let start = Instant::now();
        // The fade starts from what is shown.
        assert_eq!(light.update(start, 2700), None);
        assert_eq!(light.next_tick(), FADE_STEP);
        let halfway = light.update(start + FADE / 2, 2700).unwrap();
        assert_eq!(halfway, 4600);
        assert_eq!(light.update(start + FADE, 2700), Some(2700));
        assert_eq!(light.next_tick(), TICK);
        assert_eq!(light.kelvin(), 2700);
    }
}
