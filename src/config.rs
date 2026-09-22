//! Settings from `~/.config/rustwm/config.toml`.
//!
//! Every setting is optional; a missing file or key uses the default. A file
//! that fails to parse is reported and ignored rather than stopping the
//! compositor.

use std::{env, fs, path::PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub output: OutputConfig,
    pub input: InputConfig,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct OutputConfig {
    /// Output scale, such as 1.25. Unset means a guess from the monitor's
    /// pixel density.
    pub scale: Option<f64>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct InputConfig {
    /// libinput pointer acceleration speed, from -1.0 (slowest) to 1.0.
    pub pointer_speed: f64,
    /// Content follows the fingers when scrolling on a touchpad.
    pub natural_scroll: bool,
    pub tap_to_click: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            pointer_speed: 0.0,
            natural_scroll: false,
            tap_to_click: true,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = path() else {
            return Self::default();
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                info!(path = %path.display(), "no config file; using defaults");
                return Self::default();
            }
            Err(err) => {
                warn!(path = %path.display(), %err, "cannot read config; using defaults");
                return Self::default();
            }
        };
        match Self::parse(&text) {
            Ok(config) => {
                info!(path = %path.display(), ?config, "config loaded");
                config
            }
            Err(err) => {
                warn!(path = %path.display(), %err, "invalid config; using defaults");
                Self::default()
            }
        }
    }

    fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

fn path() -> Option<PathBuf> {
    let dir = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(dir.join("rustwm").join("config.toml"))
}

/// Guesses an output scale from its size in pixels and millimetres.
///
/// Targets about 140 pixels per inch after scaling, a comfortable size on
/// laptops and desktop monitors alike, rounded to a quarter so the result is
/// one people recognise from other desktops.
pub fn auto_scale(width_px: i32, width_mm: u32) -> f64 {
    const TARGET_DPI: f64 = 140.0;
    if width_mm == 0 || width_px <= 0 {
        // Projectors and some virtual outputs report no physical size.
        return 1.0;
    }
    let dpi = width_px as f64 / (width_mm as f64 / 25.4);
    let scale = (dpi / TARGET_DPI * 4.0).round() / 4.0;
    scale.clamp(1.0, 3.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn full_file_parses() {
        let config = Config::parse(
            "[output]\nscale = 1.25\n\n[input]\npointer_speed = 0.5\nnatural_scroll = true\ntap_to_click = false\n",
        )
        .unwrap();
        assert_eq!(config.output.scale, Some(1.25));
        assert_eq!(config.input.pointer_speed, 0.5);
        assert!(config.input.natural_scroll);
        assert!(!config.input.tap_to_click);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let config = Config::parse("[input]\nnatural_scroll = true\n").unwrap();
        assert_eq!(config.output.scale, None);
        assert!(config.input.tap_to_click);
    }

    #[test]
    fn typos_are_rejected() {
        assert!(Config::parse("[input]\nnatural_scrolling = true\n").is_err());
    }

    #[test]
    fn auto_scale_guesses() {
        // 14" 2560x1600 laptop panel, about 190 dpi.
        assert_eq!(auto_scale(2560, 344), 1.25);
        // 24" 1920x1200 monitor, about 94 dpi.
        assert_eq!(auto_scale(1920, 518), 1.0);
        // 13" 2880x1800 panel, about 256 dpi.
        assert_eq!(auto_scale(2880, 286), 1.75);
        // No physical size reported.
        assert_eq!(auto_scale(1920, 0), 1.0);
    }
}
