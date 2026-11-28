//! Colour temperature in Kelvin to gamma ramps.
//!
//! The colours come from Tanner Helland's fit to the blackbody curve ("How
//! to Convert Temperature (K) to RGB", tannerhelland.com, 2012), normalised
//! so the neutral temperature leaves every channel untouched. Only the branch
//! below 6600 K is needed, since the setting never goes above neutral.

/// The temperature that leaves the screen as it is.
pub const NEUTRAL: u32 = 6500;
/// The warmest temperature the setting allows.
pub const MIN: u32 = 1000;

/// How much of each channel's full output remains, from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

/// The channel scale factors for `kelvin`, clamped to `MIN..=NEUTRAL`.
pub fn multipliers(kelvin: u32) -> Rgb {
    let kelvin = kelvin.clamp(MIN, NEUTRAL);
    let (green, blue) = helland(kelvin);
    let (neutral_green, neutral_blue) = helland(NEUTRAL);
    Rgb {
        // Red stays at full output below 6600 K in Helland's fit.
        red: 1.0,
        green: green / neutral_green,
        blue: blue / neutral_blue,
    }
}

/// Helland's green and blue, from 0 to 255, for a temperature below 6600 K.
fn helland(kelvin: u32) -> (f64, f64) {
    let t = kelvin as f64 / 100.0;
    let green = 99.470_802_586_1 * t.ln() - 161.119_568_166_1;
    let blue = if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
    };
    (green.clamp(0.0, 255.0), blue.clamp(0.0, 255.0))
}

/// The temperature `level` of the way from neutral to `night`: neutral at
/// 0.0, `night` at 1.0. Blending in Kelvin rather than in colour keeps the
/// evening transition along the blackbody curve.
pub fn blend(night: u32, level: f64) -> u32 {
    let level = level.clamp(0.0, 1.0);
    let neutral = NEUTRAL as f64;
    (neutral + (night as f64 - neutral) * level).round() as u32
}

/// One gamma table per channel, as the display controller takes it: entry
/// `i` is the output for input `i / (size - 1)`, from 0 to 65535.
#[derive(Debug, Clone, PartialEq)]
pub struct GammaRamp {
    pub red: Vec<u16>,
    pub green: Vec<u16>,
    pub blue: Vec<u16>,
}

impl GammaRamp {
    /// A linear ramp of `size` entries, scaled per channel for `kelvin`.
    /// `size` must be at least 2; neutral gives the identity.
    pub fn new(size: usize, kelvin: u32) -> Self {
        let rgb = multipliers(kelvin);
        Self {
            red: channel(size, rgb.red),
            green: channel(size, rgb.green),
            blue: channel(size, rgb.blue),
        }
    }
}

fn channel(size: usize, multiplier: f64) -> Vec<u16> {
    let last = (size.max(2) - 1) as f64;
    (0..size)
        .map(|i| (i as f64 / last * 65535.0 * multiplier).round() as u16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn neutral_is_exactly_white() {
        assert_eq!(
            multipliers(NEUTRAL),
            Rgb {
                red: 1.0,
                green: 1.0,
                blue: 1.0
            }
        );
    }

    #[test]
    fn warm_temperatures_match_the_fit() {
        let rgb = multipliers(2700);
        assert_eq!(rgb.red, 1.0);
        assert!(close(rgb.green, 0.656, 0.02), "{rgb:?}");
        assert!(close(rgb.blue, 0.350, 0.02), "{rgb:?}");
        let rgb = multipliers(MIN);
        assert_eq!(rgb.blue, 0.0);
        assert!(close(rgb.green, 0.27, 0.02), "{rgb:?}");
    }

    #[test]
    fn warmer_means_less_green_and_blue() {
        let mut previous = multipliers(NEUTRAL);
        for kelvin in (MIN..NEUTRAL).rev().step_by(100) {
            let rgb = multipliers(kelvin);
            assert_eq!(rgb.red, 1.0);
            assert!(rgb.green <= previous.green, "{kelvin}");
            assert!(rgb.blue <= previous.blue, "{kelvin}");
            previous = rgb;
        }
    }

    #[test]
    fn out_of_range_is_clamped() {
        assert_eq!(multipliers(9000), multipliers(NEUTRAL));
        assert_eq!(multipliers(200), multipliers(MIN));
    }

    #[test]
    fn blend_goes_from_neutral_to_night() {
        assert_eq!(blend(2700, 0.0), NEUTRAL);
        assert_eq!(blend(2700, 1.0), 2700);
        assert_eq!(blend(2700, 0.5), 4600);
        assert_eq!(blend(2700, 2.0), 2700);
    }

    #[test]
    fn neutral_ramp_is_the_identity() {
        let ramp = GammaRamp::new(256, NEUTRAL);
        assert_eq!(ramp.red.len(), 256);
        assert_eq!(ramp.red[0], 0);
        assert_eq!(ramp.red[255], 65535);
        assert_eq!(ramp.red[1], 257);
        assert_eq!(ramp.red, ramp.green);
        assert_eq!(ramp.red, ramp.blue);
    }

    #[test]
    fn warm_ramp_scales_each_channel() {
        let ramp = GammaRamp::new(256, 2700);
        let rgb = multipliers(2700);
        assert_eq!(ramp.red[255], 65535);
        assert_eq!(ramp.green[255], (65535.0 * rgb.green).round() as u16);
        assert_eq!(ramp.blue[255], (65535.0 * rgb.blue).round() as u16);
    }

    #[test]
    fn two_entries_are_enough() {
        let ramp = GammaRamp::new(2, NEUTRAL);
        assert_eq!(ramp.red, [0, 65535]);
    }
}
