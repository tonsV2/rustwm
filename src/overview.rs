//! The overview, opened with a Super tap and modelled on GNOME's: the panel
//! stays at the top, and under it are a search field and the open windows
//! as live previews in a grid. Typing searches the installed
//! applications and, through an outside command, files. The arrow keys or
//! the pointer pick a window or a result, Return or a click takes it, and
//! Escape or another Super tap closes the overview.
//!
//! While it is open no key reaches a window. The windows stay where they
//! are underneath; the overview is only a picture of them.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use fontdue::Font;
use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                utils::{CropRenderElement, RescaleRenderElement},
            },
            gles::GlesRenderer,
        },
    },
    desktop::layer_map_for_output,
    output::Output,
    reexports::calloop::{
        LoopHandle, RegistrationToken,
        channel::{self, Sender},
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point, Rectangle, Transform},
};
use tracing::{debug, info, warn};

use crate::{
    apps::{self, AppInfo, DesktopApp},
    commands,
    focus::WindowId,
    keyboard::Edit,
    layout::Direction,
    session::ChildEnv,
    state::State,
    switcher::{
        BACKGROUND, Canvas, HIGHLIGHT, Image, NAME_COLOR, PLACEHOLDER, SwitcherElement, TEXT_COLOR,
        TextStyle, app_id_and_title, fit, image_src, load_font, text_width,
    },
};

// Sizes in logical pixels.
const MARGIN: i32 = 48;
const SEARCH_TOP: i32 = 40;
const SEARCH_WIDTH: i32 = 640;
const SEARCH_HEIGHT: i32 = 48;
const SEARCH_PADDING: i32 = 20;
const SEARCH_SIZE: f32 = 17.0;
const CARET_WIDTH: i32 = 2;
const CARET_HEIGHT: i32 = 22;
/// How long the caret is on, then off.
const BLINK: Duration = Duration::from_millis(530);
/// Between the search field and the windows or results.
const CONTENT_GAP: i32 = 40;
const GRID_GAP: i32 = 28;
const NAME_HEIGHT: i32 = 36;
const NAME_SIZE: f32 = 14.0;
const MAX_THUMB_WIDTH: i32 = 560;
/// Around a window's thumbnail and name, inside its highlight.
const TILE_PADDING: i32 = 10;
const ICON_SIZE: i32 = 48;
/// How far the icon reaches past the thumbnail's lower right corner.
const ICON_OVERHANG: i32 = 8;
const ROW_HEIGHT: i32 = 56;
const RESULTS_WIDTH: i32 = 720;
const ROW_ICON: i32 = 32;
const ROW_PADDING: i32 = 16;
const ROW_SIZE: f32 = 16.0;
const DETAIL_SIZE: f32 = 13.0;
const KIND_SIZE: f32 = 9.0;
const MAX_APP_RESULTS: usize = 6;
const MAX_RESULTS: usize = 12;
const PLACEHOLDER_TEXT: &str = "Search applications and files";

// Colours as straight (not premultiplied) RGBA.
const BACKDROP: [f32; 4] = [0.10, 0.10, 0.11, 0.97];
const FIELD: [f32; 4] = [1.0, 1.0, 1.0, 0.10];
const HINT_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 0.45];
/// Behind a thumbnail that does not fill its box.
const THUMB_BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 0.5];

pub struct Overview {
    pub open: bool,
    query: String,
    /// Where typing goes, as a character index into the query.
    caret: usize,
    /// Whether the caret is in the on phase of its blink.
    caret_on: bool,
    blink_timer: Option<RegistrationToken>,
    /// The caret image for the current scale.
    caret_image: Option<(u64, MemoryRenderBuffer)>,
    /// The highlighted window or result.
    selected: usize,
    /// The installed applications, read when the overview opens.
    apps: Vec<DesktopApp>,
    /// The last file search's results.
    files: Vec<FileHit>,
    /// Numbers the file searches, so a slow one finishing after a newer
    /// one does not replace its results.
    search_seq: u64,
    sender: Sender<FileResults>,
    /// Loaded on first use; the inner `None` means no font was found.
    font: Option<Option<Font>>,
    app_infos: HashMap<String, AppInfo>,
    /// Icons by name and pixel size; `None` when the icon was not found.
    icons: HashMap<(String, u32), Option<Pixmap>>,
    /// What is on screen, for pointer hit tests.
    scene: Option<Scene>,
    /// The images last drawn, reused while the scene and scale are the
    /// same. Thumbnails are not cached; they show the windows as they are.
    cache: Option<(Scene, u64, Images)>,
}

/// A file the outside search command found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileHit {
    path: PathBuf,
    name: String,
    /// The folder it is in, with the home directory as `~`.
    folder: String,
    /// A short label for its icon: the extension, or "DIR".
    kind: String,
}

struct FileResults {
    seq: u64,
    files: Vec<FileHit>,
}

/// The backdrop, text and row icons drawn under the thumbnails, and the
/// window icons drawn over them.
struct Images {
    below: MemoryRenderBuffer,
    above: MemoryRenderBuffer,
}

/// Where everything is, in logical pixels relative to the output.
#[derive(Debug, Clone, PartialEq)]
struct Scene {
    /// The output's position, so pointer positions can be made relative.
    output: Point<i32, Logical>,
    /// The overview's area: the output minus the panel.
    area: Rectangle<i32, Logical>,
    query: String,
    search: Rectangle<i32, Logical>,
    /// Where the caret goes, from the left of the output.
    caret_x: i32,
    /// Where the windows or results go.
    content: Rectangle<i32, Logical>,
    items: Vec<Placed>,
    /// Columns in the window grid, for Up and Down; 1 for a results list.
    columns: usize,
    selected: usize,
    /// Shown in the middle when there are no items.
    empty: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
struct Placed {
    /// The whole tile, which the highlight and the pointer use.
    tile: Rectangle<i32, Logical>,
    item: Item,
}

#[derive(Debug, Clone, PartialEq)]
enum Item {
    Window {
        id: WindowId,
        /// The box the live preview is fitted into.
        thumb: Rectangle<i32, Logical>,
        app: AppInfo,
        title: String,
    },
    App {
        index: usize,
        name: String,
        detail: String,
        icon: Option<String>,
    },
    File {
        index: usize,
        name: String,
        folder: String,
        kind: String,
    },
}

impl Overview {
    pub fn new(loop_handle: &LoopHandle<'static, State>) -> anyhow::Result<Self> {
        let (sender, receiver) = channel::channel();
        loop_handle
            .insert_source(receiver, |event, _, state| {
                if let channel::Event::Msg(results) = event {
                    state.file_results(results);
                }
            })
            .map_err(|err| anyhow::anyhow!("failed to listen for file search results: {err}"))?;
        Ok(Self {
            open: false,
            query: String::new(),
            caret: 0,
            caret_on: true,
            blink_timer: None,
            caret_image: None,
            selected: 0,
            apps: Vec::new(),
            files: Vec::new(),
            search_seq: 0,
            sender,
            font: None,
            app_infos: HashMap::new(),
            icons: HashMap::new(),
            scene: None,
            cache: None,
        })
    }

    pub fn has_text(&self) -> bool {
        !self.query.is_empty()
    }

    fn app_info(&mut self, app_id: &str) -> AppInfo {
        self.app_infos
            .entry(app_id.to_string())
            .or_insert_with(|| apps::app_info(app_id))
            .clone()
    }

    fn load_icon(&mut self, name: &str, size: u32) {
        self.icons
            .entry((name.to_string(), size))
            .or_insert_with(|| apps::load_icon(name, size));
    }

    /// The applications matching the query, best first, then the files.
    fn results(&self) -> Vec<Item> {
        let query = self.query.trim().to_lowercase();
        let mut apps: Vec<(u8, usize)> = self
            .apps
            .iter()
            .enumerate()
            .filter_map(|(index, app)| app.score(&query).map(|score| (score, index)))
            .collect();
        // Equal scores keep the name order the list has.
        apps.sort_by_key(|&(score, index)| (score, index));
        apps.truncate(MAX_APP_RESULTS);
        let mut items: Vec<Item> = apps
            .into_iter()
            .map(|(_, index)| {
                let app = &self.apps[index];
                Item::App {
                    index,
                    name: app.name.clone(),
                    detail: app.detail.clone(),
                    icon: app.icon.clone(),
                }
            })
            .collect();
        for (index, file) in self.files.iter().enumerate() {
            if items.len() >= MAX_RESULTS {
                break;
            }
            items.push(Item::File {
                index,
                name: file.name.clone(),
                folder: file.folder.clone(),
                kind: file.kind.clone(),
            });
        }
        items
    }

    /// Draws the images for `scene`, or reuses the last drawing.
    fn images(&mut self, scene: &Scene, scale: f64) -> &Images {
        let stale = self.cache.as_ref().is_none_or(|(cached, cached_scale, _)| {
            cached != scene || *cached_scale != scale.to_bits()
        });
        if stale {
            let px = |size: i32| (size as f64 * scale).round() as u32;
            for placed in &scene.items {
                match &placed.item {
                    Item::Window { app, .. } => {
                        if let Some(icon) = &app.icon {
                            self.load_icon(icon, px(ICON_SIZE));
                        }
                    }
                    Item::App {
                        icon: Some(icon), ..
                    } => self.load_icon(icon, px(ROW_ICON)),
                    _ => {}
                }
            }
            let icons: Vec<Option<&Pixmap>> = scene
                .items
                .iter()
                .map(|placed| {
                    let (icon, size) = match &placed.item {
                        Item::Window { app, .. } => (app.icon.as_ref()?, px(ICON_SIZE)),
                        Item::App { icon, .. } => (icon.as_ref()?, px(ROW_ICON)),
                        Item::File { .. } => return None,
                    };
                    self.icons.get(&(icon.clone(), size))?.as_ref()
                })
                .collect();
            let font = self.font.get_or_insert_with(load_font).as_ref();
            let (below, above) = draw(font, scene, &icons, scale);
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
            self.cache = Some((scene.clone(), scale.to_bits(), images));
        }
        &self.cache.as_ref().expect("cache was just filled").2
    }

    /// The caret, a bar in the text colour, drawn once per scale.
    fn caret_image(&mut self, scale: f64) -> &MemoryRenderBuffer {
        if self
            .caret_image
            .as_ref()
            .is_none_or(|(cached, _)| *cached != scale.to_bits())
        {
            let width = (CARET_WIDTH as f64 * scale).round() as i32;
            let height = (CARET_HEIGHT as f64 * scale).round() as i32;
            let mut canvas = Canvas::new(width, height);
            canvas.fill(TEXT_COLOR);
            let image = canvas.into_image();
            let buffer = MemoryRenderBuffer::from_slice(
                &image.pixels,
                Fourcc::Argb8888,
                (image.width, image.height),
                1,
                Transform::Normal,
                None,
            );
            self.caret_image = Some((scale.to_bits(), buffer));
        }
        &self.caret_image.as_ref().expect("cache was just filled").1
    }

    /// The item under `pos`.
    fn item_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        let scene = self.scene.as_ref()?;
        let pos = pos - scene.output.to_f64();
        scene
            .items
            .iter()
            .position(|placed| placed.tile.to_f64().contains(pos))
    }
}

impl State {
    pub fn toggle_overview(&mut self) {
        if self.overview.open {
            self.close_overview();
        } else {
            self.open_overview();
        }
    }

    pub fn open_overview(&mut self) {
        if self.overview.open || self.is_locked() {
            return;
        }
        info!("overview opened");
        // An Alt+Tab switch in progress ends with nothing changed.
        self.focus.cancel_switch();
        self.switcher.hide();
        self.overview.open = true;
        self.overview.query.clear();
        self.overview.caret = 0;
        self.overview.selected = 0;
        self.overview.files.clear();
        self.overview.apps = apps::all_apps();
        self.overview.cache = None;
        self.restart_blink();
        self.overview_panel(true);
        // Windows get no keys while the overview is open.
        self.set_keyboard_focus(None);
        self.refresh_pointer_focus();
    }

    pub fn close_overview(&mut self) {
        if !self.overview.open {
            return;
        }
        info!("overview closed");
        self.overview.open = false;
        self.overview.query.clear();
        self.overview.caret = 0;
        self.overview.selected = 0;
        self.stop_blink();
        self.overview.files.clear();
        self.overview.search_seq += 1;
        self.overview.scene = None;
        self.overview.cache = None;
        self.overview_panel(false);
        self.sync_focus();
        self.refresh_pointer_focus();
    }

    pub fn overview_type(&mut self, c: char) {
        if !self.overview.open || c.is_control() {
            return;
        }
        let overview = &mut self.overview;
        let at = char_to_byte(&overview.query, overview.caret);
        overview.query.insert(at, c);
        overview.caret += 1;
        self.query_changed();
    }

    pub fn overview_edit(&mut self, edit: Edit) {
        if !self.overview.open {
            return;
        }
        let overview = &mut self.overview;
        if apply_edit(&mut overview.query, &mut overview.caret, edit) {
            self.query_changed();
        } else {
            self.restart_blink();
        }
    }

    fn query_changed(&mut self) {
        self.overview.selected = 0;
        self.restart_blink();
        self.search_files();
    }

    /// Shows the caret and starts its blinking over, so it is on right
    /// after typing.
    fn restart_blink(&mut self) {
        self.stop_blink();
        self.overview.caret_on = true;
        let timer = Timer::from_duration(BLINK);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.overview.caret_on = !state.overview.caret_on;
            TimeoutAction::ToDuration(BLINK)
        });
        match inserted {
            Ok(token) => self.overview.blink_timer = Some(token),
            Err(err) => warn!(%err, "failed to schedule the caret blinking"),
        }
    }

    fn stop_blink(&mut self) {
        if let Some(token) = self.overview.blink_timer.take() {
            self.loop_handle.remove(token);
        }
    }

    /// Runs the configured file search command with the query as `$1` on a
    /// thread, so a slow search never holds up drawing. Single characters
    /// match too much to be worth a search.
    fn search_files(&mut self) {
        self.overview.search_seq += 1;
        let seq = self.overview.search_seq;
        let query = self.overview.query.trim().to_string();
        let command = self.config.overview.file_search.clone();
        if query.chars().count() < 2 || command.is_empty() {
            self.overview.files.clear();
            return;
        }
        let sender = self.overview.sender.clone();
        let child_env = self.child_env();
        let spawned = thread::Builder::new()
            .name("file search".into())
            .spawn(move || {
                let files = run_file_search(&command, &query, &child_env);
                // Fails only when the compositor is shutting down.
                let _ = sender.send(FileResults { seq, files });
            });
        if let Err(err) = spawned {
            warn!(%err, "failed to start the file search");
        }
    }

    fn file_results(&mut self, results: FileResults) {
        if results.seq != self.overview.search_seq || !self.overview.open {
            return;
        }
        self.overview.files = results.files;
    }

    /// Moves the highlight: Left and Right step through the items, Up and
    /// Down move a row in the window grid.
    pub fn overview_move(&mut self, direction: Direction) {
        let Some(scene) = &self.overview.scene else {
            return;
        };
        let count = scene.items.len();
        if count == 0 {
            return;
        }
        let selected = scene.selected.min(count - 1);
        let columns = scene.columns.max(1);
        self.overview.selected = match direction {
            Direction::Left => (selected + count - 1) % count,
            Direction::Right => (selected + 1) % count,
            Direction::Up if columns == 1 => selected.saturating_sub(1),
            Direction::Down if columns == 1 => (selected + 1).min(count - 1),
            Direction::Up => selected.checked_sub(columns).unwrap_or(selected),
            Direction::Down => {
                let below = selected + columns;
                if below < count { below } else { selected }
            }
        };
    }

    /// Focuses the highlighted window, or starts the highlighted result.
    pub fn overview_activate(&mut self) {
        let item = self
            .overview
            .scene
            .as_ref()
            .and_then(|scene| scene.items.get(scene.selected))
            .map(|placed| placed.item.clone());
        if let Some(item) = item {
            self.activate_item(item);
        }
    }

    /// Highlights the item under the pointer.
    pub fn overview_pointer_moved(&mut self, pos: Point<f64, Logical>) {
        if !self.overview.open {
            return;
        }
        if let Some(index) = self.overview.item_at(pos) {
            self.overview.selected = index;
        }
    }

    /// A click in the overview takes the item under the pointer. Returns
    /// whether the click was the overview's; clicks on the panel are not.
    pub fn overview_click(&mut self, pos: Point<f64, Logical>) -> bool {
        if !self.overview.open || self.surface_under(pos).is_some() {
            return false;
        }
        let item = self
            .overview
            .item_at(pos)
            .and_then(|index| self.overview.scene.as_ref()?.items.get(index))
            .map(|placed| placed.item.clone());
        if let Some(item) = item {
            self.activate_item(item);
        }
        true
    }

    fn activate_item(&mut self, item: Item) {
        let child_env = self.child_env();
        self.close_overview();
        match item {
            Item::Window { id, .. } => {
                info!(window = id.0, "window picked in the overview");
                self.focus_window(id);
            }
            Item::App { index, name, .. } => {
                let Some(app) = self.overview.apps.get(index) else {
                    return;
                };
                info!(name, path = %app.path.display(), "starting from the overview");
                let command = format!("gio launch {}", quote(&app.path));
                commands::run(&command, &child_env, || {});
            }
            Item::File { index, .. } => {
                let Some(file) = self.overview.files.get(index) else {
                    return;
                };
                info!(path = %file.path.display(), "opening from the overview");
                let command = format!("xdg-open {}", quote(&file.path));
                commands::run(&command, &child_env, || {});
            }
        }
    }

    /// Lays the overview out for `output`: the date, the search field and
    /// either the windows, most recently used first, or the results.
    fn overview_scene(&mut self, output: &Output) -> Option<Scene> {
        let geometry = self.space.output_geometry(output)?;
        let area = layer_map_for_output(output).non_exclusive_zone();
        let screen = (geometry.size.w, geometry.size.h);
        let query = self.overview.query.clone();
        let (items, columns, empty) = if query.trim().is_empty() {
            let ids: Vec<WindowId> = self
                .focus
                .order()
                .iter()
                .copied()
                .filter(|id| self.window(*id).is_some_and(|w| w.mapped))
                .collect();
            let mut windows = Vec::with_capacity(ids.len());
            for id in ids {
                let Some(surface) = self.window(id).and_then(|w| w.wl_surface()) else {
                    continue;
                };
                let (app_id, title) = app_id_and_title(surface);
                let app = self.overview.app_info(&app_id);
                let title = if title.is_empty() {
                    app.name.clone()
                } else {
                    title
                };
                windows.push((id, app, title));
            }
            let content = content_area(area);
            let (boxes, columns) = grid(windows.len(), content, screen);
            let items = windows
                .into_iter()
                .zip(boxes)
                .map(|((id, app, title), thumb)| Placed {
                    tile: Rectangle::new(
                        thumb.loc - Point::from((TILE_PADDING, TILE_PADDING)),
                        (
                            thumb.size.w + 2 * TILE_PADDING,
                            thumb.size.h + NAME_HEIGHT + TILE_PADDING,
                        )
                            .into(),
                    ),
                    item: Item::Window {
                        id,
                        thumb,
                        app,
                        title,
                    },
                })
                .collect::<Vec<_>>();
            let empty = items.is_empty().then_some("No open windows");
            (items, columns, empty)
        } else {
            let content = content_area(area);
            let width = RESULTS_WIDTH.min(content.size.w);
            let left = area.loc.x + (area.size.w - width) / 2;
            let rows = (content.size.h / ROW_HEIGHT).max(0) as usize;
            let items: Vec<Placed> = self
                .overview
                .results()
                .into_iter()
                .take(rows)
                .enumerate()
                .map(|(row, item)| Placed {
                    tile: Rectangle::new(
                        (left, content.loc.y + row as i32 * ROW_HEIGHT).into(),
                        (width, ROW_HEIGHT).into(),
                    ),
                    item,
                })
                .collect();
            let empty = items.is_empty().then_some("No matches");
            (items, 1, empty)
        };
        let search_width = SEARCH_WIDTH.min(area.size.w - 2 * MARGIN).max(0);
        let search = Rectangle::new(
            (
                area.loc.x + (area.size.w - search_width) / 2,
                area.loc.y + SEARCH_TOP,
            )
                .into(),
            (search_width, SEARCH_HEIGHT).into(),
        );
        // The text is drawn at the output's scale, so the caret's place is
        // measured there too.
        let scale = output.current_scale().fractional_scale() as f32;
        let before: String = query.chars().take(self.overview.caret).collect();
        let typed = match self.overview.font.get_or_insert_with(load_font) {
            Some(font) => text_width(font, &before, SEARCH_SIZE * scale) / scale,
            None => 0.0,
        };
        let caret_x = (search.loc.x + SEARCH_PADDING + typed.round() as i32)
            .min(search.loc.x + search.size.w - SEARCH_PADDING);
        Some(Scene {
            output: geometry.loc,
            area,
            query,
            search,
            caret_x,
            content: content_area(area),
            selected: self.overview.selected.min(items.len().saturating_sub(1)),
            items,
            columns,
            empty,
        })
    }

    /// The overview for `output`, front to back, ready to draw over the
    /// windows. Empty while it is closed.
    pub fn overview_elements(&mut self, output: &Output) -> Vec<SwitcherElement> {
        if !self.overview.open {
            return Vec::new();
        }
        let Some(scene) = self.overview_scene(output) else {
            return Vec::new();
        };
        self.overview.selected = scene.selected;
        let scale = output.current_scale().fractional_scale();
        // Whole physical pixels, so the images are not blurred.
        let origin = scene
            .area
            .loc
            .to_f64()
            .to_physical(scale)
            .to_i32_round::<i32>();
        let images = self.overview.images(&scene, scale);
        let renderer = self.backend.renderer();
        let image_element = |renderer: &mut GlesRenderer, buffer: &MemoryRenderBuffer| {
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                origin.to_f64(),
                buffer,
                None,
                Some(image_src(scene.area.size, scale)),
                Some(scene.area.size),
                Kind::Unspecified,
            )
            .map_err(|err| warn!(?err, "failed to draw the overview"))
            .ok()
        };
        let Some(above) = image_element(renderer, &images.above) else {
            return Vec::new();
        };
        let Some(below) = image_element(renderer, &images.below) else {
            return Vec::new();
        };

        let mut elements = Vec::new();
        if self.overview.caret_on {
            let caret = self.overview.caret_image(scale);
            let caret_top = scene.search.loc.y + (SEARCH_HEIGHT - CARET_HEIGHT) / 2;
            let location = Point::<i32, Logical>::from((scene.caret_x, caret_top))
                .to_f64()
                .to_physical(scale)
                .to_i32_round::<i32>();
            let element = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location.to_f64(),
                caret,
                None,
                Some(image_src((CARET_WIDTH, CARET_HEIGHT).into(), scale)),
                Some((CARET_WIDTH, CARET_HEIGHT).into()),
                Kind::Unspecified,
            );
            match element {
                Ok(element) => elements.push(SwitcherElement::Image(element)),
                Err(err) => warn!(?err, "failed to draw the caret"),
            }
        }
        elements.push(SwitcherElement::Image(above));
        for placed in &scene.items {
            let Item::Window { id, thumb, .. } = &placed.item else {
                continue;
            };
            let Some(window) = self.windows.iter().find(|w| w.id == *id) else {
                continue;
            };
            let Some(surface) = window.wl_surface() else {
                continue;
            };
            // The window's own area, without client-side shadows.
            let window_geo = window.window.geometry();
            let Some(fitted) = fit(*thumb, window_geo.size) else {
                continue;
            };
            let factor = fitted.size.w / window_geo.size.w as f64;
            let thumb_origin = fitted.loc.to_physical(scale).to_i32_round::<i32>();
            let crop = Rectangle::new(thumb_origin, fitted.size.to_physical(scale).to_i32_round());
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
        elements.push(SwitcherElement::Image(below));
        self.overview.scene = Some(scene);
        elements
    }
}

/// Where the windows or results go: under the search field, inside the
/// margins.
fn content_area(area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let top = SEARCH_TOP + SEARCH_HEIGHT + CONTENT_GAP;
    Rectangle::new(
        (area.loc.x + MARGIN, area.loc.y + top).into(),
        (
            (area.size.w - 2 * MARGIN).max(0),
            (area.size.h - top - MARGIN).max(0),
        )
            .into(),
    )
}

/// Lays `count` screen-shaped thumbnail boxes out in `area`, with as many
/// columns as make them biggest and, at the same size, as few rows as
/// possible, each row centred. Returns the boxes and the number of columns.
fn grid(
    count: usize,
    area: Rectangle<i32, Logical>,
    screen: (i32, i32),
) -> (Vec<Rectangle<i32, Logical>>, usize) {
    if count == 0 || screen.0 <= 0 || screen.1 <= 0 {
        return (Vec::new(), 1);
    }
    let aspect = screen.0 as f64 / screen.1 as f64;
    let mut best = (0, 1);
    for columns in 1..=count {
        let rows = count.div_ceil(columns);
        let cell_width = (area.size.w - (columns as i32 - 1) * GRID_GAP) / columns as i32;
        let cell_height = (area.size.h - (rows as i32 - 1) * GRID_GAP) / rows as i32 - NAME_HEIGHT;
        if cell_width <= 0 || cell_height <= 0 {
            continue;
        }
        let width = cell_width
            .min((cell_height as f64 * aspect) as i32)
            .min(MAX_THUMB_WIDTH);
        // At the same width, more columns means fewer rows.
        if width >= best.0 {
            best = (width, columns);
        }
    }
    let (width, columns) = best;
    let width = width.max(1);
    let height = ((width as f64 / aspect) as i32).max(1);
    let rows = count.div_ceil(columns);
    let total_height = rows as i32 * (height + NAME_HEIGHT) + (rows as i32 - 1) * GRID_GAP;
    let top = area.loc.y + ((area.size.h - total_height) / 2).max(0);
    let boxes = (0..count)
        .map(|index| {
            let (row, column) = (index / columns, index % columns);
            let in_row = columns.min(count - row * columns) as i32;
            let row_width = in_row * width + (in_row - 1) * GRID_GAP;
            let left = area.loc.x + (area.size.w - row_width) / 2;
            Rectangle::new(
                (
                    left + column as i32 * (width + GRID_GAP),
                    top + row as i32 * (height + NAME_HEIGHT + GRID_GAP),
                )
                    .into(),
                (width, height).into(),
            )
        })
        .collect();
    (boxes, columns)
}

/// Draws the backdrop, search field, highlight, names and row icons,
/// and separately the icons that go over the window thumbnails. `icons` has
/// one entry per item.
fn draw(
    font: Option<&Font>,
    scene: &Scene,
    icons: &[Option<&Pixmap>],
    scale: f64,
) -> (Image, Image) {
    let width = (scene.area.size.w as f64 * scale).round() as i32;
    let height = (scene.area.size.h as f64 * scale).round() as i32;
    let mut below = Canvas::new(width, height);
    let mut above = Canvas::new(width, height);
    // Scale from logical to image pixels, exact for this image's size.
    let s = width as f32 / scene.area.size.w.max(1) as f32;
    let origin = scene.area.loc;
    let rect = |r: Rectangle<i32, Logical>| {
        (
            (r.loc.x - origin.x) as f32 * s,
            (r.loc.y - origin.y) as f32 * s,
            r.size.w as f32 * s,
            r.size.h as f32 * s,
        )
    };
    let style = |size: f32, color| TextStyle {
        size: size * s,
        color,
    };

    below.fill(BACKDROP);

    let search = rect(scene.search);
    below.rounded_rect(search, search.3 / 2.0, FIELD);
    if let Some(font) = font {
        let left = search.0 + SEARCH_PADDING as f32 * s;
        let max_width = search.2 - 2.0 * SEARCH_PADDING as f32 * s;
        let middle = search.1 + search.3 / 2.0;
        if scene.query.is_empty() {
            below.left_text(
                font,
                PLACEHOLDER_TEXT,
                style(SEARCH_SIZE, HINT_COLOR),
                left,
                max_width,
                middle,
            );
        } else {
            below.left_text(
                font,
                &scene.query,
                style(SEARCH_SIZE, TEXT_COLOR),
                left,
                max_width,
                middle,
            );
        }
    }

    if let (Some(font), Some(message)) = (font, scene.empty) {
        let content = rect(scene.content);
        below.centred_text(
            font,
            message,
            style(ROW_SIZE, NAME_COLOR),
            content.0 + content.2 / 2.0,
            content.2,
            content.1 + content.3 / 2.0,
        );
    }

    for (index, placed) in scene.items.iter().enumerate() {
        let tile = rect(placed.tile);
        if index == scene.selected {
            below.rounded_rect(tile, 12.0 * s, HIGHLIGHT);
        }
        let icon = icons.get(index).copied().flatten();
        match &placed.item {
            Item::Window {
                thumb, app, title, ..
            } => {
                let thumb = rect(*thumb);
                below.rounded_rect(thumb, 6.0 * s, THUMB_BACKGROUND);
                let icon_size = ICON_SIZE as f32 * s;
                let icon_box = (
                    thumb.0 + thumb.2 + ICON_OVERHANG as f32 * s - icon_size,
                    thumb.1 + thumb.3 + ICON_OVERHANG as f32 * s - icon_size,
                    icon_size,
                    icon_size,
                );
                draw_icon(&mut above, font, icon, icon_box, &app.name);
                if let Some(font) = font {
                    let middle = thumb.1 + thumb.3 + (ICON_OVERHANG + NAME_HEIGHT) as f32 * s / 2.0;
                    below.centred_text(
                        font,
                        title,
                        style(NAME_SIZE, NAME_COLOR),
                        thumb.0 + thumb.2 / 2.0,
                        thumb.2,
                        middle,
                    );
                }
            }
            Item::App { name, detail, .. } => {
                let icon_box = row_icon_box(tile, s);
                draw_icon(&mut below, font, icon, icon_box, name);
                draw_row_text(&mut below, font, tile, name, detail, s);
            }
            Item::File {
                name, folder, kind, ..
            } => {
                let icon_box = row_icon_box(tile, s);
                below.rounded_rect(icon_box, 6.0 * s, BACKGROUND);
                if let Some(font) = font {
                    below.centred_text(
                        font,
                        kind,
                        style(KIND_SIZE, TEXT_COLOR),
                        icon_box.0 + icon_box.2 / 2.0,
                        icon_box.2 - 4.0 * s,
                        icon_box.1 + icon_box.3 / 2.0,
                    );
                }
                draw_row_text(&mut below, font, tile, name, folder, s);
            }
        }
    }

    (below.into_image(), above.into_image())
}

fn row_icon_box((x, y, _, h): (f32, f32, f32, f32), s: f32) -> (f32, f32, f32, f32) {
    let size = ROW_ICON as f32 * s;
    (x + ROW_PADDING as f32 * s, y + (h - size) / 2.0, size, size)
}

/// Draws `icon` in `icon_box`, or a placeholder with the first letter of
/// `name` when there is none.
fn draw_icon(
    canvas: &mut Canvas,
    font: Option<&Font>,
    icon: Option<&Pixmap>,
    icon_box: (f32, f32, f32, f32),
    name: &str,
) {
    match icon {
        Some(pixmap) => canvas.pixmap(pixmap, icon_box.0.round() as i32, icon_box.1.round() as i32),
        None => {
            canvas.rounded_rect(icon_box, icon_box.2 / 6.0, PLACEHOLDER);
            if let Some(font) = font {
                let letter: String = name.chars().take(1).collect();
                let style = TextStyle {
                    size: icon_box.2 * 0.5,
                    color: TEXT_COLOR,
                };
                canvas.centred_text(
                    font,
                    &letter,
                    style,
                    icon_box.0 + icon_box.2 / 2.0,
                    icon_box.2,
                    icon_box.1 + icon_box.3 / 2.0,
                );
            }
        }
    }
}

/// The name and, dimmer under it, the detail of a result row.
fn draw_row_text(
    canvas: &mut Canvas,
    font: Option<&Font>,
    tile: (f32, f32, f32, f32),
    name: &str,
    detail: &str,
    s: f32,
) {
    let Some(font) = font else {
        return;
    };
    let left = tile.0 + (ROW_PADDING + ROW_ICON + 14) as f32 * s;
    let max_width = tile.0 + tile.2 - ROW_PADDING as f32 * s - left;
    let name_style = TextStyle {
        size: ROW_SIZE * s,
        color: TEXT_COLOR,
    };
    let detail_style = TextStyle {
        size: DETAIL_SIZE * s,
        color: NAME_COLOR,
    };
    if detail.is_empty() {
        canvas.left_text(
            font,
            name,
            name_style,
            left,
            max_width,
            tile.1 + tile.3 / 2.0,
        );
    } else {
        canvas.left_text(font, name, name_style, left, max_width, tile.1 + 20.0 * s);
        canvas.left_text(
            font,
            detail,
            detail_style,
            left,
            max_width,
            tile.1 + 38.0 * s,
        );
    }
}

/// Runs the file search command with `query` as `$1` and reads one path
/// per line from what it prints.
fn run_file_search(command: &str, query: &str, child_env: &ChildEnv) -> Vec<FileHit> {
    let mut sh = Command::new("sh");
    sh.arg("-c")
        .arg(command)
        .arg("rustwm-overview")
        .arg(query)
        .stdin(Stdio::null());
    child_env.apply(&mut sh);
    let output = match sh.output() {
        Ok(output) => output,
        Err(err) => {
            warn!(command, %err, "failed to run the file search");
            return Vec::new();
        }
    };
    if !output.status.success() && output.stdout.is_empty() {
        debug!(command, status = %output.status, "file search found nothing");
    }
    let home = std::env::var("HOME").unwrap_or_default();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| file_hit(Path::new(line), &home))
        .collect()
}

fn file_hit(path: &Path, home: &str) -> FileHit {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let folder = path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    let folder = match folder.strip_prefix(home).filter(|_| !home.is_empty()) {
        Some(rest) => format!("~{rest}"),
        None => folder,
    };
    let kind = if path.is_dir() {
        "DIR".to_string()
    } else {
        path.extension()
            .map(|ext| ext.to_string_lossy().to_uppercase())
            .filter(|ext| ext.chars().count() <= 4)
            .unwrap_or_else(|| "FILE".to_string())
    };
    FileHit {
        path: path.to_path_buf(),
        name,
        folder,
        kind,
    }
}

/// The byte index of character `index` in `text`, or the end.
fn char_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

/// Applies `edit` to `text` at `caret`, a character index, which it moves.
/// Returns whether the text changed.
fn apply_edit(text: &mut String, caret: &mut usize, edit: Edit) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    *caret = (*caret).min(len);
    let (start, end) = match edit {
        Edit::Left => {
            *caret = caret.saturating_sub(1);
            return false;
        }
        Edit::Right => {
            *caret = (*caret + 1).min(len);
            return false;
        }
        Edit::Home => {
            *caret = 0;
            return false;
        }
        Edit::End => {
            *caret = len;
            return false;
        }
        Edit::WordLeft => {
            *caret = word_start(&chars, *caret);
            return false;
        }
        Edit::WordRight => {
            *caret = word_end(&chars, *caret);
            return false;
        }
        Edit::Backspace => (caret.saturating_sub(1), *caret),
        Edit::Delete => (*caret, (*caret + 1).min(len)),
        Edit::DeleteWordBack => (word_start(&chars, *caret), *caret),
        Edit::DeleteWordForward => (*caret, word_end(&chars, *caret)),
        Edit::Clear => (0, len),
    };
    if start == end {
        return false;
    }
    *text = chars[..start].iter().chain(&chars[end..]).collect();
    *caret = start;
    true
}

/// The start of the word before `index`: back over spaces, then over the
/// word.
fn word_start(chars: &[char], index: usize) -> usize {
    let mut i = index;
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// The end of the word after `index`: forward over spaces, then over the
/// word.
fn word_end(chars: &[char], index: usize) -> usize {
    let mut i = index;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    while i < chars.len() && !chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// `path` quoted for `sh -c`.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (w, h).into())
    }

    #[test]
    fn grid_uses_the_space_and_centres_rows() {
        let (boxes, columns) = grid(5, area(1600, 900), (1600, 1000));
        assert_eq!(columns, 3);
        assert_eq!(boxes.len(), 5);
        // Screen shaped.
        let first = boxes[0];
        assert!((first.size.w as f64 / first.size.h as f64 - 1.6).abs() < 0.02);
        // The second row has two boxes, centred under the three above.
        let row_left = boxes[3].loc.x;
        let row_right = boxes[4].loc.x + boxes[4].size.w;
        assert_eq!(row_left, 1600 - row_right);
        assert!(boxes[3].loc.y > boxes[0].loc.y + boxes[0].size.h);
        // Nothing leaves the area.
        for b in &boxes {
            assert!(b.loc.x >= 0 && b.loc.x + b.size.w <= 1600);
            assert!(b.loc.y >= 0 && b.loc.y + b.size.h <= 900);
        }
    }

    #[test]
    fn single_window_is_capped_and_centred() {
        let (boxes, columns) = grid(1, area(2000, 1200), (2000, 1250));
        assert_eq!(columns, 1);
        assert_eq!(boxes[0].size.w, MAX_THUMB_WIDTH);
        assert_eq!(boxes[0].loc.x, (2000 - MAX_THUMB_WIDTH) / 2);
        // Three windows that fit at full size side by side make one row.
        let (boxes, columns) = grid(3, area(1952, 1000), (2048, 1280));
        assert_eq!(columns, 3);
        assert!(boxes.iter().all(|b| b.loc.y == boxes[0].loc.y));
        assert_eq!(grid(0, area(2000, 1200), (2000, 1250)).0, Vec::new());
    }

    #[test]
    fn many_windows_still_fit() {
        let (boxes, columns) = grid(24, area(1200, 700), (1920, 1080));
        assert_eq!(boxes.len(), 24);
        assert!(columns >= 4);
        let last = boxes[23];
        assert!(last.loc.y + last.size.h + NAME_HEIGHT <= 700);
    }

    #[test]
    fn file_hits_show_folder_and_kind() {
        let dir = std::env::temp_dir().join(format!("rustwm-overview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hit = file_hit(Path::new("/home/me/Documents/report.pdf"), "/home/me");
        assert_eq!(hit.name, "report.pdf");
        assert_eq!(hit.folder, "~/Documents");
        assert_eq!(hit.kind, "PDF");
        let hit = file_hit(&dir, "");
        assert_eq!(hit.kind, "DIR");
        assert_eq!(hit.folder, dir.parent().unwrap().display().to_string());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(file_hit(Path::new("/tmp/README"), "/home/me").kind, "FILE");
        assert_eq!(file_hit(Path::new("/tmp/a.tar.gzipped"), "/x").kind, "FILE");
    }

    #[test]
    fn edits_work_on_characters_and_words() {
        let mut text = String::from("héllo wörld  x");
        let mut caret = text.chars().count();
        assert!(!apply_edit(&mut text, &mut caret, Edit::Left));
        assert_eq!(caret, 13);
        assert!(apply_edit(&mut text, &mut caret, Edit::Backspace));
        assert_eq!((text.as_str(), caret), ("héllo wörld x", 12));
        // The space before the caret goes with the word.
        assert!(apply_edit(&mut text, &mut caret, Edit::DeleteWordBack));
        assert_eq!((text.as_str(), caret), ("héllo x", 6));
        assert!(!apply_edit(&mut text, &mut caret, Edit::WordLeft));
        assert_eq!(caret, 0);
        assert!(!apply_edit(&mut text, &mut caret, Edit::Backspace));
        assert!(!apply_edit(&mut text, &mut caret, Edit::WordRight));
        assert_eq!(caret, 5);
        assert!(apply_edit(&mut text, &mut caret, Edit::DeleteWordForward));
        assert_eq!((text.as_str(), caret), ("héllo", 5));
        assert!(!apply_edit(&mut text, &mut caret, Edit::Delete));
        assert!(!apply_edit(&mut text, &mut caret, Edit::Left));
        assert_eq!(caret, 4);
        assert!(apply_edit(&mut text, &mut caret, Edit::Delete));
        assert_eq!((text.as_str(), caret), ("héll", 4));
        assert!(!apply_edit(&mut text, &mut caret, Edit::Home));
        assert!(!apply_edit(&mut text, &mut caret, Edit::Right));
        assert_eq!(caret, 1);
        assert!(!apply_edit(&mut text, &mut caret, Edit::End));
        assert_eq!(caret, 4);
        assert!(apply_edit(&mut text, &mut caret, Edit::Clear));
        assert_eq!((text.as_str(), caret), ("", 0));
        assert!(!apply_edit(&mut text, &mut caret, Edit::Clear));
        assert_eq!(char_to_byte("héllo", 2), 3);
        assert_eq!(char_to_byte("héllo", 9), 6);
    }

    #[test]
    fn quoting_survives_apostrophes() {
        assert_eq!(quote(Path::new("/a b/c'd")), "'/a b/c'\\''d'");
    }

    #[test]
    fn app_placeholder_and_search_field_are_drawn() {
        let scene = Scene {
            output: (0, 0).into(),
            area: area(800, 600),
            query: "fi".into(),
            search: Rectangle::new((80, SEARCH_TOP).into(), (640, SEARCH_HEIGHT).into()),
            caret_x: 120,
            content: content_area(area(800, 600)),
            items: vec![Placed {
                tile: Rectangle::new((40, 160).into(), (720, ROW_HEIGHT).into()),
                item: Item::App {
                    index: 0,
                    name: "Files".into(),
                    detail: String::new(),
                    icon: None,
                },
            }],
            columns: 1,
            selected: 0,
            empty: None,
        };
        let (below, above) = draw(None, &scene, &[None], 1.0);
        let pixel = |image: &Image, x: i32, y: i32| {
            let i = ((y * image.width + x) * 4) as usize;
            <[u8; 4]>::try_from(&image.pixels[i..i + 4]).unwrap()
        };
        // The backdrop is nearly opaque everywhere, nothing is above.
        assert!(pixel(&below, 5, 5)[3] > 240);
        assert_eq!(pixel(&above, 5, 5)[3], 0);
        // The search field and the highlighted row are lighter than the backdrop.
        assert!(pixel(&below, 400, SEARCH_TOP + SEARCH_HEIGHT / 2)[0] > pixel(&below, 5, 5)[0]);
        assert!(pixel(&below, 400, 160 + ROW_HEIGHT / 2)[0] > pixel(&below, 5, 5)[0]);
        // The placeholder icon is grey and opaque.
        let icon = pixel(
            &below,
            40 + ROW_PADDING + ROW_ICON / 2,
            160 + ROW_HEIGHT / 2,
        );
        assert_eq!(icon[3], 255);
        assert!(icon[0] > icon[2]);
    }
}
