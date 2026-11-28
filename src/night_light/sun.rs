//! Where the sun is, for the sunset schedule.
//!
//! Follows NOAA's "General Solar Position Calculations"
//! (gml.noaa.gov/grad/solcalc/solareqns.PDF), which is accurate to a fraction
//! of a degree: far better than a screen tint needs. Everything is in UTC
//! and the longitude, so time zones and daylight saving play no part.

use std::f64::consts::PI;

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};

/// The sun's height above the horizon at `at`, in degrees, as seen from
/// `latitude` (north positive) and `longitude` (east positive). Negative
/// below the horizon.
pub fn elevation(at: DateTime<Utc>, latitude: f64, longitude: f64) -> f64 {
    let minutes = at.hour() as f64 * 60.0 + at.minute() as f64 + at.second() as f64 / 60.0;
    let days_in_year = if NaiveDate::from_ymd_opt(at.year(), 2, 29).is_some() {
        366.0
    } else {
        365.0
    };
    // The fractional year, in radians.
    let gamma = 2.0 * PI / days_in_year * (at.ordinal0() as f64 + (minutes / 60.0 - 12.0) / 24.0);

    // How far solar time runs ahead of clock time, in minutes.
    let eqtime = 229.18
        * (0.000075 + 0.001868 * gamma.cos()
            - 0.032077 * gamma.sin()
            - 0.014615 * (2.0 * gamma).cos()
            - 0.040849 * (2.0 * gamma).sin());
    // The sun's declination, in radians.
    let declination = 0.006918 - 0.399912 * gamma.cos() + 0.070257 * gamma.sin()
        - 0.006758 * (2.0 * gamma).cos()
        + 0.000907 * (2.0 * gamma).sin()
        - 0.002697 * (3.0 * gamma).cos()
        + 0.00148 * (3.0 * gamma).sin();

    let true_solar_minutes = minutes + eqtime + 4.0 * longitude;
    let hour_angle = (true_solar_minutes / 4.0 - 180.0).to_radians();
    let latitude = latitude.to_radians();
    let cos_zenith =
        latitude.sin() * declination.sin() + latitude.cos() * declination.cos() * hour_angle.cos();
    90.0 - cos_zenith.clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} is not within {tolerance} of {expected}"
        );
    }

    #[test]
    fn overhead_at_the_equator_on_the_equinox() {
        assert_close(elevation(utc(2024, 3, 20, 12, 7), 0.0, 0.0), 90.0, 1.0);
    }

    #[test]
    fn on_the_horizon_at_sunset_in_copenhagen() {
        // Sunset on midsummer is 21:57 local time, 19:57 UTC.
        assert_close(elevation(utc(2024, 6, 21, 19, 57), 55.68, 12.57), -0.8, 0.5);
    }

    #[test]
    fn near_the_pole_the_height_is_the_declination() {
        assert_close(elevation(utc(2024, 6, 21, 0, 0), 89.9, 0.0), 23.4, 0.5);
        assert_close(elevation(utc(2024, 12, 21, 12, 0), 89.9, 0.0), -23.4, 0.5);
    }
}
