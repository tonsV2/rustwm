//! Settings from `~/.config/rustwm/config.toml`.
//!
//! Every setting is optional; a missing file or key uses the default. A file
//! that fails to parse is reported and ignored rather than stopping the
//! compositor. Saving the file applies it while rustwm runs (see
//! [`crate::reload`]).

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
    /// Still accepted under its old name, `[shortcuts]`.
    #[serde(alias = "shortcuts")]
    pub commands: CommandsConfig,
    pub osd: OsdConfig,
    pub autostart: AutostartConfig,
    pub panel: PanelConfig,
    pub notifications: NotificationsConfig,
    pub overview: OverviewConfig,
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
    /// How a touchpad press picks the button: by the number of fingers,
    /// as GNOME does, or by where on the pad it lands.
    pub click_method: ClickMethod,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClickMethod {
    /// One finger is a left click, two a right click, three a middle click.
    Fingers,
    /// The bottom-right corner is a right click, the bottom middle a middle
    /// click, anywhere else a left click.
    Areas,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            pointer_speed: 0.0,
            natural_scroll: false,
            tap_to_click: true,
            click_method: ClickMethod::Fingers,
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
            repeat_delay: 500,
            repeat_rate: 25,
        }
    }
}

/// The command each key runs, with `sh -c`. An empty string leaves the key to the focused application.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CommandsConfig {
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

impl Default for CommandsConfig {
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

impl CommandsConfig {
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

/// The notification daemon: an outside layer-shell program, mako by default.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationsConfig {
    /// Started once rustwm is ready, before the panel and autostart, so it owns `org.freedesktop.Notifications` before apps send any. Empty means no daemon.
    pub command: String,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            command: "rustwm-notifications".into(),
        }
    }
}

/// The Super tap overview.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct OverviewConfig {
    /// The command the overview's search runs for files, with `sh -c` and
    /// the typed text as `$1`. It prints one path per line. Empty means
    /// applications only.
    pub file_search: String,
}

impl Default for OverviewConfig {
    fn default() -> Self {
        Self {
            file_search: FILE_SEARCH.into(),
        }
    }
}

/// Files under the home directory whose path contains the text, leaving
/// out hidden directories, from the plocate database.
const FILE_SEARCH: &str = "plocate -i -- \"$HOME/\" \"$1\" | grep -v '/\\.' | head -20";

/// Programs started once the Wayland socket is ready, such as swayidle.
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
    /// In auto-hide mode, how long the panel takes to slide in, in milliseconds. 0 shows it at once.
    pub slide_in_ms: u64,
    /// In auto-hide mode, how long the panel takes to slide out, in milliseconds. 0 hides it at once.
    pub slide_out_ms: u64,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            command: "rustwm-panel".into(),
            position: PanelPosition::Top,
            mode: PanelMode::AutoHide,
            namespace: "waybar".into(),
            hide_delay_ms: 1000,
            slide_in_ms: 200,
            slide_out_ms: 200,
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
        Self::read().unwrap_or_default()
    }

    /// The file's settings, or `None` when it cannot be read or parsed, so a half-finished edit keeps the settings in use.
    pub fn read() -> Option<Self> {
        let Some(path) = path() else {
            return Some(Self::default());
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                info!(path = %path.display(), "no config file; using defaults");
                return Some(Self::default());
            }
            Err(err) => {
                warn!(path = %path.display(), %err, "cannot read config");
                return None;
            }
        };
        match Self::parse(&text) {
            Ok(config) => {
                info!(path = %path.display(), ?config, "config loaded");
                Some(config)
            }
            Err(err) => {
                warn!(path = %path.display(), %err, "invalid config");
                None
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

pub fn path() -> Option<PathBuf> {
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
            "[output]\nscale = 1.25\n\n[input]\npointer_speed = 0.5\nnatural_scroll = true\ntap_to_click = false\nclick_method = \"areas\"\n",
        )
        .unwrap();
        assert_eq!(config.output.scale, Some(1.25));
        assert_eq!(config.input.pointer_speed, 0.5);
        assert!(config.input.natural_scroll);
        assert!(!config.input.tap_to_click);
        assert_eq!(config.input.click_method, ClickMethod::Areas);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let config = Config::parse("[input]\nnatural_scroll = true\n").unwrap();
        assert_eq!(config.output.scale, None);
        assert!(config.input.tap_to_click);
        assert_eq!(config.input.click_method, ClickMethod::Fingers);
        assert_eq!(config.commands, CommandsConfig::default());
    }

    #[test]
    fn commands_parse() {
        let config = Config::parse(
            "[commands]\nvolume_up = \"wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+\"\nmic_mute = \"\"\n",
        )
        .unwrap();
        assert_eq!(
            config.commands.get(Command::VolumeUp),
            "wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+"
        );
        assert_eq!(config.commands.get(Command::MicMute), "");
        assert_eq!(config.commands.get(Command::Screenshot), SCREENSHOT);
        assert_eq!(
            config.commands.get(Command::BrightnessUp),
            "brightnessctl set +5%"
        );
        assert!(Config::parse("[commands]\nvolume_raise = \"true\"\n").is_err());
        // The section's old name still works.
        let config = Config::parse("[shortcuts]\nmic_mute = \"\"\n").unwrap();
        assert_eq!(config.commands.get(Command::MicMute), "");
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
        assert_eq!(config.keyboard.repeat_delay, 500);
        assert_eq!(config.keyboard.repeat_rate, 1);
    }

    #[test]
    fn sample_config_parses() {
        let sample = include_str!("../resources/config.toml");
        let config = Config::parse(sample).unwrap();
        assert_eq!(config.commands.app(App::Lock), "swaylock -f -c 000000");
        assert_eq!(config.commands.terminal, CommandsConfig::default().terminal);
    }

    #[test]
    fn autostart_parses() {
        let config =
            Config::parse("[autostart]\ncommands = [\"nm-applet\", \"swayidle -w\"]\n").unwrap();
        assert_eq!(config.autostart.commands, ["nm-applet", "swayidle -w"]);
        assert!(Config::default().autostart.commands.is_empty());
    }

    #[test]
    fn panel_parses() {
        let config = Config::parse(
            "[panel]\nposition = \"bottom\"\nmode = \"always\"\nhide_delay_ms = 2500\nslide_in_ms = 150\nslide_out_ms = 0\n",
        )
        .unwrap();
        assert_eq!(config.panel.hide_delay_ms, 2500);
        assert_eq!(config.panel.slide_in_ms, 150);
        assert_eq!(config.panel.slide_out_ms, 0);
        assert_eq!(Config::default().panel.slide_in_ms, 200);
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
    fn notifications_parse() {
        assert_eq!(
            Config::default().notifications.command,
            "rustwm-notifications"
        );
        let config = Config::parse("[notifications]\ncommand = \"\"\n").unwrap();
        assert!(config.notifications.command.is_empty());
    }

    #[test]
    fn overview_parses() {
        assert_eq!(Config::default().overview.file_search, FILE_SEARCH);
        let config = Config::parse("[overview]\nfile_search = \"\"\n").unwrap();
        assert!(config.overview.file_search.is_empty());
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
