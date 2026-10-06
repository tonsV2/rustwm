//! The visual bell (xdg-system-bell): instead of a sound, the window that
//! rang it, or the whole screen when no window did, flashes white for a
//! moment and fades back.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::{
        element::{Id, Kind, solid::SolidColorRenderElement},
        utils::CommitCounter,
    },
    delegate_xdg_system_bell,
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Rectangle},
    wayland::{compositor::get_parent, xdg_system_bell::XdgSystemBellHandler},
};
use tracing::debug;

use crate::{focus::WindowId, state::State};

/// How long the flash takes to fade out.
const FLASH: Duration = Duration::from_millis(250);
/// How white the flash starts.
const PEAK: f32 = 0.3;

/// The last ring and how far its flash has faded.
#[derive(Debug)]
pub struct Bell {
    flash: Option<Flash>,
    id: Id,
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
            id: Id::new(),
            commit: CommitCounter::default(),
            drawn: 0.0,
        }
    }
}

impl Bell {
    /// Starts a flash, over `window` or the whole screen. A ring during a
    /// flash starts it again.
    fn ring(&mut self, window: Option<WindowId>) {
        self.flash = Some(Flash {
            window,
            started: Instant::now(),
        });
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
    /// The flash for `output`, in front of the windows; `None` when the bell
    /// is quiet or the window that rang is gone.
    pub fn bell_element(&mut self, output: &Output) -> Option<SolidColorRenderElement> {
        let alpha = self.bell.alpha(Instant::now());
        if alpha <= 0.0 {
            self.bell.flash = None;
            self.bell.drawn = 0.0;
            return None;
        }
        let output_geometry = self.space.output_geometry(output)?;
        let area: Rectangle<i32, Logical> = match self.bell.flash?.window {
            Some(id) => {
                let window = self.window(id).filter(|w| w.mapped)?;
                let geometry = self.space.element_geometry(&window.window)?;
                Rectangle::new(geometry.loc - output_geometry.loc, geometry.size)
            }
            None => Rectangle::from_size(output_geometry.size),
        };
        if alpha != self.bell.drawn {
            self.bell.commit.increment();
            self.bell.drawn = alpha;
        }
        let scale = output.current_scale().fractional_scale();
        let area: Rectangle<i32, Physical> = area.to_physical_precise_round(scale);
        Some(SolidColorRenderElement::new(
            self.bell.id.clone(),
            area,
            self.bell.commit,
            [alpha, alpha, alpha, alpha],
            Kind::Unspecified,
        ))
    }
}

impl XdgSystemBellHandler for State {
    fn ring(&mut self, surface: Option<WlSurface>) {
        // A subsurface rings for the window it is part of.
        let mut root = surface;
        while let Some(parent) = root.as_ref().and_then(get_parent) {
            root = Some(parent);
        }
        let window = root
            .as_ref()
            .and_then(|surface| self.window_for_surface(surface))
            .map(|w| w.id);
        debug!(window = window.map(|id| id.0), "bell rang");
        self.bell.ring(window);
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
        bell.ring(None);
        let start = bell.flash.unwrap().started;
        assert_eq!(bell.alpha(start), PEAK);
        assert!((bell.alpha(start + FLASH / 2) - PEAK / 2.0).abs() < 0.01);
        assert_eq!(bell.alpha(start + FLASH), 0.0);
        assert_eq!(bell.alpha(start + FLASH * 2), 0.0);
    }
}
