//! The Alt+Tab switcher: a row of live window thumbnails, most recently
//! used first, drawn in the middle of the screen while Alt is held. Each
//! thumbnail has the application's icon in its lower right corner and the
//! application's name under it; the highlighted window's title is shown
//! below the row.
//!
//! Stepping only moves the highlight; the highlighted window is raised and
//! focused when Alt is released. A quick Alt+Tab finishes before
//! [`SHOW_DELAY`] passes, so it toggles between the two most recent windows
//! without the switcher flashing up.

use std::{collections::HashMap, fs, process::Command, time::Duration};

use fontdue::{Font, FontSettings};
use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                render_elements,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                utils::{CropRenderElement, RescaleRenderElement},
            },
            gles::GlesRenderer,
        },
    },
    output::Output,
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Point, Rectangle, Size, Transform},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};
use tracing::warn;

use crate::{
    apps::{self, AppInfo},
    focus::WindowId,
    state::State,
};

/// How long Alt must be held after the first Tab before the switcher
/// appears.
const SHOW_DELAY: Duration = Duration::from_millis(150);

// Sizes in logical pixels.
const SCREEN_MARGIN: i32 = 32;
const PADDING: i32 = 12;
const TILE_PADDING: i32 = 12;
const THUMB_WIDTH: i32 = 208;
const THUMB_HEIGHT: i32 = 130;
const NAME_HEIGHT: i32 = 32;
const TILE_WIDTH: i32 = THUMB_WIDTH + 2 * TILE_PADDING;
const TILE_HEIGHT: i32 = TILE_PADDING + THUMB_HEIGHT + NAME_HEIGHT;
const TITLE_HEIGHT: i32 = 36;
const ICON_SIZE: i32 = 48;
/// How far the icon reaches past the thumbnail's lower right corner.
const ICON_OVERHANG: i32 = 8;
const NAME_SIZE: f32 = 13.0;
const TITLE_SIZE: f32 = 15.0;
pub(crate) const RADIUS: f32 = 16.0;

// Colours as straight (not premultiplied) RGBA.
pub(crate) const BACKGROUND: [f32; 4] = [0.12, 0.12, 0.13, 0.94];
pub(crate) const HIGHLIGHT: [f32; 4] = [1.0, 1.0, 1.0, 0.14];
pub(crate) const TEXT_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];
const NAME_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 0.75];
const PLACEHOLDER: [f32; 4] = [0.35, 0.37, 0.42, 1.0];

render_elements! {
    /// Everything the switcher draws.
    pub SwitcherElement<=GlesRenderer>;
    Image=MemoryRenderBufferRenderElement<GlesRenderer>,
    Thumbnail=CropRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
}

/// One window in the switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: WindowId,
    pub app: AppInfo,
    /// The window title, which tells windows of the same application apart.
    pub title: String,
}

/// What the switcher shows: the windows and the highlighted one.
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
    /// The images last drawn. Reusing them while nothing changed keeps the
    /// renderer from redrawing them every frame. Thumbnails are not cached;
    /// they show the windows as they are now.
    cache: Option<(CacheKey, Images)>,
    apps: HashMap<String, AppInfo>,
    /// Icons by name and pixel size; `None` when the icon was not found.
    icons: HashMap<(String, u32), Option<Pixmap>>,
}

#[derive(PartialEq)]
struct CacheKey {
    view: View,
    screen: (i32, i32),
    scale: u64,
}

/// The panel drawn under the thumbnails, and the icons drawn over them.
struct Images {
    below: MemoryRenderBuffer,
    above: MemoryRenderBuffer,
}

impl Switcher {
    pub fn hide(&mut self) {
        self.visible = false;
        self.session += 1;
        self.cache = None;
    }

    fn app(&mut self, app_id: &str) -> AppInfo {
        self.apps
            .entry(app_id.to_string())
            .or_insert_with(|| apps::app_info(app_id))
            .clone()
    }

    fn icon(&mut self, name: &str, size: u32) -> Option<&Pixmap> {
        self.icons
            .entry((name.to_string(), size))
            .or_insert_with(|| apps::load_icon(name, size))
            .as_ref()
    }

    /// Draws the panel and icons for `view`, or reuses the last drawing.
    fn images(&mut self, view: &View, panel: &Panel, screen: (i32, i32), scale: f64) -> &Images {
        let key = CacheKey {
            view: view.clone(),
            screen,
            scale: scale.to_bits(),
        };
        let stale = self.cache.as_ref().is_none_or(|(cached, _)| *cached != key);
        if stale {
            let s = (panel.width as f64 * scale).round() / panel.width as f64;
            let icon_px = (ICON_SIZE as f64 * s).round() as u32;
            // Load the icons first; the font is borrowed while drawing.
            for entry in &view.entries[panel.first..panel.first + panel.count] {
                if let Some(icon) = &entry.app.icon {
                    self.icon(icon, icon_px);
                }
            }
            let icons: Vec<Option<&Pixmap>> = view.entries[panel.first..panel.first + panel.count]
                .iter()
                .map(|entry| {
                    let icon = entry.app.icon.as_ref()?;
                    self.icons.get(&(icon.clone(), icon_px))?.as_ref()
                })
                .collect();
            let font = self.font.get_or_insert_with(load_font).as_ref();
            let (below, above) = draw(font, view, panel, &icons, scale);
            let buffer = |image: Image| {
                MemoryRenderBuffer::from_slice(
                    &image.pixels,
                    Fourcc::Argb8888,
                    (image.width, image.height),
                    1,
                    Transform::Normal,
                    None,
                )
            };
            let images = Images {
                below: buffer(below),
                above: buffer(above),
            };
            self.cache = Some((key, images));
        }
        &self.cache.as_ref().expect("cache was just filled").1
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
            // A second Tab shows the switcher at once.
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

    /// The windows to show while switching.
    fn switcher_view(&mut self) -> Option<View> {
        let (ids, selected) = self.focus.switch_list()?;
        let details: Vec<(WindowId, String, String)> = ids
            .iter()
            .filter_map(|&id| {
                let surface = self.window(id)?.wl_surface()?;
                let (app_id, title) = with_states(surface, |states| {
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
                });
                Some((id, app_id, title))
            })
            .collect();
        // Every id in the history has a window, so indices still match.
        if details.len() != ids.len() {
            return None;
        }

        let entries = details
            .into_iter()
            .map(|(id, app_id, title)| Entry {
                id,
                app: self.switcher.app(&app_id),
                title,
            })
            .collect();
        Some(View { entries, selected })
    }

    /// The switcher for `output`, front to back, ready to draw on top of
    /// everything else. Empty while it is hidden.
    pub fn switcher_elements(&mut self, output: &Output) -> Vec<SwitcherElement> {
        if !self.switcher.visible {
            return Vec::new();
        }
        let Some(view) = self.switcher_view() else {
            return Vec::new();
        };
        let Some(geometry) = self.space.output_geometry(output) else {
            return Vec::new();
        };
        let screen = (geometry.size.w, geometry.size.h);
        let scale = output.current_scale().fractional_scale();
        let panel = Panel::new(view.entries.len(), view.selected, screen);
        // Whole physical pixels, so the images are not blurred.
        let origin = Point::<i32, Logical>::from((
            (screen.0 - panel.width) / 2,
            (screen.1 - panel.height) / 2,
        ))
        .to_f64()
        .to_physical(scale)
        .to_i32_round::<i32>();

        let images = self.switcher.images(&view, &panel, screen, scale);
        let renderer = self.backend.renderer();
        let image_element = |renderer: &mut GlesRenderer, buffer: &MemoryRenderBuffer| {
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                origin.to_f64(),
                buffer,
                None,
                Some(image_src((panel.width, panel.height).into(), scale)),
                Some((panel.width, panel.height).into()),
                Kind::Unspecified,
            )
            .map_err(|err| warn!(?err, "failed to draw window switcher"))
            .ok()
        };
        let Some(above) = image_element(renderer, &images.above) else {
            return Vec::new();
        };
        let Some(below) = image_element(renderer, &images.below) else {
            return Vec::new();
        };

        let mut elements = vec![SwitcherElement::Image(above)];
        for slot in 0..panel.count {
            let id = view.entries[panel.first + slot].id;
            let Some(window) = self.windows.iter().find(|w| w.id == id) else {
                continue;
            };
            let Some(surface) = window.wl_surface() else {
                continue;
            };
            // The window's own area, without client-side shadows.
            let window_geo = window.window.geometry();
            let Some(thumb) = fit(panel.thumb_box(slot), window_geo.size) else {
                continue;
            };
            let factor = thumb.size.w / window_geo.size.w as f64;
            let thumb_origin = (origin.to_f64() + thumb.loc.to_physical(scale)).to_i32_round();
            let surface_location =
                thumb_origin - window_geo.loc.to_f64().to_physical(scale).to_i32_round();
            let crop = Rectangle::new(thumb_origin, thumb.size.to_physical(scale).to_i32_round());

            let surface_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    surface,
                    surface_location,
                    scale,
                    1.0,
                    Kind::Unspecified,
                );
            elements.extend(surface_elements.into_iter().filter_map(|element| {
                let element = RescaleRenderElement::from_element(element, thumb_origin, factor);
                CropRenderElement::from_element(element, scale, crop)
                    .map(SwitcherElement::Thumbnail)
            }));
        }
        elements.push(SwitcherElement::Image(below));
        elements
    }
}

/// Where the switcher's parts go, in logical pixels relative to its top
/// left corner.
#[derive(Debug, PartialEq, Eq)]
struct Panel {
    width: i32,
    height: i32,
    /// The first window shown; long lists scroll to keep the highlight in
    /// view.
    first: usize,
    /// How many windows are shown.
    count: usize,
    selected: usize,
}

impl Panel {
    fn new(windows: usize, selected: usize, screen: (i32, i32)) -> Self {
        let max_tiles = ((screen.0 - 2 * SCREEN_MARGIN - 2 * PADDING) / TILE_WIDTH).max(1) as usize;
        let count = windows.min(max_tiles);
        let first = selected
            .saturating_sub(count / 2)
            .min(windows.saturating_sub(count));
        Self {
            width: count as i32 * TILE_WIDTH + 2 * PADDING,
            height: PADDING + TILE_HEIGHT + TITLE_HEIGHT + PADDING,
            first,
            count,
            selected,
        }
    }

    fn tile(&self, slot: usize) -> Rectangle<i32, Logical> {
        Rectangle::new(
            (PADDING + slot as i32 * TILE_WIDTH, PADDING).into(),
            (TILE_WIDTH, TILE_HEIGHT).into(),
        )
    }

    fn thumb_box(&self, slot: usize) -> Rectangle<i32, Logical> {
        let tile = self.tile(slot);
        Rectangle::new(
            tile.loc + Point::from((TILE_PADDING, TILE_PADDING)),
            (THUMB_WIDTH, THUMB_HEIGHT).into(),
        )
    }

    fn icon(&self, slot: usize) -> Rectangle<i32, Logical> {
        let thumb = self.thumb_box(slot);
        let corner =
            thumb.loc + thumb.size.to_point() + Point::from((ICON_OVERHANG, ICON_OVERHANG));
        Rectangle::new(
            corner - Point::from((ICON_SIZE, ICON_SIZE)),
            (ICON_SIZE, ICON_SIZE).into(),
        )
    }
}

/// A window of `size` scaled down to fit `area`, keeping its shape, and
/// centred in it. Small windows are not enlarged.
fn fit(area: Rectangle<i32, Logical>, size: Size<i32, Logical>) -> Option<Rectangle<f64, Logical>> {
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    let factor = (area.size.w as f64 / size.w as f64)
        .min(area.size.h as f64 / size.h as f64)
        .min(1.0);
    let scaled = Size::from((size.w as f64 * factor, size.h as f64 * factor));
    let loc = area.loc.to_f64()
        + Point::from((
            (area.size.w as f64 - scaled.w) / 2.0,
            (area.size.h as f64 - scaled.h) / 2.0,
        ));
    Some(Rectangle::new(loc, scaled))
}

/// An ARGB8888 image in physical pixels.
pub(crate) struct Image {
    pub(crate) width: i32,
    pub(crate) height: i32,
    /// Premultiplied, in memory order B, G, R, A.
    pub(crate) pixels: Vec<u8>,
}

/// The source rectangle for an image drawn at `scale` and shown at `size`:
/// the whole image. Without it, Smithay takes the logical size as the source
/// in buffer pixels, which crops the right and bottom of an image drawn at a
/// scale above 1.
pub(crate) fn image_src(size: Size<i32, Logical>, scale: f64) -> Rectangle<f64, Logical> {
    let pixels = |length: i32| (length as f64 * scale).round();
    Rectangle::from_size((pixels(size.w), pixels(size.h)).into())
}

/// Draws the panel (background, highlight and text) and, separately, the
/// icons that go on top of the thumbnails. `icons` has one entry per shown
/// window.
fn draw(
    font: Option<&Font>,
    view: &View,
    panel: &Panel,
    icons: &[Option<&Pixmap>],
    scale: f64,
) -> (Image, Image) {
    let width = (panel.width as f64 * scale).round() as i32;
    let height = (panel.height as f64 * scale).round() as i32;
    let mut below = Canvas::new(width, height);
    let mut above = Canvas::new(width, height);
    // Scale from logical to image pixels, exact for this image's size.
    let s = width as f32 / panel.width as f32;
    let rect = |r: Rectangle<i32, Logical>| {
        (
            r.loc.x as f32 * s,
            r.loc.y as f32 * s,
            r.size.w as f32 * s,
            r.size.h as f32 * s,
        )
    };

    below.rounded_rect(
        (0.0, 0.0, width as f32, height as f32),
        RADIUS * s,
        BACKGROUND,
    );

    for slot in 0..panel.count {
        let entry = &view.entries[panel.first + slot];
        if panel.first + slot == panel.selected {
            below.rounded_rect(rect(panel.tile(slot)), (RADIUS - 4.0) * s, HIGHLIGHT);
        }

        let icon = rect(panel.icon(slot));
        match icons.get(slot).copied().flatten() {
            Some(pixmap) => above.pixmap(pixmap, icon.0.round() as i32, icon.1.round() as i32),
            None => {
                above.rounded_rect(icon, 8.0 * s, PLACEHOLDER);
                if let Some(font) = font {
                    let letter: String = entry.app.name.chars().take(1).collect();
                    let style = TextStyle {
                        size: ICON_SIZE as f32 * 0.5 * s,
                        color: TEXT_COLOR,
                    };
                    above.centred_text(
                        font,
                        &letter,
                        style,
                        icon.0 + icon.2 / 2.0,
                        icon.2,
                        icon.1 + icon.3 / 2.0,
                    );
                }
            }
        }

        let Some(font) = font else { continue };
        let tile = rect(panel.tile(slot));
        let thumb = rect(panel.thumb_box(slot));
        let style = TextStyle {
            size: NAME_SIZE * s,
            color: NAME_COLOR,
        };
        // Leave room for the icon's overhang above the name.
        let name_middle = thumb.1 + thumb.3 + (ICON_OVERHANG as f32 + NAME_HEIGHT as f32) * s / 2.0;
        below.centred_text(
            font,
            &entry.app.name,
            style,
            tile.0 + tile.2 / 2.0,
            tile.2 - 2.0 * TILE_PADDING as f32 * s,
            name_middle,
        );
    }

    if let (Some(font), Some(entry)) = (font, view.entries.get(panel.selected)) {
        let style = TextStyle {
            size: TITLE_SIZE * s,
            color: TEXT_COLOR,
        };
        let middle = (PADDING + TILE_HEIGHT) as f32 * s + TITLE_HEIGHT as f32 * s / 2.0;
        below.centred_text(
            font,
            &entry.title,
            style,
            width as f32 / 2.0,
            width as f32 - 4.0 * PADDING as f32 * s,
            middle,
        );
    }

    (below.into_image(), above.into_image())
}

struct TextStyle {
    size: f32,
    color: [f32; 4],
}

pub(crate) struct Canvas {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

impl Canvas {
    pub(crate) fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    pub(crate) fn into_image(self) -> Image {
        Image {
            width: self.width,
            height: self.height,
            pixels: self.pixels,
        }
    }

    /// Blends a premultiplied B, G, R, A pixel over the one at (x, y).
    fn blend_premultiplied(&mut self, x: i32, y: i32, src: [f32; 4]) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height || src[3] <= 0.0 {
            return;
        }
        let i = ((y * self.width + x) * 4) as usize;
        let keep = 1.0 - src[3].min(1.0);
        for (dst, src) in self.pixels[i..i + 4].iter_mut().zip(src) {
            let blended = src * 255.0 + *dst as f32 * keep;
            *dst = blended.round().clamp(0.0, 255.0) as u8;
        }
    }

    /// Blends `color` over the pixel at (x, y) with `coverage` from 0 to 1.
    fn blend(&mut self, x: i32, y: i32, color: [f32; 4], coverage: f32) {
        let alpha = color[3] * coverage.clamp(0.0, 1.0);
        self.blend_premultiplied(
            x,
            y,
            [color[2] * alpha, color[1] * alpha, color[0] * alpha, alpha],
        );
    }

    /// Fills a rectangle (x, y, width, height) with rounded, anti-aliased
    /// corners.
    pub(crate) fn rounded_rect(
        &mut self,
        (x, y, w, h): (f32, f32, f32, f32),
        radius: f32,
        color: [f32; 4],
    ) {
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

    /// Draws `pixmap` (premultiplied RGBA) with its top left at (x, y).
    pub(crate) fn pixmap(&mut self, pixmap: &Pixmap, x: i32, y: i32) {
        let width = pixmap.width() as i32;
        for (i, pixel) in pixmap.pixels().iter().enumerate() {
            let px = x + i as i32 % width;
            let py = y + i as i32 / width;
            let channel = |c: u8| c as f32 / 255.0;
            self.blend_premultiplied(
                px,
                py,
                [
                    channel(pixel.blue()),
                    channel(pixel.green()),
                    channel(pixel.red()),
                    channel(pixel.alpha()),
                ],
            );
        }
    }

    /// Draws one line of text centred on (`centre`, `middle`), cut short
    /// with an ellipsis if it is wider than `max_width`.
    fn centred_text(
        &mut self,
        font: &Font,
        text: &str,
        style: TextStyle,
        centre: f32,
        max_width: f32,
        middle: f32,
    ) {
        let TextStyle { size, color } = style;
        let text = fit_text(font, text, size, max_width);
        let Some(line) = font.horizontal_line_metrics(size) else {
            return;
        };
        let baseline = (middle + (line.ascent + line.descent) / 2.0).round() as i32;

        let mut pen = (centre - text_width(font, &text, size) / 2.0).round();
        for c in text.chars() {
            let (metrics, coverage) = font.rasterize(c, size);
            let x0 = pen.round() as i32 + metrics.xmin;
            let y0 = baseline - metrics.height as i32 - metrics.ymin;
            for (i, &value) in coverage.iter().enumerate() {
                let x = x0 + (i % metrics.width.max(1)) as i32;
                let y = y0 + (i / metrics.width.max(1)) as i32;
                self.blend(x, y, color, value as f32 / 255.0);
            }
            pen += metrics.advance_width;
        }
    }
}

fn text_width(font: &Font, text: &str, size: f32) -> f32 {
    text.chars()
        .map(|c| font.metrics(c, size).advance_width)
        .sum()
}

/// `text` shortened with a trailing ellipsis so it fits in `max_width`.
fn fit_text(font: &Font, text: &str, size: f32, max_width: f32) -> String {
    if text_width(font, text, size) <= max_width {
        return text.to_string();
    }
    let ellipsis = text_width(font, "…", size);
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
        warn!("no font found (is fontconfig installed?); the window switcher shows no text");
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
                err, "cannot load font; the window switcher shows no text"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(n: usize, selected: usize) -> View {
        View {
            entries: (0..n)
                .map(|i| Entry {
                    id: WindowId(i as u64),
                    app: AppInfo {
                        name: "App".into(),
                        icon: None,
                    },
                    title: format!("Window {i}"),
                })
                .collect(),
            selected,
        }
    }

    fn pixel(image: &Image, x: i32, y: i32) -> [u8; 4] {
        let i = ((y * image.width + x) * 4) as usize;
        image.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn panel_fits_all_windows_on_a_wide_screen() {
        let panel = Panel::new(3, 1, (1920, 1080));
        assert_eq!(panel.count, 3);
        assert_eq!(panel.first, 0);
        assert_eq!(panel.width, 3 * TILE_WIDTH + 2 * PADDING);
        assert_eq!(panel.height, PADDING + TILE_HEIGHT + TITLE_HEIGHT + PADDING);
    }

    #[test]
    fn long_lists_scroll_to_keep_the_highlight_in_view() {
        let panel = Panel::new(20, 15, (1000, 800));
        assert_eq!(panel.count, 3);
        assert!(panel.width <= 1000 - 2 * SCREEN_MARGIN);
        assert!((panel.first..panel.first + panel.count).contains(&15));

        // Near the end the row stops at the last window.
        let panel = Panel::new(20, 19, (1000, 800));
        assert_eq!(panel.first + panel.count, 20);

        // A screen too narrow for even one tile still shows one.
        assert_eq!(Panel::new(5, 0, (100, 800)).count, 1);
    }

    #[test]
    fn icon_sits_on_the_thumbnails_lower_right_corner() {
        let panel = Panel::new(2, 0, (1920, 1080));
        let thumb = panel.thumb_box(1);
        let icon = panel.icon(1);
        assert_eq!(
            icon.loc + icon.size.to_point(),
            thumb.loc + thumb.size.to_point() + Point::from((ICON_OVERHANG, ICON_OVERHANG))
        );
    }

    #[test]
    fn thumbnails_keep_the_window_shape() {
        let area = Rectangle::new((0, 0).into(), (208, 130).into());
        // A 16:10 screen-sized window fills the box.
        let thumb = fit(area, (2048, 1280).into()).unwrap();
        assert_eq!(thumb.size, Size::from((208.0, 130.0)));
        // A tall window is centred horizontally.
        let thumb = fit(area, (650, 1300).into()).unwrap();
        assert_eq!(thumb.size, Size::from((65.0, 130.0)));
        assert_eq!(thumb.loc, Point::from((71.5, 0.0)));
        // Small windows are not enlarged.
        let thumb = fit(area, (100, 50).into()).unwrap();
        assert_eq!(thumb.size, Size::from((100.0, 50.0)));
        assert_eq!(fit(area, (0, 50).into()), None);
    }

    #[test]
    fn image_src_covers_the_whole_image() {
        let panel = Panel::new(3, 0, (1920, 1080));
        let view = view(3, 0);
        let (below, _) = draw(None, &view, &panel, &[None, None, None], 1.25);
        let src = image_src((panel.width, panel.height).into(), 1.25);
        assert_eq!(
            (src.size.w as i32, src.size.h as i32),
            (below.width, below.height)
        );
    }

    #[test]
    fn images_follow_panel_size_and_scale() {
        let panel = Panel::new(3, 1, (1920, 1080));
        let (below, above) = draw(None, &view(3, 1), &panel, &[], 1.25);
        assert_eq!(below.width, (panel.width as f64 * 1.25).round() as i32);
        assert_eq!(below.height, (panel.height as f64 * 1.25).round() as i32);
        assert_eq!((above.width, above.height), (below.width, below.height));
    }

    #[test]
    fn corners_are_transparent_and_middle_is_opaque() {
        let panel = Panel::new(2, 0, (1920, 1080));
        let (below, _) = draw(None, &view(2, 0), &panel, &[], 1.0);
        assert_eq!(pixel(&below, 0, 0)[3], 0);
        assert!(pixel(&below, below.width / 2, below.height / 2)[3] > 200);
    }

    #[test]
    fn highlight_follows_selection() {
        let brightness = |selected| {
            let panel = Panel::new(2, selected, (1920, 1080));
            let (below, _) = draw(None, &view(2, selected), &panel, &[], 1.0);
            // Inside the first tile, next to its thumbnail.
            pixel(&below, PADDING + 4, PADDING + 4)[0]
        };
        assert!(brightness(0) > brightness(1));
    }

    #[test]
    fn icons_are_drawn_above_and_placeholders_without_them() {
        let panel = Panel::new(2, 0, (1920, 1080));
        let mut red = Pixmap::new(ICON_SIZE as u32, ICON_SIZE as u32).unwrap();
        red.fill(resvg::tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let (_, above) = draw(None, &view(2, 0), &panel, &[Some(&red), None], 1.0);

        let centre = |slot| {
            let icon = panel.icon(slot);
            pixel(
                &above,
                icon.loc.x + ICON_SIZE / 2,
                icon.loc.y + ICON_SIZE / 2,
            )
        };
        assert_eq!(centre(0), [0, 0, 255, 255]);
        // The placeholder is an opaque grey square.
        let placeholder = centre(1);
        assert_eq!(placeholder[3], 255);
        assert!(placeholder[0] > placeholder[2]);
        // Nothing else is drawn above the thumbnails.
        assert_eq!(pixel(&above, above.width / 2, 2)[3], 0);
    }
}
