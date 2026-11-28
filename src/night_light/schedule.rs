//! When it is night: how warm the screen should be at a given moment.
//!
//! Both schedules are worked out from the time alone, with nothing
//! remembered between calls, so clock changes, daylight saving and suspend
//! need no special handling.

use chrono::{DateTime, TimeDelta, TimeZone, Timelike, Utc};
use serde::Deserialize;

use super::sun;
use crate::config::{NightLightConfig, NightLightSchedule};

const MINUTES_PER_DAY: f64 = 1440.0;
/// The sun's height at and above which it is day, in degrees.
const DAY_ELEVATION: f64 = 3.0;
/// The sun's height at and below which it is full night, in degrees.
const NIGHT_ELEVATION: f64 = -6.0;
/// How far ahead [`Schedule::next_morning`] looks, in minutes.
const MORNING_HORIZON: i64 = 2 * 1440;

/// A time of day in the config, written "HH:MM".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct TimeOfDay {
    minutes: u16,
}

impl TimeOfDay {
    pub fn new(hour: u16, minute: u16) -> Self {
        Self {
            minutes: hour * 60 + minute,
        }
    }

    /// Minutes since midnight.
    pub fn minutes(self) -> u16 {
        self.minutes
    }
}

impl TryFrom<String> for TimeOfDay {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let error = || format!("night_light times are HH:MM, such as 20:00, not {text:?}");
        let (hour, minute) = text.split_once(':').ok_or_else(error)?;
        let digits = |s: &str, max_len: usize| {
            !s.is_empty() && s.len() <= max_len && s.bytes().all(|b| b.is_ascii_digit())
        };
        if !digits(hour, 2) || !digits(minute, 2) || minute.len() != 2 {
            return Err(error());
        }
        let hour: u16 = hour.parse().map_err(|_| error())?;
        let minute: u16 = minute.parse().map_err(|_| error())?;
        if hour > 23 || minute > 59 {
            return Err(error());
        }
        Ok(Self::new(hour, minute))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Schedule {
    /// Night from `from` to `to` in local time, warming up over
    /// `transition_minutes` before `from` and cooling down over the same
    /// before `to`, as GNOME does.
    Fixed {
        from: TimeOfDay,
        to: TimeOfDay,
        transition_minutes: u32,
    },
    /// Night follows the sun at the given place.
    Sun { latitude: f64, longitude: f64 },
}

impl Schedule {
    /// The schedule the config asks for. A sunset schedule without a
    /// location falls back to the fixed times; see [`unlocated`].
    pub fn from_config(config: &NightLightConfig) -> Self {
        match (config.schedule, config.latitude, config.longitude) {
            (NightLightSchedule::Sunset, Some(latitude), Some(longitude)) => Schedule::Sun {
                latitude,
                longitude,
            },
            _ => Schedule::Fixed {
                from: config.from,
                to: config.to,
                transition_minutes: config.transition_minutes,
            },
        }
    }

    /// How far into night it is at `at`: 0.0 by day, 1.0 at full night.
    pub fn level<Tz: TimeZone>(&self, at: &DateTime<Tz>) -> f64 {
        match *self {
            Schedule::Fixed {
                from,
                to,
                transition_minutes,
            } => {
                let time = at.time();
                let minute =
                    time.hour() as f64 * 60.0 + time.minute() as f64 + time.second() as f64 / 60.0;
                fixed_level(minute, from.minutes(), to.minutes(), transition_minutes)
            }
            Schedule::Sun {
                latitude,
                longitude,
            } => sun_level(sun::elevation(at.with_timezone(&Utc), latitude, longitude)),
        }
    }

    /// The end of the night `now` is in, or of the next night when it is
    /// day: the first minute after some night that is fully day again.
    /// `None` when that does not happen within two days, as in polar day
    /// and night.
    pub fn next_morning<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        let mut seen_night = self.level(now) > 0.0;
        for minutes in 1..=MORNING_HORIZON {
            // Adding to the zoned time keeps daylight saving changes right.
            let at = now.clone() + TimeDelta::minutes(minutes);
            let night = self.level(&at) > 0.0;
            if seen_night && !night {
                return Some(at);
            }
            seen_night |= night;
        }
        None
    }
}

/// True when the config asks for the sunset schedule without saying where,
/// so the fixed times are used instead.
pub fn unlocated(config: &NightLightConfig) -> bool {
    config.schedule == NightLightSchedule::Sunset
        && (config.latitude.is_none() || config.longitude.is_none())
}

/// The night level at `minute` of the day for a night from `from` to `to`
/// (minutes since midnight), ramping up over the `transition` minutes before
/// `from` and down over the `transition` minutes before `to`. The night may
/// cross midnight; `from == to` means never night.
fn fixed_level(minute: f64, from: u16, to: u16, transition: u32) -> f64 {
    let from = from as f64;
    let to = to as f64;
    let night_length = (to - from).rem_euclid(MINUTES_PER_DAY);
    if night_length == 0.0 {
        return 0.0;
    }
    // The ramps fit inside the night and the day they run in.
    let smear = (transition as f64)
        .min(night_length)
        .min(MINUTES_PER_DAY - night_length);

    let since_from = (minute - from).rem_euclid(MINUTES_PER_DAY);
    if since_from < night_length - smear {
        return 1.0;
    }
    if since_from < night_length {
        // Cooling down towards `to`; only reached with a smear.
        return 1.0 - (since_from - (night_length - smear)) / smear;
    }
    let until_from = (from - minute).rem_euclid(MINUTES_PER_DAY);
    if smear > 0.0 && until_from <= smear {
        // Warming up towards `from`.
        return 1.0 - until_from / smear;
    }
    0.0
}

/// The night level for the sun at `elevation` degrees: day from 3° above
/// the horizon, full night from 6° below, as redshift and gammastep do.
fn sun_level(elevation: f64) -> f64 {
    ((DAY_ELEVATION - elevation) / (DAY_ELEVATION - NIGHT_ELEVATION)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn at(h: u16, m: u16) -> f64 {
        (h * 60 + m) as f64
    }

    fn evening(minute: f64) -> f64 {
        fixed_level(minute, 20 * 60, 6 * 60, 60)
    }

    #[test]
    fn fixed_night_crossing_midnight() {
        assert_eq!(evening(at(12, 0)), 0.0);
        assert_eq!(evening(at(19, 0)), 0.0);
        assert_eq!(evening(at(19, 30)), 0.5);
        assert_eq!(evening(at(20, 0)), 1.0);
        assert_eq!(evening(at(3, 0)), 1.0);
        assert_eq!(evening(at(5, 0)), 1.0);
        assert_eq!(evening(at(5, 30)), 0.5);
        assert_eq!(evening(at(6, 0)), 0.0);
    }

    #[test]
    fn fixed_night_before_midnight_ends() {
        let level = |minute| fixed_level(minute, 0, 7 * 60, 60);
        assert_eq!(level(at(23, 30)), 0.5);
        assert_eq!(level(at(0, 0)), 1.0);
        assert_eq!(level(at(6, 30)), 0.5);
        assert_eq!(level(at(7, 0)), 0.0);
        assert_eq!(level(at(12, 0)), 0.0);
    }

    #[test]
    fn short_night_clamps_the_transition() {
        let level = |minute| fixed_level(minute, 20 * 60, 20 * 60 + 30, 60);
        assert_eq!(level(at(19, 0)), 0.0);
        assert_eq!(level(at(19, 45)), 0.5);
        assert_eq!(level(at(20, 0)), 1.0);
        assert_eq!(level(at(20, 15)), 0.5);
        assert_eq!(level(at(20, 30)), 0.0);
    }

    #[test]
    fn equal_times_are_never_night() {
        for hour in 0..24 {
            assert_eq!(fixed_level(at(hour, 0), 20 * 60, 20 * 60, 60), 0.0);
        }
    }

    #[test]
    fn no_transition_is_a_step() {
        let level = |minute| fixed_level(minute, 20 * 60, 6 * 60, 0);
        assert_eq!(level(at(19, 59)), 0.0);
        assert_eq!(level(at(20, 0)), 1.0);
        assert_eq!(level(at(5, 59)), 1.0);
        assert_eq!(level(at(6, 0)), 0.0);
    }

    #[test]
    fn sun_level_follows_the_elevation() {
        assert_eq!(sun_level(10.0), 0.0);
        assert_eq!(sun_level(3.0), 0.0);
        assert!((sun_level(-1.5) - 0.5).abs() < 1e-9);
        assert_eq!(sun_level(-6.0), 1.0);
        assert_eq!(sun_level(-30.0), 1.0);
    }

    const LONGYEARBYEN: Schedule = Schedule::Sun {
        latitude: 78.2,
        longitude: 15.6,
    };

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn polar_night_and_day() {
        for hour in 0..24 {
            assert_eq!(LONGYEARBYEN.level(&utc(2024, 12, 21, hour, 0)), 1.0);
        }
        assert_eq!(LONGYEARBYEN.level(&utc(2024, 6, 21, 0, 0)), 0.0);
        assert_eq!(LONGYEARBYEN.next_morning(&utc(2024, 12, 21, 12, 0)), None);
        assert_eq!(LONGYEARBYEN.next_morning(&utc(2024, 6, 21, 12, 0)), None);
    }

    fn fixed() -> Schedule {
        Schedule::Fixed {
            from: TimeOfDay::new(20, 0),
            to: TimeOfDay::new(6, 0),
            transition_minutes: 60,
        }
    }

    fn local(d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(3600)
            .unwrap()
            .with_ymd_and_hms(2024, 3, d, h, mi, 0)
            .unwrap()
    }

    #[test]
    fn fixed_level_uses_local_time() {
        assert_eq!(fixed().level(&local(10, 3, 0)), 1.0);
        // A zone other than local time works too: 12:00 UTC is day.
        assert_eq!(fixed().level(&utc(2024, 3, 10, 12, 0)), 0.0);
    }

    #[test]
    fn next_morning_in_a_fixed_schedule() {
        let schedule = fixed();
        assert_eq!(
            schedule.next_morning(&local(10, 22, 0)),
            Some(local(11, 6, 0))
        );
        assert_eq!(
            schedule.next_morning(&local(10, 12, 0)),
            Some(local(11, 6, 0))
        );
        assert_eq!(
            schedule.next_morning(&local(10, 5, 30)),
            Some(local(10, 6, 0))
        );
    }

    fn parse(text: &str) -> Result<TimeOfDay, String> {
        TimeOfDay::try_from(text.to_string())
    }

    #[test]
    fn times_of_day_parse() {
        assert_eq!(parse("20:00"), Ok(TimeOfDay::new(20, 0)));
        assert_eq!(parse("6:30"), Ok(TimeOfDay::new(6, 30)));
        assert_eq!(parse("00:00").map(TimeOfDay::minutes), Ok(0));
        assert_eq!(parse("23:59").map(TimeOfDay::minutes), Ok(1439));
        for bad in [
            "24:00", "12:60", "8", "ab", "", "12:5", "-1:00", "123:00", "12:00:00",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
