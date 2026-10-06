//! The Alt+Tab switcher: a grid of live window thumbnails, most recently
//! used first, reading left to right and then top to bottom, drawn in the
//! middle of the screen while Alt is held. Each thumbnail has the
//! application's icon in its lower right corner and the application's name
//! under it; the highlighted window's title is shown below the grid. A
//! window group is one screen-shaped thumbnail with its members in their
//! tiles, the members' app names under it and their titles below the grid.
//!
//! When more windows are open than fit across the screen, they wrap onto
//! more rows, as evenly filled as possible, with the last row centred.
//! When the rows that fit on the screen are not enough, the thumbnails
//! shrink, down to [`MIN_THUMB_SCALE`] of their size. Only past that does
//! the grid scroll by whole rows to keep the highlight in view, with an
//! arrow and a count such as "+4" at either end for the windows hidden
//! before and after the shown ones. Clicking an arrow or turning the mouse
//! wheel over the switcher moves the selection.
//!
//! Tab, Left and Right step through the windows, onto the next row at the
//! end of one; Up and Down move a row up or down.
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
                solid::SolidColorRenderElement,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                utils::{CropRenderElement, RescaleRenderElement},
            },
            gles::GlesRenderer,
        },
    },
    desktop::PopupManager,
    output::Output,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size, Transform},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};
use tracing::warn;

use crate::{
    apps::{self, AppInfo},
    focus::WindowId,
    group::GroupId,
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
#[cfg(test)]
const TILE_WIDTH: i32 = THUMB_WIDTH + 2 * TILE_PADDING;
#[cfg(test)]
const TILE_HEIGHT: i32 = TILE_PADDING + THUMB_HEIGHT + NAME_HEIGHT;
/// The smallest the thumbnails shrink to, as a share of their full size,
/// before the grid scrolls instead.
const MIN_THUMB_SCALE: f64 = 0.6;
/// The sizes tried, from full size down to the smallest.
const THUMB_SCALES: [f64; 5] = [1.0, 0.9, 0.8, 0.7, MIN_THUMB_SCALE];
const TITLE_HEIGHT: i32 = 36;
const ICON_SIZE: i32 = 48;
/// How far the icon reaches past the thumbnail's lower right corner.
const ICON_OVERHANG: i32 = 8;
/// The room at each end of a row that does not show every window, for the
/// arrow and count of the windows hidden beyond that end.
const EDGE_WIDTH: i32 = 32;
const ARROW_WIDTH: f32 = 9.0;
const ARROW_HEIGHT: f32 = 16.0;
/// How far the mouse wheel or touchpad scrolls, in logical pixels, to move
/// the selection by one window: one wheel notch.
pub(crate) const SCROLL_STEP: f64 = 15.0;
const NAME_SIZE: f32 = 13.0;
const TITLE_SIZE: f32 = 15.0;
pub(crate) const RADIUS: f32 = 16.0;

// Colours as straight (not premultiplied) RGBA.
pub(crate) const BACKGROUND: [f32; 4] = [0.12, 0.12, 0.13, 0.94];
pub(crate) const HIGHLIGHT: [f32; 4] = [1.0, 1.0, 1.0, 0.14];
/// The selection while another thumbnail is hovered: what releasing Alt
/// still picks.
const SELECTION_MARK: [f32; 4] = [1.0, 1.0, 1.0, 0.06];
pub(crate) const TEXT_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];
pub(crate) const NAME_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 0.75];
pub(crate) const PLACEHOLDER: [f32; 4] = [0.35, 0.37, 0.42, 1.0];
/// Behind a group's members, where the screen shows through between them.
const GROUP_BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 0.6];
/// The arrows and counts of hidden windows.
const EDGE_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 0.6];

render_elements! {
    /// Everything the switcher draws.
    pub SwitcherElement<=GlesRenderer>;
    Image=MemoryRenderBufferRenderElement<GlesRenderer>,
    Thumbnail=CropRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
    /// A surface drawn as it is, such as the panel sliding in.
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    /// A flat colour, such as the fade to black over the whole screen.
    Solid=SolidColorRenderElement,
}

/// One window or group in the switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The window, or a group's members from front to back.
    pub windows: Vec<WindowId>,
    /// The window's application; `None` for a group, which has no icon.
    pub app: Option<AppInfo>,
    /// The application's name, or a group's application names.
    pub name: String,
    /// The window title, which tells windows of the same application apart, or a group's titles.
    pub title: String,
}

/// What the switcher shows: the windows, the selected one and the one
/// under the pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub entries: Vec<Entry>,
    /// The entry Alt+Tab and the arrow keys moved to, which releasing Alt
    /// picks.
    pub selected: usize,
    /// The entry under the pointer, which a click picks.
    pub hovered: Option<usize>,
}

impl View {
    /// The entry drawn highlighted, with its title under the row: the one
    /// under the pointer, or else the selected one.
    fn highlighted(&self) -> usize {
        self.hovered.unwrap_or(self.selected)
    }
}

#[derive(Default)]
pub struct Switcher {
    /// Loaded on first use; the inner `None` means no font was found.
    font: Option<Option<Font>>,
    visible: bool,
    /// Whether the pointer moved since the switcher showed or the keyboard
    /// last moved the selection, so the thumbnail under it is highlighted.
    /// Which thumbnail that is gets worked out from where the pointer is
    /// each time, since the row shifts when a window closes. Hover only
    /// moves the highlight; the selection stays where the keyboard put it.
    hover: bool,
    /// Bumped whenever the switcher is hidden, so a pending show timer from
    /// an earlier switch does nothing.
    session: u64,
    /// Wheel or touchpad scrolling not yet turned into a step, in logical
    /// pixels, so a touchpad's small movements add up.
    scroll: f64,
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
    /// The layout, which also changes with the screen size and with
    /// `switcher.wrap` when the config is reloaded.
    panel: Panel,
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
        self.hover = false;
        self.session += 1;
        self.scroll = 0.0;
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
    fn images(&mut self, view: &View, panel: &Panel, scale: f64) -> &Images {
        let key = CacheKey {
            view: view.clone(),
            panel: panel.clone(),
            scale: scale.to_bits(),
        };
        let stale = self.cache.as_ref().is_none_or(|(cached, _)| *cached != key);
        if stale {
            let s = (panel.width as f64 * scale).round() / panel.width as f64;
            let icon_px = (ICON_SIZE as f64 * s).round() as u32;
            // Load the icons first; the font is borrowed while drawing.
            for entry in &view.entries[panel.first..panel.first + panel.count] {
                if let Some(icon) = entry.app.as_ref().and_then(|app| app.icon.as_ref()) {
                    self.icon(icon, icon_px);
                }
            }
            let icons: Vec<Option<&Pixmap>> = view.entries[panel.first..panel.first + panel.count]
                .iter()
                .map(|entry| {
                    let icon = entry.app.as_ref()?.icon.as_ref()?;
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
        // Windows share an entry when they are in one group, or are a window
        // and the modal dialog it waits on.
        let entry_of: HashMap<WindowId, (Option<GroupId>, WindowId)> = self
            .focus
            .order()
            .iter()
            .map(|&id| {
                let root = self.modal_root_of(id);
                (id, (self.groups.group_of(root), root))
            })
            .collect();
        let entry = |id: WindowId| {
            entry_of
                .get(&id)
                .map(|&(group, root)| (group, group.is_none().then_some(root)))
        };
        if self.focus.step(forward, entry).is_none() {
            return;
        }
        // The keyboard moved the selection; highlight it again rather than
        // the thumbnail under the pointer.
        self.switcher.hover = false;
        self.switcher.scroll = 0.0;
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

    /// A click while the switcher shows picks the window under the pointer
    /// and ends the switch, as releasing Alt would. Returns whether the click
    /// was the switcher's: any click on its panel is, so a click between
    /// thumbnails does not reach the window behind.
    pub fn switcher_click(&mut self, pos: Point<f64, Logical>) -> bool {
        // Nothing is drawn without a view, so there is nothing to click.
        if self.switcher_view().is_none() {
            return false;
        }
        match self.switcher_hit(pos) {
            Hit::Outside => false,
            Hit::Panel => true,
            Hit::More(index) => {
                self.switcher_select(index);
                true
            }
            Hit::Entry(index) => {
                self.end_pointer_grab();
                self.focus.select(index);
                self.focus.finish_switch();
                self.switcher.hide();
                self.sync_focus();
                // The picked window is now under the pointer.
                self.refresh_pointer_focus();
                true
            }
        }
    }

    /// Wheel or touchpad scrolling by `delta` logical pixels at `pos` moves
    /// the selection, one window per [`SCROLL_STEP`], forwards for down or
    /// right. Returns whether the scroll was the switcher's: any over its
    /// panel is, so the window behind does not scroll.
    ///
    /// A scroll of nothing, such as a touchpad's stop when the fingers
    /// lift, is left for the window with pointer focus, which may have seen
    /// the start of the scroll before the switcher showed and waits for its
    /// end to begin kinetic scrolling.
    pub fn switcher_scroll(&mut self, pos: Point<f64, Logical>, delta: f64) -> bool {
        if delta == 0.0 || self.switcher_hit(pos) == Hit::Outside {
            return false;
        }
        // Nothing is drawn without a view, so there is nothing to scroll.
        if self.switcher_view().is_none() {
            return false;
        }
        let Some((len, selected)) = self.focus.switch_list().map(|(ids, i)| (ids.len(), i)) else {
            return false;
        };
        // Turning back starts afresh, so the first notch the other way
        // moves at once.
        if self.switcher.scroll * delta < 0.0 {
            self.switcher.scroll = 0.0;
        }
        self.switcher.scroll += delta;
        let steps = (self.switcher.scroll / SCROLL_STEP).trunc();
        self.switcher.scroll -= steps * SCROLL_STEP;
        if steps != 0.0 {
            // Stop at the ends rather than wrapping round as Tab does, so
            // scrolling past the last window does not jump to the first.
            let index = (selected as f64 + steps).clamp(0.0, (len - 1) as f64);
            self.switcher_select(index as usize);
        }
        true
    }

    /// Moves the Alt+Tab selection a row down or up the grid (Down and Up),
    /// as laid out on the output under the pointer, where clicks are tested
    /// too. Stops at the first and last rows; on the row above a shorter
    /// last row, Down goes to the last window when nothing is right below.
    /// With one row, whether it shows every window or scrolls, and before
    /// the switcher shows, it steps through the windows as Left and Right
    /// do.
    pub fn switch_row(&mut self, down: bool) {
        let Some((len, selected)) = self.focus.switch_list().map(|(ids, i)| (ids.len(), i)) else {
            self.step_switch(down);
            return;
        };
        let pointer = self.seat.get_pointer().map(|p| p.current_location());
        let screen = pointer
            .and_then(|pos| self.space.output_under(pos).next().cloned())
            .or_else(|| self.primary_output())
            .and_then(|output| self.space.output_geometry(&output))
            .map(|geometry| (geometry.size.w, geometry.size.h));
        let panel = match screen {
            Some(screen) if self.switcher.visible => {
                Panel::new(len, selected, screen, self.config.switcher.wrap)
            }
            _ => {
                self.step_switch(down);
                return;
            }
        };
        if panel.rows() <= 1 {
            self.step_switch(down);
            return;
        }
        if let Some(index) = row_target(len, selected, panel.columns, down) {
            self.switcher_select(index);
        }
    }

    /// Selects entry `index` without ending the switch; the grid scrolls to
    /// show it.
    fn switcher_select(&mut self, index: usize) {
        self.focus.select(index);
        // As when the keyboard moves the selection, highlight it rather
        // than the thumbnail that is now under the pointer.
        self.switcher.hover = false;
        self.switcher.scroll = 0.0;
    }

    /// From now on the thumbnail under the pointer is highlighted, or the
    /// selected one again when the pointer is off the thumbnails.
    pub fn switcher_pointer_moved(&mut self) {
        if self.switcher.visible {
            self.switcher.hover = true;
        }
    }

    /// What of the switcher is at `pos`; `Outside` while it is hidden.
    fn switcher_hit(&self, pos: Point<f64, Logical>) -> Hit {
        if !self.switcher.visible {
            return Hit::Outside;
        }
        let Some((len, selected)) = self.focus.switch_list().map(|(ids, i)| (ids.len(), i)) else {
            return Hit::Outside;
        };
        let Some(geometry) = self
            .space
            .output_under(pos)
            .next()
            .and_then(|output| self.space.output_geometry(output))
        else {
            return Hit::Outside;
        };
        let screen = (geometry.size.w, geometry.size.h);
        let panel = Panel::new(len, selected, screen, self.config.switcher.wrap);
        let origin = geometry.loc + panel.origin();
        panel.hit(pos - origin.to_f64())
    }

    /// Ends a pointer grab before the switcher changes focus, closing the
    /// focused window's popup menus. A menu's grab would otherwise keep the
    /// keyboard from the picked window, and the menu would stay open on the
    /// window left behind.
    fn end_pointer_grab(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if !pointer.is_grabbed() {
            return;
        }
        let focused = self.focus.current().and_then(|id| self.window(id));
        if let Some(surface) = focused.and_then(|w| w.wl_surface()) {
            let popups: Vec<_> = PopupManager::popups_for_surface(surface).collect();
            for (popup, _) in popups {
                // Only a dead surface fails, and it has no menu to close.
                let _ = PopupManager::dismiss_popup(surface, &popup);
            }
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.unset_grab(self);
        }
        let time = self.start_time.elapsed().as_millis() as u32;
        pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
    }

    /// The entry under the pointer, once the pointer has moved; `None`
    /// before that or when the pointer is off the thumbnails.
    fn hovered_entry(&self) -> Option<usize> {
        if !self.switcher.hover {
            return None;
        }
        let pos = self.seat.get_pointer()?.current_location();
        match self.switcher_hit(pos) {
            Hit::Entry(index) => Some(index),
            Hit::Outside | Hit::Panel | Hit::More(_) => None,
        }
    }

    /// The windows and groups to show while switching.
    fn switcher_view(&mut self) -> Option<View> {
        let (ids, selected) = self.focus.switch_list()?;
        let ids = ids.to_vec();
        let hovered = self.hovered_entry();
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            let windows = match self.groups.group_of(id) {
                Some(group) => {
                    let members = self.groups.members(group);
                    self.focus
                        .order()
                        .iter()
                        .copied()
                        .filter(|member| members.contains(member))
                        .collect()
                }
                None => vec![id],
            };
            let mut apps = Vec::new();
            let mut titles = Vec::new();
            for &window in &windows {
                let surface = self.window(window)?.wl_surface()?;
                let (app_id, title) = app_id_and_title(surface);
                apps.push(self.switcher.app(&app_id));
                titles.push(title);
            }
            let entry = if let [app] = apps.as_slice() {
                Entry {
                    windows,
                    name: app.name.clone(),
                    app: Some(app.clone()),
                    title: titles.concat(),
                }
            } else {
                let mut names: Vec<String> = Vec::new();
                for app in apps {
                    if !names.contains(&app.name) {
                        names.push(app.name);
                    }
                }
                Entry {
                    windows,
                    app: None,
                    name: names.join(", "),
                    title: titles.join(" · "),
                }
            };
            entries.push(entry);
        }
        Some(View {
            entries,
            selected,
            hovered,
        })
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
        let panel = Panel::new(
            view.entries.len(),
            view.selected,
            screen,
            self.config.switcher.wrap,
        );
        let output_loc = geometry.loc;
        // Whole physical pixels, so the images are not blurred.
        let origin = panel
            .origin()
            .to_f64()
            .to_physical(scale)
            .to_i32_round::<i32>();

        let images = self.switcher.images(&view, &panel, scale);
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
            let entry = &view.entries[panel.first + slot];
            // A window fills its thumbnail; a group's members sit where they are on the screen, drawn to scale.
            let (area, factor) = match entry.windows.as_slice() {
                [id] => {
                    let Some(window) = self.windows.iter().find(|w| w.id == *id) else {
                        continue;
                    };
                    // The window's own area, without client-side shadows.
                    let size = window.window.geometry().size;
                    let Some(thumb) = fit(panel.thumb_box(slot), size) else {
                        continue;
                    };
                    (thumb, thumb.size.w / size.w as f64)
                }
                _ => {
                    let Some(thumb) = panel.screen_box(slot) else {
                        continue;
                    };
                    (thumb, thumb.size.w / screen.0 as f64)
                }
            };
            let crop = Rectangle::new(
                (origin.to_f64() + area.loc.to_physical(scale)).to_i32_round(),
                area.size.to_physical(scale).to_i32_round(),
            );
            for id in &entry.windows {
                let Some(window) = self.windows.iter().find(|w| w.id == *id) else {
                    continue;
                };
                let Some(surface) = window.wl_surface() else {
                    continue;
                };
                let window_geo = window.window.geometry();
                let offset = match entry.windows.len() {
                    1 => Point::default(),
                    _ => match self.space.element_location(&window.window) {
                        Some(loc) => (loc - output_loc).to_f64().upscale(factor),
                        None => continue,
                    },
                };
                let thumb_origin =
                    (origin.to_f64() + (area.loc + offset).to_physical(scale)).to_i32_round();
                let surface_location =
                    thumb_origin - window_geo.loc.to_f64().to_physical(scale).to_i32_round();
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
        }
        elements.push(SwitcherElement::Image(below));
        elements
    }
}

/// A toplevel's app id and title, empty when it has not set them.
pub(crate) fn app_id_and_title(surface: &WlSurface) -> (String, String) {
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
}

/// Where the switcher's parts go, in logical pixels relative to its top
/// left corner.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Panel {
    width: i32,
    height: i32,
    /// The first window shown; long lists scroll by whole rows to keep the
    /// highlight in view.
    first: usize,
    /// How many windows are shown.
    count: usize,
    /// How many windows there are, shown or not.
    windows: usize,
    /// Tiles per row.
    columns: usize,
    /// The size of a thumbnail's box, smaller than [`THUMB_WIDTH`] by
    /// [`THUMB_HEIGHT`] when many windows are open.
    thumb: (i32, i32),
    /// The room for the arrow at each end, between the padding and the
    /// tiles: none when every window is shown.
    edge_width: i32,
    /// The screen's logical size, the shape of a group's thumbnail.
    screen: (i32, i32),
}

impl Panel {
    /// Lays `windows` out on `screen` around the selected one: in rows if
    /// `wrap` (`switcher.wrap` in the config), or else in one row of
    /// full-size thumbnails that scrolls.
    fn new(windows: usize, selected: usize, screen: (i32, i32), wrap: bool) -> Self {
        let room_width = screen.0 - 2 * SCREEN_MARGIN - 2 * PADDING;
        let room_height = screen.1 - 2 * SCREEN_MARGIN - 2 * PADDING - TITLE_HEIGHT;
        let max_rows = if wrap { usize::MAX } else { 1 };
        // How many tiles of a thumbnail size fit across and down.
        let fits = |thumb: (i32, i32), room_width: i32| {
            let (tile_width, tile_height) = tile_size(thumb);
            (
                (room_width / tile_width).max(1) as usize,
                ((room_height / tile_height).max(1) as usize).min(max_rows),
            )
        };
        let scales: &[f64] = if wrap { &THUMB_SCALES } else { &[1.0] };
        // The biggest thumbnails that let every window fit.
        let fitting = scales.iter().find_map(|&scale| {
            let thumb = (
                (THUMB_WIDTH as f64 * scale).round() as i32,
                (THUMB_HEIGHT as f64 * scale).round() as i32,
            );
            let (columns, rows) = fits(thumb, room_width);
            (columns * rows >= windows).then_some((thumb, columns))
        });
        let (thumb, columns, first, count, edge_width) = match fitting {
            Some((thumb, columns)) => {
                // As few rows as needed, filled as evenly as they can be.
                let rows = windows.div_ceil(columns).max(1);
                let columns = windows.div_ceil(rows).max(1);
                (thumb, columns, 0, windows, 0)
            }
            None => {
                let smallest = scales.last().copied().unwrap_or(1.0);
                let thumb = (
                    (THUMB_WIDTH as f64 * smallest).round() as i32,
                    (THUMB_HEIGHT as f64 * smallest).round() as i32,
                );
                // The grid does not show every window, so it needs room at
                // its ends for the arrows, which can leave space for one
                // column fewer.
                let (columns, rows) = fits(thumb, room_width - 2 * EDGE_WIDTH);
                let first = if rows == 1 {
                    // A single row scrolls a window at a time, keeping the
                    // selection in the middle.
                    selected
                        .saturating_sub(columns / 2)
                        .min(windows.saturating_sub(columns))
                } else {
                    // Rows scroll whole, so windows keep their columns.
                    let total_rows = windows.div_ceil(columns);
                    let first_row = (selected / columns)
                        .saturating_sub(rows / 2)
                        .min(total_rows.saturating_sub(rows));
                    first_row * columns
                };
                let count = (columns * rows).min(windows - first);
                // On a screen too narrow for one tile and the arrows, the
                // arrows get what is left beside the tile rather than
                // widening the panel.
                let tile_width = tile_size(thumb).0;
                let edge_width =
                    ((room_width - columns as i32 * tile_width) / 2).clamp(0, EDGE_WIDTH);
                (thumb, columns, first, count, edge_width)
            }
        };
        let (tile_width, tile_height) = tile_size(thumb);
        let rows = count.div_ceil(columns).max(1) as i32;
        Self {
            width: columns as i32 * tile_width + 2 * (PADDING + edge_width),
            height: PADDING + rows * tile_height + TITLE_HEIGHT + PADDING,
            first,
            count,
            windows,
            columns,
            thumb,
            edge_width,
            screen,
        }
    }

    /// How many rows are shown.
    fn rows(&self) -> usize {
        self.count.div_ceil(self.columns).max(1)
    }

    /// Where the arrows go from the top, level with the middle of the
    /// shown thumbnails.
    fn arrow_middle(&self) -> f32 {
        let tile_height = tile_size(self.thumb).1 as f32;
        (PADDING + TILE_PADDING) as f32
            + self.thumb.1 as f32 / 2.0
            + (self.rows() - 1) as f32 * tile_height / 2.0
    }

    /// Where the title goes from the top: under the grid.
    fn title_middle(&self) -> i32 {
        PADDING + self.rows() as i32 * tile_size(self.thumb).1 + TITLE_HEIGHT / 2
    }

    /// How many windows are hidden beyond `side` of the row.
    fn hidden(&self, side: Side) -> usize {
        match side {
            Side::Left => self.first,
            Side::Right => self.windows - self.first - self.count,
        }
    }

    /// The end of the panel beyond the tiles on `side`, from its top to its
    /// bottom, and the index of the nearest window hidden beyond it; `None`
    /// when no window is.
    fn edge(&self, side: Side) -> Option<(Rectangle<i32, Logical>, usize)> {
        if self.hidden(side) == 0 {
            return None;
        }
        let width = PADDING + self.edge_width;
        let (x, nearest) = match side {
            Side::Left => (0, self.first - 1),
            Side::Right => (self.width - width, self.first + self.count),
        };
        Some((
            Rectangle::new((x, 0).into(), (width, self.height).into()),
            nearest,
        ))
    }

    /// The tile of the shown window in `slot`. A last row that is not full
    /// is centred when every window shows; in a grid that scrolls it stays
    /// in its columns, so windows keep their place as rows scroll.
    fn tile(&self, slot: usize) -> Rectangle<i32, Logical> {
        let (tile_width, tile_height) = tile_size(self.thumb);
        let (row, column) = (slot / self.columns, slot % self.columns);
        let in_row = self.columns.min(self.count - row * self.columns) as i32;
        let indent = if self.count == self.windows {
            (self.columns as i32 - in_row) * tile_width / 2
        } else {
            0
        };
        Rectangle::new(
            (
                PADDING + self.edge_width + indent + column as i32 * tile_width,
                PADDING + row as i32 * tile_height,
            )
                .into(),
            (tile_width, tile_height).into(),
        )
    }

    /// The panel's top left corner relative to the screen's: centred on it.
    fn origin(&self) -> Point<i32, Logical> {
        Point::from((
            (self.screen.0 - self.width) / 2,
            (self.screen.1 - self.height) / 2,
        ))
    }

    /// What is at `pos`, relative to the panel's top left corner.
    fn hit(&self, pos: Point<f64, Logical>) -> Hit {
        let inside = |r: Rectangle<i32, Logical>| r.to_f64().contains(pos);
        if !inside(Rectangle::from_size((self.width, self.height).into())) {
            return Hit::Outside;
        }
        if let Some(slot) = (0..self.count).find(|&slot| inside(self.tile(slot))) {
            return Hit::Entry(self.first + slot);
        }
        [Side::Left, Side::Right]
            .into_iter()
            .filter_map(|side| self.edge(side))
            .find(|&(area, _)| inside(area))
            .map_or(Hit::Panel, |(_, nearest)| Hit::More(nearest))
    }

    fn thumb_box(&self, slot: usize) -> Rectangle<i32, Logical> {
        let tile = self.tile(slot);
        Rectangle::new(
            tile.loc + Point::from((TILE_PADDING, TILE_PADDING)),
            self.thumb.into(),
        )
    }

    /// Where a group's thumbnail, the screen scaled down, goes.
    fn screen_box(&self, slot: usize) -> Option<Rectangle<f64, Logical>> {
        fit(self.thumb_box(slot), self.screen.into())
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

/// The window a row down or up from `selected` in a grid of `len` windows
/// and `columns` columns; None on the first or last row.
fn row_target(len: usize, selected: usize, columns: usize, down: bool) -> Option<usize> {
    if !down {
        return selected.checked_sub(columns);
    }
    if selected + columns < len {
        Some(selected + columns)
    } else if selected / columns < (len - 1) / columns {
        // Nothing right below on a shorter last row.
        Some(len - 1)
    } else {
        None
    }
}

/// The size of a tile around a thumbnail box of `thumb`, with its padding
/// and the name under it.
fn tile_size(thumb: (i32, i32)) -> (i32, i32) {
    (
        thumb.0 + 2 * TILE_PADDING,
        TILE_PADDING + thumb.1 + NAME_HEIGHT,
    )
}

/// Where on the switcher a click landed.
#[derive(Debug, PartialEq, Eq)]
enum Hit {
    Outside,
    /// On the panel, between the tiles.
    Panel,
    /// On the tile of the entry with this index.
    Entry(usize),
    /// On an arrow towards hidden windows; the index is the nearest of them.
    More(usize),
}

/// An end of the switcher's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// A window of `size` scaled down to fit `area`, keeping its shape, and
/// centred in it. Small windows are not enlarged.
pub(crate) fn fit(
    area: Rectangle<i32, Logical>,
    size: Size<i32, Logical>,
) -> Option<Rectangle<f64, Logical>> {
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
        let index = panel.first + slot;
        let fill = if index == view.highlighted() {
            Some(HIGHLIGHT)
        } else if index == view.selected {
            Some(SELECTION_MARK)
        } else {
            None
        };
        if let Some(fill) = fill {
            below.rounded_rect(rect(panel.tile(slot)), (RADIUS - 4.0) * s, fill);
        }

        let icon = rect(panel.icon(slot));
        match (&entry.app, icons.get(slot).copied().flatten()) {
            (None, _) => {
                if let Some(screen) = panel.screen_box(slot) {
                    let screen = (
                        screen.loc.x as f32 * s,
                        screen.loc.y as f32 * s,
                        screen.size.w as f32 * s,
                        screen.size.h as f32 * s,
                    );
                    below.rounded_rect(screen, 4.0 * s, GROUP_BACKGROUND);
                }
            }
            (Some(_), Some(pixmap)) => {
                above.pixmap(pixmap, icon.0.round() as i32, icon.1.round() as i32)
            }
            (Some(app), None) => {
                above.rounded_rect(icon, 8.0 * s, PLACEHOLDER);
                if let Some(font) = font {
                    let letter: String = app.name.chars().take(1).collect();
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
            &entry.name,
            style,
            tile.0 + tile.2 / 2.0,
            tile.2 - 2.0 * TILE_PADDING as f32 * s,
            name_middle,
        );
    }

    for side in [Side::Left, Side::Right] {
        let Some((area, _)) = panel.edge(side) else {
            continue;
        };
        // The arrow and count sit in the room beside the tiles, not in the
        // panel's padding.
        let edge_width = panel.edge_width as f32 * s;
        let centre = match side {
            Side::Left => PADDING as f32 * s + edge_width / 2.0,
            Side::Right => area.loc.x as f32 * s + edge_width / 2.0,
        };
        let middle = panel.arrow_middle() * s;
        let (w, h) = (ARROW_WIDTH * s, ARROW_HEIGHT * s);
        let (back, tip) = match side {
            Side::Left => (centre + w / 2.0, centre - w / 2.0),
            Side::Right => (centre - w / 2.0, centre + w / 2.0),
        };
        below.convex_polygon(
            &[
                (back, middle - h / 2.0),
                (tip, middle),
                (back, middle + h / 2.0),
            ],
            EDGE_COLOR,
        );
        let Some(font) = font else { continue };
        let style = TextStyle {
            size: NAME_SIZE * s,
            color: EDGE_COLOR,
        };
        // Under the arrow, as a name is under its thumbnail.
        let name_middle =
            middle + (panel.thumb.1 as f32 / 2.0 + (ICON_OVERHANG + NAME_HEIGHT) as f32 / 2.0) * s;
        below.centred_text(
            font,
            &format!("+{}", panel.hidden(side)),
            style,
            centre,
            edge_width,
            name_middle,
        );
    }

    if let (Some(font), Some(entry)) = (font, view.entries.get(view.highlighted())) {
        let style = TextStyle {
            size: TITLE_SIZE * s,
            color: TEXT_COLOR,
        };
        let middle = panel.title_middle() as f32 * s;
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

pub(crate) struct TextStyle {
    pub(crate) size: f32,
    pub(crate) color: [f32; 4],
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

    /// Fills a convex polygon, anti-aliased. `points` may go round it in
    /// either direction.
    pub(crate) fn convex_polygon(&mut self, points: &[(f32, f32)], color: [f32; 4]) {
        if points.len() < 3 {
            return;
        }
        let edges: Vec<((f32, f32), (f32, f32))> = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .map(|(&a, &b)| (a, b))
            .collect();
        // Twice the signed area: which side of each edge is inside.
        let winding: f32 = edges.iter().map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum();
        // Points in a line enclose nothing.
        if winding == 0.0 {
            return;
        }
        let inside = winding.signum();
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in points {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }
        for py in top.floor() as i32..bottom.ceil() as i32 {
            for px in left.floor() as i32..right.ceil() as i32 {
                let cx = px as f32 + 0.5;
                let cy = py as f32 + 0.5;
                // Distance from the pixel centre to the shape's edge,
                // negative inside: for a convex shape, the largest of the
                // distances past each edge.
                let distance = edges
                    .iter()
                    .map(|&((ax, ay), (bx, by))| {
                        let length = (bx - ax).hypot(by - ay).max(f32::EPSILON);
                        let cross = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
                        -inside * cross / length
                    })
                    .fold(f32::MIN, f32::max);
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

    /// Fills the whole canvas with `color`, replacing what was there.
    pub(crate) fn fill(&mut self, color: [f32; 4]) {
        let alpha = color[3];
        let channel = |c: f32| (c * alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        let pixel = [
            channel(color[2]),
            channel(color[1]),
            channel(color[0]),
            channel(1.0),
        ];
        for dst in self.pixels.as_chunks_mut::<4>().0 {
            dst.copy_from_slice(&pixel);
        }
    }

    /// Draws one line of text centred on (`centre`, `middle`), cut short
    /// with an ellipsis if it is wider than `max_width`.
    pub(crate) fn centred_text(
        &mut self,
        font: &Font,
        text: &str,
        style: TextStyle,
        centre: f32,
        max_width: f32,
        middle: f32,
    ) {
        let text = fit_text(font, text, style.size, max_width);
        let left = centre - text_width(font, &text, style.size) / 2.0;
        self.text(font, &text, style, left, middle);
    }

    /// Draws one line of text starting at `left`, vertically centred on
    /// `middle`, cut short with an ellipsis if it is wider than `max_width`.
    /// Returns the width drawn.
    pub(crate) fn left_text(
        &mut self,
        font: &Font,
        text: &str,
        style: TextStyle,
        left: f32,
        max_width: f32,
        middle: f32,
    ) -> f32 {
        let text = fit_text(font, text, style.size, max_width);
        let width = text_width(font, &text, style.size);
        self.text(font, &text, style, left, middle);
        width
    }

    fn text(&mut self, font: &Font, text: &str, style: TextStyle, left: f32, middle: f32) {
        let TextStyle { size, color } = style;
        let Some(line) = font.horizontal_line_metrics(size) else {
            return;
        };
        let baseline = (middle + (line.ascent + line.descent) / 2.0).round() as i32;

        let mut pen = left.round();
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

pub(crate) fn text_width(font: &Font, text: &str, size: f32) -> f32 {
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
pub(crate) fn load_font() -> Option<Font> {
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
                    windows: vec![WindowId(i as u64)],
                    app: Some(AppInfo {
                        name: "App".into(),
                        icon: None,
                    }),
                    name: "App".into(),
                    title: format!("Window {i}"),
                })
                .collect(),
            selected,
            hovered: None,
        }
    }

    fn pixel(image: &Image, x: i32, y: i32) -> [u8; 4] {
        let i = ((y * image.width + x) * 4) as usize;
        image.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn wrapping_fills_rows_evenly_and_centres_the_last() {
        let panel = Panel::new(9, 0, (1920, 1080), true);
        assert_eq!((panel.first, panel.count), (0, 9));
        assert_eq!((panel.columns, panel.rows()), (5, 2));
        assert_eq!(panel.thumb, (THUMB_WIDTH, THUMB_HEIGHT));
        assert_eq!(panel.tile(5).loc.y, panel.tile(0).loc.y + TILE_HEIGHT);
        // The four on the last row are shifted half a tile in.
        assert_eq!(panel.tile(5).loc.x, panel.tile(0).loc.x + TILE_WIDTH / 2);
        assert_eq!(panel.edge(Side::Left), None);
        assert_eq!(
            panel.height,
            PADDING + 2 * TILE_HEIGHT + TITLE_HEIGHT + PADDING
        );
    }

    #[test]
    fn wrapping_shrinks_the_thumbnails_before_scrolling() {
        let panel = Panel::new(20, 15, (1000, 800), true);
        assert_eq!((panel.first, panel.count), (0, 20));
        assert!(panel.thumb.0 < THUMB_WIDTH);
        assert!(panel.thumb.0 >= (THUMB_WIDTH as f64 * MIN_THUMB_SCALE) as i32);
        assert_eq!(panel.edge(Side::Right), None);
    }

    #[test]
    fn wrapping_scrolls_whole_rows_past_the_smallest_size() {
        let panel = Panel::new(200, 120, (1000, 800), true);
        assert!(panel.count < 200);
        assert_eq!(panel.first % panel.columns, 0);
        assert!((panel.first..panel.first + panel.count).contains(&120));
        assert!(panel.edge(Side::Left).is_some() && panel.edge(Side::Right).is_some());
        assert_eq!(
            panel.hidden(Side::Left) + panel.count + panel.hidden(Side::Right),
            200
        );
        // The last rows end at the last window.
        let panel = Panel::new(200, 199, (1000, 800), true);
        assert_eq!(panel.first + panel.count, 200);
    }

    #[test]
    fn wrapped_grid_stays_on_the_screen() {
        for screen in [
            (1000, 800),
            (1280, 720),
            (1920, 1080),
            (2560, 1440),
            (600, 500),
        ] {
            for windows in 1..120 {
                let panel = Panel::new(windows, windows / 2, screen, true);
                let fits = panel.width <= screen.0 - 2 * SCREEN_MARGIN
                    && panel.height <= screen.1 - 2 * SCREEN_MARGIN;
                assert!(fits, "{windows} windows on {screen:?}: {panel:?}");
                for slot in 0..panel.count {
                    let tile = panel.tile(slot);
                    assert!(tile.loc.x + tile.size.w <= panel.width - PADDING);
                    assert!(tile.loc.y + tile.size.h <= panel.height - PADDING - TITLE_HEIGHT);
                }
            }
        }
    }

    #[test]
    fn rows_step_by_columns_and_stop_at_the_ends() {
        // 5 + 4 windows.
        assert_eq!(row_target(9, 1, 5, true), Some(6));
        assert_eq!(row_target(9, 4, 5, true), Some(8));
        assert_eq!(row_target(9, 6, 5, true), None);
        assert_eq!(row_target(9, 6, 5, false), Some(1));
        assert_eq!(row_target(9, 2, 5, false), None);
    }

    #[test]
    fn a_scrolling_grid_keeps_its_last_row_in_its_columns() {
        let panel = Panel::new(203, 202, (1000, 800), true);
        assert!(panel.count < 203);
        let last = panel.count - 1;
        assert_eq!(
            panel.tile(last).loc.x,
            panel.tile(last - last % panel.columns).loc.x
                + (last % panel.columns) as i32 * panel.tile(0).size.w
        );
        assert_eq!(
            panel.tile(last - last % panel.columns).loc.x,
            panel.tile(0).loc.x
        );
    }

    #[test]
    fn hit_finds_tiles_on_later_rows() {
        let panel = Panel::new(9, 0, (1920, 1080), true);
        let tile = panel.tile(7).to_f64();
        let middle = tile.loc + tile.size.downscale(2.0).to_point();
        assert_eq!(panel.hit(middle), Hit::Entry(7));
    }

    #[test]
    fn panel_fits_all_windows_on_a_wide_screen() {
        let panel = Panel::new(3, 1, (1920, 1080), false);
        assert_eq!(panel.count, 3);
        assert_eq!(panel.first, 0);
        assert_eq!(panel.width, 3 * TILE_WIDTH + 2 * PADDING);
        assert_eq!(panel.height, PADDING + TILE_HEIGHT + TITLE_HEIGHT + PADDING);
    }

    #[test]
    fn hit_finds_the_tile_under_the_pointer() {
        let panel = Panel::new(3, 1, (1920, 1080), false);
        let middle = |slot: usize| {
            let tile = panel.tile(slot).to_f64();
            tile.loc + tile.size.downscale(2.0).to_point()
        };
        assert_eq!(panel.hit(middle(0)), Hit::Entry(0));
        assert_eq!(panel.hit(middle(2)), Hit::Entry(2));
        // The padding is the panel's, but no tile's.
        assert_eq!(panel.hit((2.0, 2.0).into()), Hit::Panel);
        assert_eq!(panel.hit((-1.0, 2.0).into()), Hit::Outside);
        assert_eq!(
            panel.hit((panel.width as f64 + 1.0, 2.0).into()),
            Hit::Outside
        );
    }

    #[test]
    fn hit_counts_from_the_first_shown_window() {
        let panel = Panel::new(20, 15, (1000, 800), false);
        assert!(panel.first > 0);
        let tile = panel.tile(0).to_f64();
        let middle = tile.loc + tile.size.downscale(2.0).to_point();
        assert_eq!(panel.hit(middle), Hit::Entry(panel.first));
    }

    #[test]
    fn long_lists_scroll_to_keep_the_highlight_in_view() {
        let panel = Panel::new(20, 15, (1000, 800), false);
        assert_eq!(panel.count, 3);
        assert!(panel.width <= 1000 - 2 * SCREEN_MARGIN);
        assert!((panel.first..panel.first + panel.count).contains(&15));

        // Near the end the row stops at the last window.
        let panel = Panel::new(20, 19, (1000, 800), false);
        assert_eq!(panel.first + panel.count, 20);

        // A screen too narrow for even one tile still shows one.
        assert_eq!(Panel::new(5, 0, (100, 800), false).count, 1);
    }

    #[test]
    fn a_row_that_shows_every_window_has_no_arrows() {
        let panel = Panel::new(3, 1, (1920, 1080), false);
        assert_eq!(panel.edge(Side::Left), None);
        assert_eq!(panel.edge(Side::Right), None);
        assert_eq!(panel.tile(0).loc.x, PADDING);
    }

    #[test]
    fn arrows_count_the_windows_hidden_beyond_each_end() {
        // At the start only the right end has windows beyond it.
        let panel = Panel::new(20, 0, (1000, 800), false);
        assert_eq!(panel.hidden(Side::Left), 0);
        assert_eq!(panel.hidden(Side::Right), 20 - panel.count);
        assert_eq!(panel.edge(Side::Left), None);
        assert!(panel.edge(Side::Right).is_some());

        // In the middle both ends do, and they add up to what is not shown.
        let panel = Panel::new(20, 10, (1000, 800), false);
        assert!(panel.hidden(Side::Left) > 0);
        assert!(panel.hidden(Side::Right) > 0);
        assert_eq!(
            panel.hidden(Side::Left) + panel.count + panel.hidden(Side::Right),
            20
        );

        // At the end only the left end does.
        let panel = Panel::new(20, 19, (1000, 800), false);
        assert_eq!(panel.hidden(Side::Right), 0);
        assert_eq!(panel.edge(Side::Right), None);
    }

    #[test]
    fn the_arrows_room_leaves_the_row_on_the_screen() {
        // Four tiles fit exactly, but not with the arrows beside them.
        let screen = 4 * TILE_WIDTH + 2 * (SCREEN_MARGIN + PADDING);
        assert_eq!(Panel::new(4, 0, (screen, 800), false).count, 4);
        let panel = Panel::new(5, 0, (screen, 800), false);
        assert_eq!(panel.count, 3);
        assert!(panel.width <= screen - 2 * SCREEN_MARGIN);
        // The tiles start after the left arrow's room.
        assert_eq!(panel.tile(0).loc.x, PADDING + EDGE_WIDTH);
        let last = panel.tile(panel.count - 1);
        assert_eq!(last.loc.x + last.size.w, panel.width - PADDING - EDGE_WIDTH);
    }

    #[test]
    fn arrows_do_not_widen_the_panel_past_a_narrow_screen() {
        for screen in 320..600 {
            let panel = Panel::new(20, 10, (screen, 800), false);
            assert!(
                panel.width <= screen - 2 * SCREEN_MARGIN,
                "screen {screen}: panel {}",
                panel.width
            );
        }
    }

    #[test]
    fn hit_on_an_arrow_names_the_nearest_hidden_window() {
        let panel = Panel::new(20, 10, (1000, 800), false);
        let middle = |side| {
            let (area, _) = panel.edge(side).unwrap();
            let area = area.to_f64();
            area.loc + area.size.downscale(2.0).to_point()
        };
        assert_eq!(panel.hit(middle(Side::Left)), Hit::More(panel.first - 1));
        assert_eq!(
            panel.hit(middle(Side::Right)),
            Hit::More(panel.first + panel.count)
        );
        // Without hidden windows the end is just the panel.
        let panel = Panel::new(20, 0, (1000, 800), false);
        assert_eq!(panel.hit((2.0, 50.0).into()), Hit::Panel);
    }

    #[test]
    fn arrows_are_drawn_only_towards_hidden_windows() {
        let panel = Panel::new(20, 0, (1000, 800), false);
        let (below, _) = draw(None, &view(20, 0), &panel, &[], 1.0);
        let arrow = |x: i32| pixel(&below, x, PADDING + TILE_PADDING + THUMB_HEIGHT / 2)[0];
        let left = PADDING + EDGE_WIDTH / 2;
        let right = panel.width - PADDING - EDGE_WIDTH / 2;
        // The panel's background is dark; the arrow is light.
        assert!(arrow(right) > arrow(left) + 50);
    }

    #[test]
    fn convex_polygon_fills_inside_either_way_round() {
        for points in [
            [(2.0, 2.0), (18.0, 2.0), (10.0, 18.0)],
            [(10.0, 18.0), (18.0, 2.0), (2.0, 2.0)],
        ] {
            let mut canvas = Canvas::new(20, 20);
            canvas.convex_polygon(&points, [1.0, 1.0, 1.0, 1.0]);
            let image = canvas.into_image();
            assert_eq!(pixel(&image, 10, 6), [255, 255, 255, 255]);
            assert_eq!(pixel(&image, 2, 16)[3], 0);
            assert_eq!(pixel(&image, 0, 0)[3], 0);
        }
    }

    #[test]
    fn icon_sits_on_the_thumbnails_lower_right_corner() {
        let panel = Panel::new(2, 0, (1920, 1080), false);
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
        let panel = Panel::new(3, 0, (1920, 1080), false);
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
        let panel = Panel::new(3, 1, (1920, 1080), false);
        let (below, above) = draw(None, &view(3, 1), &panel, &[], 1.25);
        assert_eq!(below.width, (panel.width as f64 * 1.25).round() as i32);
        assert_eq!(below.height, (panel.height as f64 * 1.25).round() as i32);
        assert_eq!((above.width, above.height), (below.width, below.height));
    }

    #[test]
    fn corners_are_transparent_and_middle_is_opaque() {
        let panel = Panel::new(2, 0, (1920, 1080), false);
        let (below, _) = draw(None, &view(2, 0), &panel, &[], 1.0);
        assert_eq!(pixel(&below, 0, 0)[3], 0);
        assert!(pixel(&below, below.width / 2, below.height / 2)[3] > 200);
    }

    #[test]
    fn highlight_follows_selection() {
        let brightness = |selected| {
            let panel = Panel::new(2, selected, (1920, 1080), false);
            let (below, _) = draw(None, &view(2, selected), &panel, &[], 1.0);
            // Inside the first tile, next to its thumbnail.
            pixel(&below, PADDING + 4, PADDING + 4)[0]
        };
        assert!(brightness(0) > brightness(1));
    }

    #[test]
    fn hover_highlights_instead_of_the_selection() {
        let panel = Panel::new(3, 0, (1920, 1080), false);
        let brightness = |hovered, slot: usize| {
            let view = View {
                hovered,
                ..view(3, 0)
            };
            let (below, _) = draw(None, &view, &panel, &[], 1.0);
            // Inside the tile, next to its thumbnail.
            pixel(&below, panel.tile(slot).loc.x + 4, PADDING + 4)[0]
        };
        assert_eq!(brightness(None, 0), brightness(Some(0), 0));
        // Hovering another tile highlights it fully and leaves a fainter
        // mark on the selection, above a plain tile.
        assert_eq!(brightness(Some(1), 1), brightness(None, 0));
        assert!(brightness(Some(1), 0) < brightness(Some(1), 1));
        assert!(brightness(Some(1), 0) > brightness(Some(1), 2));
    }

    #[test]
    fn icons_are_drawn_above_and_placeholders_without_them() {
        let panel = Panel::new(2, 0, (1920, 1080), false);
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

    #[test]
    fn a_group_gets_a_screen_shaped_backdrop_and_no_icon() {
        let panel = Panel::new(2, 1, (1600, 1000), false);
        let plain = view(2, 1);
        let mut view = plain.clone();
        view.entries[0] = Entry {
            windows: vec![WindowId(7), WindowId(8)],
            app: None,
            name: "Ghostty".into(),
            title: "htop · k9s".into(),
        };
        let (below, above) = draw(None, &view, &panel, &[None, None], 1.0);

        let screen = panel.screen_box(0).unwrap();
        assert_eq!(screen.size.w / screen.size.h, 1.6);
        let middle = screen.loc + screen.size.downscale(2.0).to_point();
        let (plain_below, _) = draw(None, &plain, &panel, &[None, None], 1.0);
        let at = |image: &Image| pixel(image, middle.x as i32, middle.y as i32)[0];
        assert!(at(&below) < at(&plain_below));
        let icon = panel.icon(0);
        assert_eq!(
            pixel(
                &above,
                icon.loc.x + ICON_SIZE / 2,
                icon.loc.y + ICON_SIZE / 2
            )[3],
            0
        );
    }
}
