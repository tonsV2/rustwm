//! The on-screen indicator for the brightness and volume keys: a small
//! panel near the bottom of the screen with an icon and a level bar, hidden
//! again [`HIDE_DELAY`] after the last key press.
//!
//! The compositor does not compute the level. After a key's command has
//! finished, the thread that waited for it reads the level back with
//! `wpctl` or `brightnessctl` and hands it to the event loop through a
//! channel, so a slow read never holds up drawing. A custom command in
//! `[commands]` still shows the right level as long as it changes the
//! default device.

use std::{
    collections::HashMap,
    process::{self, Stdio},
    time::Duration,
};

use resvg::tiny_skia::{Pixmap, PremultipliedColorU8};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    output::Output,
    reexports::calloop::{
        LoopHandle, RegistrationToken,
        channel::{self, Sender},
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point, Transform},
};
use tracing::{debug, warn};

use crate::{
    apps, commands,
    keyboard::Command,
    state::State,
    switcher::{BACKGROUND, Canvas, HIGHLIGHT, Image, RADIUS, SwitcherElement, TEXT_COLOR},
};

/// How long the indicator stays after the last level read.
const HIDE_DELAY: Duration = Duration::from_millis(1500);

// Sizes in logical pixels.
const WIDTH: i32 = 280;
const HEIGHT: i32 = 52;
const PADDING: i32 = 18;
const ICON_SIZE: i32 = 20;
/// Between the icon and the bar.
const GAP: i32 = 14;
const BAR_HEIGHT: i32 = 6;
/// From the bottom of the screen to the bottom of the panel.
const BOTTOM_MARGIN: i32 = 96;

/// The bar's filled part while muted, as straight (not premultiplied) RGBA.
const MUTED_BAR: [f32; 4] = [0.95, 0.95, 0.95, 0.3];

/// What a key changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Speakers,
    Microphone,
    Screen,
}

impl Device {
    fn of(command: Command) -> Self {
        match command {
            Command::BrightnessUp | Command::BrightnessDown => Device::Screen,
            Command::VolumeUp | Command::VolumeDown | Command::VolumeMute => Device::Speakers,
            Command::MicMute => Device::Microphone,
        }
    }

    /// Reads the default device's level. Blocks while the tool runs.
    fn read(self) -> Option<Level> {
        let (program, args): (&str, &[&str]) = match self {
            Device::Speakers => ("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]),
            Device::Microphone => ("wpctl", &["get-volume", "@DEFAULT_AUDIO_SOURCE@"]),
            Device::Screen => ("brightnessctl", &["-m"]),
        };
        let output = process::Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success());
        let Some(output) = output else {
            debug!(program, "cannot read level");
            return None;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let (value, muted) = match self {
            Device::Screen => (parse_brightness(&text)?, false),
            Device::Speakers | Device::Microphone => parse_volume(&text)?,
        };
        Some(Level {
            device: self,
            value,
            muted,
        })
    }
}

/// A device's level as the indicator shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    device: Device,
    /// 0 to 1; volume can go above 1 when amplified.
    value: f64,
    muted: bool,
}

impl Level {
    fn icon_name(&self) -> &'static str {
        let step = if self.value <= 0.0 {
            0
        } else if self.value < 1.0 / 3.0 {
            1
        } else if self.value < 2.0 / 3.0 {
            2
        } else {
            3
        };
        match self.device {
            Device::Screen => "display-brightness-symbolic",
            Device::Speakers if self.muted => "audio-volume-muted-symbolic",
            Device::Speakers => [
                "audio-volume-muted-symbolic",
                "audio-volume-low-symbolic",
                "audio-volume-medium-symbolic",
                "audio-volume-high-symbolic",
            ][step],
            Device::Microphone if self.muted => "microphone-sensitivity-muted-symbolic",
            Device::Microphone => [
                "microphone-sensitivity-muted-symbolic",
                "microphone-sensitivity-low-symbolic",
                "microphone-sensitivity-medium-symbolic",
                "microphone-sensitivity-high-symbolic",
            ][step],
        }
    }
}

/// The level read after the command numbered `seq` finished.
pub struct Report {
    seq: u64,
    level: Level,
}

pub struct Osd {
    sender: Sender<Report>,
    /// Numbers the commands run, so reads finishing out of order during key
    /// repeat do not move the bar backwards.
    next_seq: u64,
    shown_seq: u64,
    /// The level on screen; `None` while hidden.
    shown: Option<Level>,
    hide_timer: Option<RegistrationToken>,
    /// The panel last drawn, for this level and scale.
    cache: Option<((Level, u64), MemoryRenderBuffer)>,
    /// Icons by name and pixel size; `None` when the icon was not found.
    icons: HashMap<(&'static str, u32), Option<Pixmap>>,
}

impl Osd {
    pub fn new(loop_handle: &LoopHandle<'static, State>) -> anyhow::Result<Self> {
        let (sender, receiver) = channel::channel();
        loop_handle
            .insert_source(receiver, |event, _, state| {
                if let channel::Event::Msg(report) = event {
                    state.show_level(report);
                }
            })
            .map_err(|err| anyhow::anyhow!("failed to listen for levels: {err}"))?;
        Ok(Self {
            sender,
            next_seq: 0,
            shown_seq: 0,
            shown: None,
            hide_timer: None,
            cache: None,
            icons: HashMap::new(),
        })
    }

    /// Draws the panel for `level`, or reuses the last drawing.
    fn image(&mut self, level: Level, scale: f64) -> &MemoryRenderBuffer {
        let key = (level, scale.to_bits());
        if self.cache.as_ref().is_none_or(|(cached, _)| *cached != key) {
            let icon_px = (ICON_SIZE as f64 * scale).round() as u32;
            let icon = self
                .icons
                .entry((level.icon_name(), icon_px))
                .or_insert_with(|| {
                    let icon = apps::load_status_icon(level.icon_name(), icon_px);
                    if icon.is_none() {
                        debug!(name = level.icon_name(), "no indicator icon");
                    }
                    icon.map(|icon| tint(icon, TEXT_COLOR))
                })
                .as_ref();
            let image = draw(level, icon, scale);
            let buffer = MemoryRenderBuffer::from_slice(
                &image.pixels,
                Fourcc::Argb8888,
                (image.width, image.height),
                1,
                Transform::Normal,
                None,
            );
            self.cache = Some((key, buffer));
        }
        &self.cache.as_ref().expect("cache was just filled").1
    }
}

impl State {
    /// Runs the command bound to a brightness or volume key and, once it has
    /// finished, shows the new level.
    pub fn run_command(&mut self, command: Command) {
        let reader = self.config.osd.enabled.then(|| {
            self.osd.next_seq += 1;
            (self.osd.sender.clone(), self.osd.next_seq)
        });
        let device = Device::of(command);
        commands::run(
            self.config.commands.get(command),
            &self.socket_name,
            move || {
                let Some((sender, seq)) = reader else { return };
                if let Some(level) = device.read() {
                    // Fails only when the compositor is shutting down.
                    let _ = sender.send(Report { seq, level });
                }
            },
        );
    }

    fn show_level(&mut self, report: Report) {
        if report.seq < self.osd.shown_seq {
            return;
        }
        self.osd.shown_seq = report.seq;
        self.osd.shown = Some(report.level);
        if let Some(token) = self.osd.hide_timer.take() {
            self.loop_handle.remove(token);
        }
        let timer = Timer::from_duration(HIDE_DELAY);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.osd.shown = None;
            state.osd.hide_timer = None;
            state.osd.cache = None;
            TimeoutAction::Drop
        });
        match inserted {
            Ok(token) => self.osd.hide_timer = Some(token),
            Err(err) => {
                // Better no indicator than one that never goes away.
                warn!(%err, "failed to schedule hiding the indicator");
                self.osd.shown = None;
            }
        }
    }

    /// Everything drawn over the windows on `output`, front to back: the
    /// brightness and volume indicator, then the Alt+Tab switcher.
    pub fn overlay_elements(&mut self, output: &Output) -> Vec<SwitcherElement> {
        let mut elements = self.osd_elements(output);
        elements.extend(self.switcher_elements(output));
        elements
    }

    /// The indicator for `output`; empty while it is hidden.
    fn osd_elements(&mut self, output: &Output) -> Vec<SwitcherElement> {
        let Some(level) = self.osd.shown else {
            return Vec::new();
        };
        let Some(geometry) = self.space.output_geometry(output) else {
            return Vec::new();
        };
        let scale = output.current_scale().fractional_scale();
        // Whole physical pixels, so the image is not blurred.
        let origin = Point::<i32, Logical>::from((
            (geometry.size.w - WIDTH) / 2,
            (geometry.size.h - BOTTOM_MARGIN - HEIGHT).max(0),
        ))
        .to_f64()
        .to_physical(scale)
        .to_i32_round::<i32>();

        let buffer = self.osd.image(level, scale);
        MemoryRenderBufferRenderElement::from_buffer(
            self.backend.renderer(),
            origin.to_f64(),
            buffer,
            None,
            None,
            Some((WIDTH, HEIGHT).into()),
            Kind::Unspecified,
        )
        .map_err(|err| warn!(?err, "failed to draw indicator"))
        .ok()
        .map(SwitcherElement::Image)
        .into_iter()
        .collect()
    }
}

/// Parses `wpctl get-volume` output such as `Volume: 0.45` or
/// `Volume: 0.45 [MUTED]` into the volume and whether it is muted.
fn parse_volume(text: &str) -> Option<(f64, bool)> {
    let mut words = text.trim().strip_prefix("Volume:")?.split_whitespace();
    let value = words.next()?.parse::<f64>().ok()?;
    let muted = words.any(|word| word == "[MUTED]");
    Some((value.max(0.0), muted))
}

/// Parses `brightnessctl -m` output such as
/// `intel_backlight,backlight,25070,65%,38787` into a fraction.
fn parse_brightness(text: &str) -> Option<f64> {
    let percent = text.lines().next()?.split(',').nth(3)?.strip_suffix('%')?;
    Some((percent.parse::<f64>().ok()? / 100.0).clamp(0.0, 1.0))
}

/// `icon` with every pixel painted `color`, keeping its shape. Symbolic
/// icons are drawn dark, for light backgrounds.
fn tint(mut icon: Pixmap, color: [f32; 4]) -> Pixmap {
    for pixel in icon.pixels_mut() {
        let alpha = pixel.alpha() as f32 / 255.0 * color[3];
        let channel = |c: f32| (c * alpha * 255.0).round() as u8;
        let tinted = PremultipliedColorU8::from_rgba(
            channel(color[0]),
            channel(color[1]),
            channel(color[2]),
            (alpha * 255.0).round() as u8,
        );
        if let Some(tinted) = tinted {
            *pixel = tinted;
        }
    }
    icon
}

/// Draws the panel: the background, `icon` on the left if there is one, and
/// the level bar, dimmed while muted.
fn draw(level: Level, icon: Option<&Pixmap>, scale: f64) -> Image {
    let width = (WIDTH as f64 * scale).round() as i32;
    let height = (HEIGHT as f64 * scale).round() as i32;
    let mut canvas = Canvas::new(width, height);
    // Scale from logical to image pixels, exact for this image's size.
    let s = width as f32 / WIDTH as f32;

    canvas.rounded_rect(
        (0.0, 0.0, width as f32, height as f32),
        RADIUS * s,
        BACKGROUND,
    );

    let mut bar_left = PADDING as f32 * s;
    if let Some(icon) = icon {
        let top = (height as f32 - icon.height() as f32) / 2.0;
        canvas.pixmap(icon, bar_left.round() as i32, top.round() as i32);
        bar_left += (ICON_SIZE + GAP) as f32 * s;
    }
    let bar_width = width as f32 - PADDING as f32 * s - bar_left;
    let bar_height = BAR_HEIGHT as f32 * s;
    let bar_top = (height as f32 - bar_height) / 2.0;
    let radius = bar_height / 2.0;
    canvas.rounded_rect(
        (bar_left, bar_top, bar_width, bar_height),
        radius,
        HIGHLIGHT,
    );
    let filled = bar_width * level.value.clamp(0.0, 1.0) as f32;
    if filled > 0.0 {
        let color = if level.muted { MUTED_BAR } else { TEXT_COLOR };
        canvas.rounded_rect((bar_left, bar_top, filled, bar_height), radius, color);
    }

    canvas.into_image()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(device: Device, value: f64, muted: bool) -> Level {
        Level {
            device,
            value,
            muted,
        }
    }

    fn pixel(image: &Image, x: i32, y: i32) -> [u8; 4] {
        let i = ((y * image.width + x) * 4) as usize;
        image.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn volume_parses() {
        assert_eq!(parse_volume("Volume: 0.45\n"), Some((0.45, false)));
        assert_eq!(parse_volume("Volume: 0.45 [MUTED]\n"), Some((0.45, true)));
        assert_eq!(parse_volume("Volume: 1.20"), Some((1.2, false)));
        assert_eq!(parse_volume(""), None);
        assert_eq!(parse_volume("Translate ID error"), None);
    }

    #[test]
    fn brightness_parses() {
        assert_eq!(
            parse_brightness("intel_backlight,backlight,25070,65%,38787\n"),
            Some(0.65)
        );
        assert_eq!(
            parse_brightness("amdgpu_bl0,backlight,255,100%,255"),
            Some(1.0)
        );
        assert_eq!(parse_brightness("intel_backlight,backlight,25070"), None);
        assert_eq!(parse_brightness(""), None);
    }

    #[test]
    fn commands_map_to_devices() {
        assert_eq!(Device::of(Command::BrightnessDown), Device::Screen);
        assert_eq!(Device::of(Command::VolumeMute), Device::Speakers);
        assert_eq!(Device::of(Command::MicMute), Device::Microphone);
    }

    #[test]
    fn icons_follow_level_and_mute() {
        let name = |device, value, muted| level(device, value, muted).icon_name();
        assert_eq!(
            name(Device::Speakers, 0.2, false),
            "audio-volume-low-symbolic"
        );
        assert_eq!(
            name(Device::Speakers, 0.5, false),
            "audio-volume-medium-symbolic"
        );
        assert_eq!(
            name(Device::Speakers, 1.2, false),
            "audio-volume-high-symbolic"
        );
        assert_eq!(
            name(Device::Speakers, 0.0, false),
            "audio-volume-muted-symbolic"
        );
        assert_eq!(
            name(Device::Speakers, 0.8, true),
            "audio-volume-muted-symbolic"
        );
        assert_eq!(
            name(Device::Microphone, 0.8, true),
            "microphone-sensitivity-muted-symbolic"
        );
        assert_eq!(
            name(Device::Screen, 0.8, false),
            "display-brightness-symbolic"
        );
    }

    #[test]
    fn panel_follows_scale() {
        let image = draw(level(Device::Screen, 0.5, false), None, 1.25);
        assert_eq!(image.width, (WIDTH as f64 * 1.25).round() as i32);
        assert_eq!(image.height, (HEIGHT as f64 * 1.25).round() as i32);
        assert_eq!(pixel(&image, 0, 0)[3], 0);
    }

    #[test]
    fn bar_fills_to_the_level_and_dims_when_muted() {
        let middle = HEIGHT / 2;
        let bar_at = |value, muted, x| {
            let image = draw(level(Device::Speakers, value, muted), None, 1.0);
            pixel(&image, x, middle)[0]
        };
        let left = PADDING + 10;
        let right = WIDTH - PADDING - 10;
        // Filled on the left, only the track on the right.
        assert!(bar_at(0.5, false, left) > bar_at(0.5, false, right));
        // A full bar reaches the right end.
        assert!(bar_at(1.0, false, right) > bar_at(0.5, false, right));
        // Muted, the filled part is dimmer.
        assert!(bar_at(0.5, false, left) > bar_at(0.5, true, left));
    }

    #[test]
    fn icon_is_tinted_and_moves_the_bar() {
        let mut icon = Pixmap::new(ICON_SIZE as u32, ICON_SIZE as u32).unwrap();
        icon.fill(resvg::tiny_skia::Color::from_rgba8(46, 52, 54, 255));
        let icon = tint(icon, [1.0, 1.0, 1.0, 1.0]);
        let image = draw(level(Device::Screen, 1.0, false), Some(&icon), 1.0);
        let at_icon = pixel(&image, PADDING + ICON_SIZE / 2, HEIGHT / 2);
        assert_eq!(at_icon, [255, 255, 255, 255]);
        // Between the icon and the bar there is only background.
        let in_gap = pixel(&image, PADDING + ICON_SIZE + GAP / 2, HEIGHT / 2);
        assert!(in_gap[0] < 60);
    }
}
