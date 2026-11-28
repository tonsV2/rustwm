//! Settings from `~/.config/rustwm/config.toml`.
//!
//! Every setting is optional; a missing file or key uses the default. A file
//! that fails to parse is reported and ignored rather than stopping the
//! compositor. Saving the file applies it while rustwm runs (see
//! [`crate::reload`]).

use std::{env, fs, path::PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

use crate::{
    keyboard::{App, Command},
    night_light::{
        schedule::TimeOfDay,
        temperature::{MIN, NEUTRAL},
    },
};

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
    pub battery: BatteryConfig,
    pub overview: OverviewConfig,
    pub switcher: SwitcherConfig,
    pub lock: LockConfig,
    pub night_light: NightLightConfig,
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
            // `-e` steps along a perceptual curve, so the low end gets as many
            // steps as the high end; `-n1` keeps the backlight from reaching
            // 0, which turns the panel black.
            brightness_up: "brightnessctl -e -n1 set +5%".into(),
            brightness_down: "brightnessctl -e -n1 set 5%-".into(),
            volume_up: "rustwm-volume up 5".into(),
            volume_down: "rustwm-volume down 5".into(),
            volume_mute: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".into(),
            mic_mute: "wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle".into(),
            screenshot: SCREENSHOT.into(),
            terminal: "rustwm-terminal".into(),
            file_manager: "xdg-open \"$HOME\"".into(),
            browser: "gtk-launch \"$(xdg-settings get default-web-browser)\"".into(),
            lock: "swaylock -f -c 000000 --indicator-idle-visible".into(),
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

/// The battery watcher, which warns through the notification daemon before the battery runs out.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BatteryConfig {
    /// Started right after the notification daemon. Empty means no warnings.
    pub command: String,
}

impl Default for BatteryConfig {
    fn default() -> Self {
        Self {
            command: "rustwm-battery 20 10 5".into(),
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
    /// Open or close the overview when the pointer touches the screen's
    /// top left corner, as GNOME's hot corner does.
    pub hot_corner: bool,
}

impl Default for OverviewConfig {
    fn default() -> Self {
        Self {
            file_search: FILE_SEARCH.into(),
            hot_corner: true,
        }
    }
}

/// Files under the home directory whose path contains the text, leaving
/// out hidden directories, from the plocate database.
const FILE_SEARCH: &str = "plocate -i -- \"$HOME/\" \"$1\" | grep -v '/\\.' | head -20";

/// The Alt+Tab switcher.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SwitcherConfig {
    /// Wrap the windows onto more rows, shrinking the thumbnails when even
    /// those are not enough, so every window shows at once. Off keeps one
    /// row of full-size thumbnails that scrolls.
    pub wrap: bool,
}

impl Default for SwitcherConfig {
    fn default() -> Self {
        Self { wrap: true }
    }
}

/// The screen lock's fade to black.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LockConfig {
    /// How long the screen takes to fade to black before an idle timeout
    /// longer than that, such as swayidle's lock, fires, in milliseconds.
    /// 0 leaves the screen as it is until the lock.
    pub fade_ms: u64,
}

impl Default for LockConfig {
    fn default() -> Self {
        Self { fade_ms: 10000 }
    }
}

/// The night light: a warm tint at night, set in the display's gamma table.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NightLightConfig {
    /// Follow the schedule. Off still lets Super+n warm the screen until morning.
    pub enabled: bool,
    /// The colour temperature at night in Kelvin, from 1000 (warmest) to 6500 (no tint).
    pub temperature: u32,
    pub schedule: NightLightSchedule,
    /// When the fixed schedule's night starts, local time "HH:MM". It is fully warm by then.
    pub from: TimeOfDay,
    /// When the fixed schedule's night ends. Equal to `from` means never.
    pub to: TimeOfDay,
    /// How long the fixed schedule takes to warm up before `from` and to cool down before `to`, in minutes. 0 switches at once.
    pub transition_minutes: u32,
    /// Where you are, for the sunset schedule: degrees north (negative south).
    pub latitude: Option<f64>,
    /// Degrees east (negative west).
    pub longitude: Option<f64>,
}

impl Default for NightLightConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            temperature: 2700,
            schedule: NightLightSchedule::Fixed,
            from: TimeOfDay::new(20, 0),
            to: TimeOfDay::new(6, 0),
            transition_minutes: 60,
            latitude: None,
            longitude: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NightLightSchedule {
    /// From `from` to `to` every day.
    Fixed,
    /// From sunset to sunrise at `latitude` and `longitude`.
    Sunset,
}

impl NightLightConfig {
    /// What is wrong with the values, which TOML alone cannot check.
    fn problem(&self) -> Option<String> {
        if !(MIN..=NEUTRAL).contains(&self.temperature) {
            return Some(format!(
                "night_light.temperature is {}, but must be from {MIN} to {NEUTRAL}",
                self.temperature
            ));
        }
        if self.latitude.is_some_and(|l| !(-90.0..=90.0).contains(&l)) {
            return Some("night_light.latitude must be from -90 to 90".into());
        }
        if self
            .longitude
            .is_some_and(|l| !(-180.0..=180.0).contains(&l))
        {
            return Some("night_light.longitude must be from -180 to 180".into());
        }
        if self.latitude.is_some() != self.longitude.is_some() {
            return Some("set both night_light.latitude and longitude".into());
        }
        None
    }
}

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
        if let Some(problem) = config.night_light.problem() {
            return Err(serde::de::Error::custom(problem));
        }
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
            "brightnessctl -e -n1 set +5%"
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
        assert_eq!(
            config.commands.app(App::Lock),
            "swaylock -f -c 000000 --indicator-idle-visible"
        );
        assert_eq!(config.lock, LockConfig::default());
        assert_eq!(config.night_light, NightLightConfig::default());
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
    fn battery_parses() {
        assert_eq!(Config::default().battery.command, "rustwm-battery 20 10 5");
        let config = Config::parse("[battery]\ncommand = \"\"\n").unwrap();
        assert!(config.battery.command.is_empty());
    }

    #[test]
    fn switcher_parses() {
        assert!(Config::default().switcher.wrap);
        let config = Config::parse("[switcher]\nwrap = false\n").unwrap();
        assert!(!config.switcher.wrap);
    }

    #[test]
    fn overview_parses() {
        assert_eq!(Config::default().overview.file_search, FILE_SEARCH);
        let config = Config::parse("[overview]\nfile_search = \"\"\n").unwrap();
        assert!(config.overview.file_search.is_empty());
        assert!(Config::default().overview.hot_corner);
        let config = Config::parse("[overview]\nhot_corner = false\n").unwrap();
        assert!(!config.overview.hot_corner);
    }

    #[test]
    fn lock_parses() {
        assert_eq!(Config::default().lock.fade_ms, 10000);
        let config = Config::parse("[lock]\nfade_ms = 0\n").unwrap();
        assert_eq!(config.lock.fade_ms, 0);
        assert!(Config::parse("[lock]\nfade = 5\n").is_err());
    }

    #[test]
    fn night_light_parses() {
        let defaults = Config::default().night_light;
        assert!(!defaults.enabled);
        assert_eq!(defaults.temperature, 2700);
        assert_eq!(defaults.schedule, NightLightSchedule::Fixed);
        assert_eq!(
            (defaults.from.minutes(), defaults.to.minutes()),
            (1200, 360)
        );
        assert_eq!(defaults.transition_minutes, 60);

        let config = Config::parse(
            "[night_light]\nenabled = true\ntemperature = 3500\nfrom = \"21:30\"\nto = \"7:00\"\ntransition_minutes = 0\n",
        )
        .unwrap()
        .night_light;
        assert!(config.enabled);
        assert_eq!(config.temperature, 3500);
        assert_eq!(config.from, TimeOfDay::new(21, 30));
        assert_eq!(config.to, TimeOfDay::new(7, 0));
        assert_eq!(config.transition_minutes, 0);

        let config = Config::parse(
            "[night_light]\nschedule = \"sunset\"\nlatitude = 55.68\nlongitude = 12.57\n",
        )
        .unwrap()
        .night_light;
        assert_eq!(config.schedule, NightLightSchedule::Sunset);
        assert_eq!(
            (config.latitude, config.longitude),
            (Some(55.68), Some(12.57))
        );
    }

    #[test]
    fn night_light_rejects_bad_values() {
        for bad in [
            "temperature = 500",
            "temperature = 7000",
            "latitude = 91.0\nlongitude = 0.0",
            "latitude = 0.0\nlongitude = -181.0",
            "latitude = 55.68",
            "from = \"25:00\"",
            "schedule = \"sometimes\"",
            "colour = 3000",
        ] {
            let text = format!("[night_light]\n{bad}\n");
            assert!(Config::parse(&text).is_err(), "{bad}");
        }
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
