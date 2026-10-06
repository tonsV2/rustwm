//! The visual bell (xdg-system-bell): instead of a sound, the window that
//! rang it, or the whole screen when no window did, flashes white for a
//! moment and fades back. Only the part of the window that can be seen
//! flashes, and rings come at most every [`MIN_INTERVAL`], so a client
//! ringing nonstop cannot make the screen strobe.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::{
        element::{Id, Kind, solid::SolidColorRenderElement},
        utils::CommitCounter,
    },
    delegate_xdg_system_bell,
    desktop::{find_popup_root_surface, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Point, Rectangle},
    wayland::{
        compositor::get_parent, shell::wlr_layer::Layer, xdg_system_bell::XdgSystemBellHandler,
    },
};
use tracing::debug;

use crate::{focus::WindowId, state::State};

/// How long the flash takes to fade out.
const FLASH: Duration = Duration::from_millis(250);
/// How white the flash starts.
const PEAK: f32 = 0.3;
/// The least time between flashes; rings in between are let go. Fewer than
/// three flashes a second stays clear of what can trigger seizures.
const MIN_INTERVAL: Duration = Duration::from_millis(400);

/// The last ring and how far its flash has faded.
#[derive(Debug)]
pub struct Bell {
    flash: Option<Flash>,
    /// One per rectangle of the flash, which a window partly covered by
    /// others is split into.
    ids: Vec<Id>,
    commit: CommitCounter,
    /// The opacity last drawn, so a change is reported to damage tracking.
    drawn: f32,
}

#[derive(Debug, Clone, Copy)]
struct Flash {
    /// The window that rang; `None` flashes the whole screen.
    window: Option<WindowId>,
    started: Instant,
}

impl Default for Bell {
    fn default() -> Self {
        Self {
            flash: None,
            ids: Vec::new(),
            commit: CommitCounter::default(),
            drawn: 0.0,
        }
    }
}

impl Bell {
    /// Starts a flash, over `window` or the whole screen, unless the last
    /// one started less than [`MIN_INTERVAL`] before `now`. Returns whether
    /// it did.
    fn ring(&mut self, window: Option<WindowId>, now: Instant) -> bool {
        if self
            .flash
            .is_some_and(|flash| now.saturating_duration_since(flash.started) < MIN_INTERVAL)
        {
            return false;
        }
        self.flash = Some(Flash {
            window,
            started: now,
        });
        true
    }

    /// How white the flash is at `now`: [`PEAK`] at the ring, fading
    /// linearly to nothing over [`FLASH`].
    fn alpha(&self, now: Instant) -> f32 {
        let Some(flash) = self.flash else {
            return 0.0;
        };
        let t = now.saturating_duration_since(flash.started).as_secs_f32() / FLASH.as_secs_f32();
        PEAK * (1.0 - t).max(0.0)
    }
}

impl State {
    /// The flash for `output`, in front of the windows: the rectangles of
    /// the ringing window that nothing covers, or the whole output. Empty
    /// when the bell is quiet or the overview hides the windows.
    pub fn bell_elements(&mut self, output: &Output) -> Vec<SolidColorRenderElement> {
        let alpha = self.bell.alpha(Instant::now());
        if alpha <= 0.0 {
            self.bell.flash = None;
            self.bell.drawn = 0.0;
            return Vec::new();
        }
        if self.overview.open {
            return Vec::new();
        }
        let Some(output_geometry) = self.space.output_geometry(output) else {
            return Vec::new();
        };
        let areas = match self.bell.flash.and_then(|flash| flash.window) {
            Some(id) => self.visible_areas(id, output),
            None => vec![output_geometry],
        };
        if alpha != self.bell.drawn {
            self.bell.commit.increment();
            self.bell.drawn = alpha;
        }
        while self.bell.ids.len() < areas.len() {
            self.bell.ids.push(Id::new());
        }
        let scale = output.current_scale().fractional_scale();
        areas
            .into_iter()
            .zip(&self.bell.ids)
            .map(|(area, id)| {
                // Corners rounded on their own, as the space places windows,
                // so the edges meet the window's.
                let corner = |point: Point<i32, Logical>| {
                    (point - output_geometry.loc)
                        .to_f64()
                        .to_physical(scale)
                        .to_i32_round()
                };
                let area: Rectangle<i32, Physical> = Rectangle::from_extremities(
                    corner(area.loc),
                    corner(area.loc + area.size.to_point()),
                );
                SolidColorRenderElement::new(
                    id.clone(),
                    area,
                    self.bell.commit,
                    [alpha, alpha, alpha, alpha],
                    Kind::Unspecified,
                )
            })
            .collect()
    }

    /// The parts of window `id` on `output` that no window, panel or other
    /// layer surface in front of it covers, in global logical coordinates.
    fn visible_areas(&self, id: WindowId, output: &Output) -> Vec<Rectangle<i32, Logical>> {
        let Some(window) = self.window(id).filter(|w| w.mapped) else {
            return Vec::new();
        };
        let Some(geometry) = self.space.element_geometry(&window.window) else {
            return Vec::new();
        };
        let Some(output_geometry) = self.space.output_geometry(output) else {
            return Vec::new();
        };
        let Some(on_output) = geometry.intersection(output_geometry) else {
            return Vec::new();
        };
        let in_front = self
            .space
            .elements()
            .skip_while(|e| *e != &window.window)
            .skip(1)
            .filter_map(|e| self.space.element_geometry(e));
        let map = layer_map_for_output(output);
        let layers: Vec<Rectangle<i32, Logical>> = map
            .layers()
            .filter(|layer| matches!(layer.layer(), Layer::Top | Layer::Overlay))
            .filter_map(|layer| map.layer_geometry(layer))
            .map(|g| Rectangle::new(g.loc + output_geometry.loc, g.size))
            .collect();
        on_output.subtract_rects(in_front.chain(layers))
    }
}

impl XdgSystemBellHandler for State {
    fn ring(&mut self, surface: Option<WlSurface>) {
        // A subsurface rings for the window it is part of.
        let mut root = surface;
        while let Some(parent) = root.as_ref().and_then(get_parent) {
            root = Some(parent);
        }
        // A popup, such as a menu, rings for the window it belongs to.
        if let Some(popup) = root.as_ref().and_then(|s| self.popups.find_popup(s)) {
            root = find_popup_root_surface(&popup).ok();
        }
        // A window not shown yet has nothing to flash; the screen does.
        let window = root
            .as_ref()
            .and_then(|surface| self.window_for_surface(surface))
            .filter(|w| w.mapped)
            .map(|w| w.id);
        let flashed = self.bell.ring(window, Instant::now());
        debug!(window = window.map(|id| id.0), flashed, "bell rang");
    }
}
delegate_xdg_system_bell!(State);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_fades_out() {
        let mut bell = Bell::default();
        assert_eq!(bell.alpha(Instant::now()), 0.0);
        let start = Instant::now();
        assert!(bell.ring(None, start));
        assert_eq!(bell.alpha(start), PEAK);
        assert!((bell.alpha(start + FLASH / 2) - PEAK / 2.0).abs() < 0.01);
        assert_eq!(bell.alpha(start + FLASH), 0.0);
        assert_eq!(bell.alpha(start + FLASH * 2), 0.0);
    }

    #[test]
    fn rings_closer_than_the_interval_are_let_go() {
        let mut bell = Bell::default();
        let start = Instant::now();
        assert!(bell.ring(None, start));
        assert!(!bell.ring(Some(WindowId(1)), start + MIN_INTERVAL / 2));
        assert_eq!(bell.flash.unwrap().window, None);
        assert!(bell.ring(Some(WindowId(1)), start + MIN_INTERVAL));
    }
}
