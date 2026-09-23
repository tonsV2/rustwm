//! Settings from `~/.config/rustwm/config.toml`.
//!
//! Every setting is optional; a missing file or key uses the default. A file
//! that fails to parse is reported and ignored rather than stopping the
//! compositor.

use std::{env, fs, path::PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

use crate::keyboard::{App, Command};

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub output: OutputConfig,
    pub input: InputConfig,
    pub keyboard: KeyboardConfig,
    pub shortcuts: ShortcutsConfig,
    pub osd: OsdConfig,
    pub autostart: AutostartConfig,
    pub panel: PanelConfig,
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

/// The XKB keymap and key repeat. Unset keymap fields come from the
/// `XKB_DEFAULT_*` variables, or else from the system keyboard set with
/// `localectl` (see [`crate::keymap`]).
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct KeyboardConfig {
    /// Comma separated layouts, such as "dk" or "us,dk".
    pub layout: Option<String>,
    /// Comma separated variants, one per layout, such as "nodeadkeys".
    pub variant: Option<String>,
    /// Keyboard model, such as "pc105".
    pub model: Option<String>,
    /// Comma separated XKB options, such as "caps:escape".
    pub options: Option<String>,
    /// Milliseconds a key is held before it repeats. Clients are told the
    /// same values, so held keys repeat alike in apps and in the
    /// compositor's own bindings.
    pub repeat_delay: i32,
    /// Repeats per second while a key is held.
    pub repeat_rate: i32,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: None,
            variant: None,
            model: None,
            options: None,
            repeat_delay: 200,
            repeat_rate: 25,
        }
    }
}

/// What each shortcut runs, as a shell command run with `sh -c`. An empty string leaves the key to the focused application.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ShortcutsConfig {
    pub brightness_up: String,
    pub brightness_down: String,
    pub volume_up: String,
    pub volume_down: String,
    pub volume_mute: String,
    pub mic_mute: String,
    pub screenshot: String,
    /// A new terminal window, Super+Return.
    pub terminal: String,
    /// The file manager, Super+f.
    pub file_manager: String,
    /// The web browser, Super+b.
    pub browser: String,
    /// The screen locker, Super+l.
    pub lock: String,
    /// A screenshot of a selected area, Super+Shift+s.
    pub screenshot_area: String,
}

/// Saves the whole screen as a PNG named after the time.
const SCREENSHOT: &str = "mkdir -p ~/Pictures/Screenshots && \
    grim ~/Pictures/Screenshots/Screenshot-$(date +%Y-%m-%d-%H%M%S).png";

const SCREENSHOT_AREA: &str = "area=\"$(slurp)\" && mkdir -p ~/Pictures/Screenshots && \
    grim -g \"$area\" ~/Pictures/Screenshots/Screenshot-$(date +%Y-%m-%d-%H%M%S).png";

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            brightness_up: "brightnessctl set +5%".into(),
            brightness_down: "brightnessctl set 5%-".into(),
            volume_up: "rustwm-volume up 5".into(),
            volume_down: "rustwm-volume down 5".into(),
            volume_mute: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".into(),
            mic_mute: "wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle".into(),
            screenshot: SCREENSHOT.into(),
            terminal: "rustwm-terminal".into(),
            file_manager: "xdg-open \"$HOME\"".into(),
            browser: "gtk-launch \"$(xdg-settings get default-web-browser)\"".into(),
            lock: "swaylock -f -c 000000".into(),
            screenshot_area: SCREENSHOT_AREA.into(),
        }
    }
}

impl ShortcutsConfig {
    pub fn get(&self, command: Command) -> &str {
        match command {
            Command::BrightnessUp => &self.brightness_up,
            Command::BrightnessDown => &self.brightness_down,
            Command::VolumeUp => &self.volume_up,
            Command::VolumeDown => &self.volume_down,
            Command::VolumeMute => &self.volume_mute,
            Command::MicMute => &self.mic_mute,
            Command::Screenshot => &self.screenshot,
        }
    }

    /// The command a Super shortcut starts `app` with.
    pub fn app(&self, app: App) -> &str {
        match app {
            App::Terminal => &self.terminal,
            App::FileManager => &self.file_manager,
            App::Browser => &self.browser,
            App::Lock => &self.lock,
            App::ScreenshotArea => &self.screenshot_area,
        }
    }
}

/// The on-screen indicator shown after a brightness or volume key.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct OsdConfig {
    /// Off for people who run their own indicator, such as swayosd or wob.
    pub enabled: bool,
}

impl Default for OsdConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Programs started once the Wayland socket is ready, such as swayidle or a notification daemon.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AutostartConfig {
    pub commands: Vec<String>,
}

/// The status bar: an outside layer-shell program, Waybar by default.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PanelConfig {
    /// Started once rustwm is ready, with `RUSTWM_PANEL_POSITION` and `RUSTWM_PANEL_MODE` set. Empty means no panel.
    pub command: String,
    pub position: PanelPosition,
    pub mode: PanelMode,
    /// The layer-shell namespace the bar gives its surface, so rustwm knows which surface to auto-hide.
    pub namespace: String,
    /// In auto-hide mode, how long the panel stays after the pointer leaves it, in milliseconds.
    pub hide_delay_ms: u64,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            command: "rustwm-panel".into(),
            position: PanelPosition::Top,
            mode: PanelMode::AutoHide,
            namespace: "waybar".into(),
            hide_delay_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PanelPosition {
    Top,
    Bottom,
}

impl PanelPosition {
    pub fn name(self) -> &'static str {
        match self {
            PanelPosition::Top => "top",
            PanelPosition::Bottom => "bottom",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PanelMode {
    /// Off screen until the pointer touches the panel's screen edge; windows keep the whole screen.
    AutoHide,
    /// Always on screen, with windows laid out beside it.
    Always,
}

impl PanelMode {
    pub fn name(self) -> &'static str {
        match self {
            PanelMode::AutoHide => "auto-hide",
            PanelMode::Always => "always",
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
        let mut config: Self = toml::from_str(text)?;
        // A rate of 0 would divide by zero when timing repeats.
        config.keyboard.repeat_delay = config.keyboard.repeat_delay.max(0);
        config.keyboard.repeat_rate = config.keyboard.repeat_rate.max(1);
        Ok(config)
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
        assert_eq!(config.shortcuts, ShortcutsConfig::default());
    }

    #[test]
    fn commands_parse() {
        let config = Config::parse(
            "[shortcuts]\nvolume_up = \"wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+\"\nmic_mute = \"\"\n",
        )
        .unwrap();
        assert_eq!(
            config.shortcuts.get(Command::VolumeUp),
            "wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+"
        );
        assert_eq!(config.shortcuts.get(Command::MicMute), "");
        assert_eq!(config.shortcuts.get(Command::Screenshot), SCREENSHOT);
        assert_eq!(
            config.shortcuts.get(Command::BrightnessUp),
            "brightnessctl set +5%"
        );
        assert!(Config::parse("[shortcuts]\nvolume_raise = \"true\"\n").is_err());
    }

    #[test]
    fn osd_can_be_turned_off() {
        assert!(Config::default().osd.enabled);
        let config = Config::parse("[osd]\nenabled = false\n").unwrap();
        assert!(!config.osd.enabled);
    }

    #[test]
    fn keyboard_parses() {
        let config = Config::parse(
            "[keyboard]\nlayout = \"us,dk\"\noptions = \"grp:alt_shift_toggle\"\nrepeat_rate = 0\n",
        )
        .unwrap();
        assert_eq!(config.keyboard.layout.as_deref(), Some("us,dk"));
        assert_eq!(config.keyboard.variant, None);
        assert_eq!(
            config.keyboard.options.as_deref(),
            Some("grp:alt_shift_toggle")
        );
        assert_eq!(config.keyboard.repeat_delay, 200);
        assert_eq!(config.keyboard.repeat_rate, 1);
    }

    #[test]
    fn sample_config_parses() {
        let sample = include_str!("../resources/config.toml");
        let config = Config::parse(sample).unwrap();
        assert_eq!(config.shortcuts.app(App::Lock), "swaylock -f -c 000000");
        assert_eq!(
            config.shortcuts.terminal,
            ShortcutsConfig::default().terminal
        );
    }

    #[test]
    fn autostart_parses() {
        let config =
            Config::parse("[autostart]\ncommands = [\"mako\", \"swayidle -w\"]\n").unwrap();
        assert_eq!(config.autostart.commands, ["mako", "swayidle -w"]);
        assert!(Config::default().autostart.commands.is_empty());
    }

    #[test]
    fn panel_parses() {
        let config = Config::parse(
            "[panel]\nposition = \"bottom\"\nmode = \"always\"\nhide_delay_ms = 2500\n",
        )
        .unwrap();
        assert_eq!(config.panel.hide_delay_ms, 2500);
        assert_eq!(config.panel.position, PanelPosition::Bottom);
        assert_eq!(config.panel.mode, PanelMode::Always);
        assert_eq!(config.panel.command, "rustwm-panel");
        let defaults = Config::default().panel;
        assert_eq!(
            (defaults.position, defaults.mode),
            (PanelPosition::Top, PanelMode::AutoHide)
        );
        assert!(Config::parse("[panel]\nmode = \"sometimes\"\n").is_err());
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
