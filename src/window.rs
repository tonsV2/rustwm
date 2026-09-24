//! A toplevel window as the window manager sees it.

use smithay::{
    desktop::Window,
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
};

use crate::{
    focus::WindowId,
    layout::{Layout, Rect},
};

#[derive(Debug, Clone)]
pub struct ManagedWindow {
    pub id: WindowId,
    pub window: Window,
    /// `None` for dialogs, which float at their own size in the middle of
    /// the output until the user snaps them.
    pub layout: Option<Layout>,
    pub output: Output,
    /// Whether the client has attached a buffer, so the window is shown.
    pub mapped: bool,
    /// The tile a group member was zoomed to full screen from, which the next Super+arrow puts it back in.
    pub unzoomed: Option<Layout>,
    /// The client asked to cover the whole output, as a video player does, until it asks to leave or the user snaps it.
    pub fullscreen: bool,
}

impl ManagedWindow {
    pub fn wl_surface(&self) -> Option<&WlSurface> {
        self.window.toplevel().map(|t| t.wl_surface())
    }

    /// Tells the client its size and tiling state for `layout`.
    ///
    /// The window is marked maximized when it fills the output rather than
    /// fullscreen: the xdg "fullscreen" state makes browsers and video
    /// players hide their own UI. Tiled edges make clients drop shadows and
    /// rounded corners where the window meets the screen edge.
    pub fn configure_for_layout(&self, layout: Layout, geometry: Rect, area: Rect) {
        let Some(toplevel) = self.window.toplevel() else {
            return;
        };
        let edges = layout.tiled_edges();

        toplevel.with_pending_state(|state| {
            state.size = Some((geometry.width, geometry.height).into());
            state.bounds = Some((area.width, area.height).into());

            let mut set = |flag: xdg_toplevel::State, on: bool| {
                if on {
                    state.states.set(flag);
                } else {
                    state.states.unset(flag);
                }
            };
            set(xdg_toplevel::State::Fullscreen, false);
            set(xdg_toplevel::State::Maximized, layout == Layout::Fullscreen);
            set(xdg_toplevel::State::TiledLeft, edges.left);
            set(xdg_toplevel::State::TiledRight, edges.right);
            set(xdg_toplevel::State::TiledTop, edges.top);
            set(xdg_toplevel::State::TiledBottom, edges.bottom);
        });

        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    /// Tells the client it covers `output`, the whole output including where panels sit.
    pub fn configure_fullscreen(&self, output: Rect) {
        let Some(toplevel) = self.window.toplevel() else {
            return;
        };
        toplevel.with_pending_state(|state| {
            state.size = Some((output.width, output.height).into());
            state.bounds = Some((output.width, output.height).into());
            state.states.set(xdg_toplevel::State::Fullscreen);
            for flag in [
                xdg_toplevel::State::Maximized,
                xdg_toplevel::State::TiledLeft,
                xdg_toplevel::State::TiledRight,
                xdg_toplevel::State::TiledTop,
                xdg_toplevel::State::TiledBottom,
            ] {
                state.states.unset(flag);
            }
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}
