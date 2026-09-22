//! The Alt+Tab switcher: a list of windows, most recently used first,
//! drawn in the middle of the screen while Alt is held.
//!
//! Stepping only moves the highlight; the highlighted window is raised and
//! focused when Alt is released. A quick Alt+Tab finishes before
//! [`SHOW_DELAY`] passes, so it toggles between the two most recent windows
//! without the list flashing up.

use std::{collections::HashMap, env, fs, path::PathBuf, process::Command, time::Duration};

use fontdue::{Font, FontSettings};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{Kind, memory::MemoryRenderBuffer, memory::MemoryRenderBufferRenderElement},
            gles::GlesRenderer,
        },
    },
    output::Output,
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Point, Transform},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};
use tracing::warn;

use crate::state::State;

/// How long Alt must be held after the first Tab before the list appears.
const SHOW_DELAY: Duration = Duration::from_millis(150);

// Sizes in logical pixels.
const MAX_WIDTH: i32 = 720;
const SCREEN_MARGIN: i32 = 32;
const PADDING: i32 = 12;
const ROW_HEIGHT: i32 = 40;
const ROW_PADDING: i32 = 12;
const APP_COLUMN: i32 = 180;
const TITLE_SIZE: f32 = 15.0;
const APP_SIZE: f32 = 13.0;
const RADIUS: f32 = 12.0;

// Colours as straight (not premultiplied) RGBA.
const BACKGROUND: [f32; 4] = [0.12, 0.12, 0.13, 0.94];
const HIGHLIGHT: [f32; 4] = [1.0, 1.0, 1.0, 0.14];
const TITLE_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];
const APP_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 0.6];

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The application's name, such as "Ghostty".
    pub app: String,
    /// The window title, which tells windows of the same application apart.
    pub title: String,
}

/// What the switcher shows: the rows and the highlighted one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub entries: Vec<Entry>,
    pub selected: usize,
}

#[derive(Default)]
pub struct Switcher {
    /// Loaded on first use; the inner `None` means no font was found.
    font: Option<Option<Font>>,
    visible: bool,
    /// Bumped whenever the switcher is hidden, so a pending show timer from
    /// an earlier switch does nothing.
    session: u64,
    /// The last image drawn. Reusing it while nothing changed keeps the
    /// renderer from redrawing the list every frame.
    cache: Option<(CacheKey, MemoryRenderBuffer, (i32, i32))>,
    /// Application names by app id, read from desktop files.
    app_names: HashMap<String, String>,
}

#[derive(PartialEq)]
struct CacheKey {
    view: View,
    screen: (i32, i32),
    scale: u64,
}

impl Switcher {
    pub fn hide(&mut self) {
        self.visible = false;
        self.session += 1;
        self.cache = None;
    }

    /// The list as a render element for an output of `screen` logical size,
    /// or `None` while it is hidden.
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        view: Option<View>,
        screen: (i32, i32),
        scale: f64,
    ) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
        let view = view.filter(|_| self.visible)?;
        let key = CacheKey {
            view,
            screen,
            scale: scale.to_bits(),
        };
        if self
            .cache
            .as_ref()
            .is_none_or(|(cached, ..)| *cached != key)
        {
            let font = self.font.get_or_insert_with(load_font).as_ref();
            let image = draw(font, &key.view, screen, scale);
            let buffer = MemoryRenderBuffer::from_slice(
                &image.pixels,
                Fourcc::Argb8888,
                (image.width, image.height),
                1,
                Transform::Normal,
                None,
            );
            self.cache = Some((key, buffer, image.logical_size));
        }
        let (_, buffer, (width, height)) = self.cache.as_ref()?;

        let location = Point::<i32, smithay::utils::Logical>::from((
            (screen.0 - width) / 2,
            (screen.1 - height) / 2,
        ))
        .to_f64()
        .to_physical(scale)
        .to_i32_round::<i32>()
        .to_f64();
        match MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location,
            buffer,
            None,
            None,
            Some((*width, *height).into()),
            Kind::Unspecified,
        ) {
            Ok(element) => Some(element),
            Err(err) => {
                warn!(?err, "failed to draw window switcher");
                None
            }
        }
    }

    fn app_name(&mut self, app_id: &str) -> String {
        self.app_names
            .entry(app_id.to_string())
            .or_insert_with(|| desktop_name(app_id).unwrap_or_else(|| fallback_name(app_id)))
            .clone()
    }
}

impl State {
    /// Moves the Alt+Tab highlight, starting a switch if none is running.
    pub fn step_switch(&mut self, forward: bool) {
        let starting = !self.focus.is_switching();
        if self.focus.step(forward).is_none() {
            return;
        }
        if !starting {
            // A second Tab shows the list at once.
            self.switcher.visible = true;
            return;
        }

        self.switcher.hide();
        let session = self.switcher.session;
        let timer = Timer::from_duration(SHOW_DELAY);
        let inserted = self.loop_handle.insert_source(timer, move |_, _, state| {
            if state.switcher.session == session && state.focus.is_switching() {
                state.switcher.visible = true;
            }
            TimeoutAction::Drop
        });
        if let Err(err) = inserted {
            warn!(%err, "failed to schedule window switcher");
            self.switcher.visible = true;
        }
    }

    /// The rows to show while switching.
    pub fn switcher_view(&mut self) -> Option<View> {
        let (ids, selected) = self.focus.switch_list()?;
        let windows: Vec<_> = ids.iter().filter_map(|&id| self.window(id)).collect();
        let details: Vec<(String, String)> = windows
            .iter()
            .filter_map(|w| w.wl_surface())
            .map(|surface| {
                with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .and_then(|data| data.lock().ok())
                        .map(|data| {
                            (
                                data.app_id.clone().unwrap_or_default(),
                                data.title.clone().unwrap_or_default(),
                            )
                        })
                        .unwrap_or_default()
                })
            })
            .collect();
        // Every id in the history has a window, so indices still match.
        if details.len() != ids.len() {
            return None;
        }

        let entries = details
            .into_iter()
            .map(|(app_id, title)| Entry {
                app: self.switcher.app_name(&app_id),
                title,
            })
            .collect();
        Some(View { entries, selected })
    }

    /// The switcher for `output`, ready to draw on top of everything else.
    pub fn switcher_element(
        &mut self,
        output: &Output,
    ) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
        let view = self.switcher_view();
        let geometry = self.space.output_geometry(output)?;
        let scale = output.current_scale().fractional_scale();
        let renderer = self.backend.renderer();
        self.switcher
            .element(renderer, view, (geometry.size.w, geometry.size.h), scale)
    }
}

/// An ARGB8888 image in physical pixels.
struct Image {
    width: i32,
    height: i32,
    /// Premultiplied, in memory order B, G, R, A.
    pixels: Vec<u8>,
    logical_size: (i32, i32),
}

/// Draws the list for a screen of `screen` logical size.
fn draw(font: Option<&Font>, view: &View, screen: (i32, i32), scale: f64) -> Image {
    let width = MAX_WIDTH.min(screen.0 - 2 * SCREEN_MARGIN).max(ROW_HEIGHT);
    let max_rows = ((screen.1 - 2 * SCREEN_MARGIN - 2 * PADDING) / ROW_HEIGHT).max(1) as usize;
    let rows = view.entries.len().min(max_rows);
    let height = rows as i32 * ROW_HEIGHT + 2 * PADDING;
    // Scroll so the highlight stays near the middle of a long list.
    let first = view
        .selected
        .saturating_sub(rows / 2)
        .min(view.entries.len() - rows);

    let mut canvas = Canvas::new(
        (width as f64 * scale).round() as i32,
        (height as f64 * scale).round() as i32,
    );
    // Scale from logical to image pixels, exact for this image's size.
    let s = canvas.width as f32 / width as f32;

    canvas.rounded_rect(
        0.0,
        0.0,
        canvas.width as f32,
        canvas.height as f32,
        RADIUS * s,
        BACKGROUND,
    );

    for (row, entry) in view.entries[first..first + rows].iter().enumerate() {
        let top = (PADDING + row as i32 * ROW_HEIGHT) as f32 * s;
        if first + row == view.selected {
            canvas.rounded_rect(
                PADDING as f32 * s,
                top,
                (width - 2 * PADDING) as f32 * s,
                ROW_HEIGHT as f32 * s,
                (RADIUS - 4.0) * s,
                HIGHLIGHT,
            );
        }
        let Some(font) = font else { continue };

        let left = (PADDING + ROW_PADDING) as f32 * s;
        let title_left = left + APP_COLUMN as f32 * s;
        let right = (width - PADDING - ROW_PADDING) as f32 * s;
        let middle = top + ROW_HEIGHT as f32 * s / 2.0;
        canvas.text(
            font,
            &entry.app,
            TextStyle {
                size: APP_SIZE * s,
                color: APP_COLOR,
            },
            left,
            title_left - left - ROW_PADDING as f32 * s,
            middle,
        );
        canvas.text(
            font,
            &entry.title,
            TextStyle {
                size: TITLE_SIZE * s,
                color: TITLE_COLOR,
            },
            title_left,
            right - title_left,
            middle,
        );
    }

    Image {
        width: canvas.width,
        height: canvas.height,
        pixels: canvas.pixels,
        logical_size: (width, height),
    }
}

struct TextStyle {
    size: f32,
    color: [f32; 4],
}

struct Canvas {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    /// Blends `color` over the pixel at (x, y) with `coverage` from 0 to 1.
    fn blend(&mut self, x: i32, y: i32, color: [f32; 4], coverage: f32) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height || coverage <= 0.0 {
            return;
        }
        let i = ((y * self.width + x) * 4) as usize;
        let alpha = color[3] * coverage.min(1.0);
        let src = [color[2] * alpha, color[1] * alpha, color[0] * alpha, alpha];
        for (dst, src) in self.pixels[i..i + 4].iter_mut().zip(src) {
            let blended = src * 255.0 + *dst as f32 * (1.0 - alpha);
            *dst = blended.round().clamp(0.0, 255.0) as u8;
        }
    }

    /// Fills a rectangle with rounded, anti-aliased corners.
    fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: [f32; 4]) {
        let radius = radius.min(w / 2.0).min(h / 2.0);
        for py in y.floor() as i32..(y + h).ceil() as i32 {
            for px in x.floor() as i32..(x + w).ceil() as i32 {
                // Distance from the pixel centre to the shape's edge,
                // negative inside.
                let cx = px as f32 + 0.5;
                let cy = py as f32 + 0.5;
                let dx = (x + radius - cx).max(cx - (x + w - radius)).max(0.0);
                let dy = (y + radius - cy).max(cy - (y + h - radius)).max(0.0);
                let corner = (dx * dx + dy * dy).sqrt() - radius;
                let edge = (x - cx).max(cx - (x + w)).max(y - cy).max(cy - (y + h));
                let distance = if dx > 0.0 && dy > 0.0 { corner } else { edge };
                self.blend(px, py, color, 0.5 - distance);
            }
        }
    }

    /// Draws one line of text vertically centred on `middle`, cut short with
    /// an ellipsis if it is wider than `max_width`.
    fn text(
        &mut self,
        font: &Font,
        text: &str,
        style: TextStyle,
        left: f32,
        max_width: f32,
        middle: f32,
    ) {
        let TextStyle { size, color } = style;
        let text = fit(font, text, size, max_width);
        let Some(line) = font.horizontal_line_metrics(size) else {
            return;
        };
        let baseline = (middle + (line.ascent + line.descent) / 2.0).round() as i32;

        let mut pen = left;
        let mut previous = None;
        for c in text.chars() {
            if let Some(kern) = previous.and_then(|p| font.horizontal_kern(p, c, size)) {
                pen += kern;
            }
            let (metrics, coverage) = font.rasterize(c, size);
            let x0 = pen.round() as i32 + metrics.xmin;
            let y0 = baseline - metrics.height as i32 - metrics.ymin;
            for (i, &value) in coverage.iter().enumerate() {
                let x = x0 + (i % metrics.width.max(1)) as i32;
                let y = y0 + (i / metrics.width.max(1)) as i32;
                self.blend(x, y, color, value as f32 / 255.0);
            }
            pen += metrics.advance_width;
            previous = Some(c);
        }
    }
}

/// `text` shortened with a trailing ellipsis so it fits in `max_width`.
fn fit(font: &Font, text: &str, size: f32, max_width: f32) -> String {
    let width = |s: &str| -> f32 { s.chars().map(|c| font.metrics(c, size).advance_width).sum() };
    if width(text) <= max_width {
        return text.to_string();
    }
    let ellipsis = width("…");
    let mut used = 0.0;
    let mut result = String::new();
    for c in text.chars() {
        used += font.metrics(c, size).advance_width;
        if used + ellipsis > max_width {
            break;
        }
        result.push(c);
    }
    result.trim_end().to_string() + "…"
}

/// Loads the desktop's default sans-serif font through fontconfig.
fn load_font() -> Option<Font> {
    let path = Command::new("fc-match")
        .args(["--format=%{file}", "sans-serif"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .filter(|path| !path.is_empty());
    let Some(path) = path else {
        warn!("no font found (is fontconfig installed?); the window switcher shows no titles");
        return None;
    };
    let font = fs::read(&path)
        .map_err(|err| err.to_string())
        .and_then(|data| Font::from_bytes(data, FontSettings::default()).map_err(str::to_string));
    match font {
        Ok(font) => Some(font),
        Err(err) => {
            warn!(
                path,
                err, "cannot load font; the window switcher shows no titles"
            );
            None
        }
    }
}

/// The `Name` from the application's desktop file, found by app id.
fn desktop_name(app_id: &str) -> Option<String> {
    if app_id.is_empty() || app_id.contains('/') {
        return None;
    }
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let data_dirs = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let dirs = data_home
        .into_iter()
        .chain(data_dirs.split(':').map(PathBuf::from));

    let file = format!("{app_id}.desktop");
    dirs.map(|dir| dir.join("applications").join(&file))
        .find_map(|path| fs::read_to_string(path).ok())
        .and_then(|text| parse_desktop_name(&text))
}

fn parse_desktop_name(text: &str) -> Option<String> {
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some(name) = line.strip_prefix("Name=") {
            return Some(name.to_string());
        }
    }
    None
}

/// A readable name from the app id alone: "com.mitchellh.ghostty" becomes
/// "Ghostty".
fn fallback_name(app_id: &str) -> String {
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(n: usize, selected: usize) -> View {
        View {
            entries: (0..n)
                .map(|i| Entry {
                    app: "App".into(),
                    title: format!("Window {i}"),
                })
                .collect(),
            selected,
        }
    }

    fn alpha(image: &Image, x: i32, y: i32) -> u8 {
        image.pixels[((y * image.width + x) * 4 + 3) as usize]
    }

    #[test]
    fn list_is_sized_to_rows_and_scale() {
        let image = draw(None, &view(3, 1), (1920, 1080), 1.0);
        assert_eq!(
            image.logical_size,
            (MAX_WIDTH, 3 * ROW_HEIGHT + 2 * PADDING)
        );
        assert_eq!((image.width, image.height), image.logical_size);

        let image = draw(None, &view(3, 1), (1920, 1080), 1.25);
        assert_eq!(image.width, 900);
        assert_eq!(image.height, 180);
    }

    #[test]
    fn narrow_screens_shrink_the_list() {
        let image = draw(None, &view(2, 1), (400, 800), 1.0);
        assert_eq!(image.logical_size.0, 400 - 2 * SCREEN_MARGIN);
    }

    #[test]
    fn long_lists_are_cut_to_the_screen() {
        let image = draw(None, &view(100, 50), (1920, 500), 1.0);
        assert!(image.logical_size.1 <= 500 - 2 * SCREEN_MARGIN);
    }

    #[test]
    fn corners_are_transparent_and_middle_is_opaque() {
        let image = draw(None, &view(2, 0), (1920, 1080), 1.0);
        assert_eq!(alpha(&image, 0, 0), 0);
        assert!(alpha(&image, image.width / 2, image.height / 2) > 200);
    }

    #[test]
    fn highlight_follows_selection() {
        // The highlight brightens the row it is on.
        let brightness = |selected| {
            let image = draw(None, &view(2, selected), (1920, 1080), 1.0);
            let y = PADDING + ROW_HEIGHT / 2;
            let i = ((y * image.width + image.width / 2) * 4) as usize;
            image.pixels[i]
        };
        assert!(brightness(0) > brightness(1));
    }

    #[test]
    fn desktop_name_is_read_from_the_entry_section() {
        let text = "[Desktop Entry]\nType=Application\nName=Ghostty\n\n[Desktop Action new]\nName=New Window\n";
        assert_eq!(parse_desktop_name(text).as_deref(), Some("Ghostty"));
        assert_eq!(parse_desktop_name("[Other]\nName=X\n"), None);
    }

    #[test]
    fn fallback_name_uses_last_app_id_segment() {
        assert_eq!(fallback_name("com.mitchellh.ghostty"), "Ghostty");
        assert_eq!(fallback_name("jetbrains-idea"), "Jetbrains-idea");
        assert_eq!(fallback_name(""), "");
    }
}
