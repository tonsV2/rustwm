//! xdg-shell: application windows and their popups.

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    delegate_xdg_shell,
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window,
        find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output,
    },
    input::{Seat, pointer::Focus},
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_seat, wl_surface::WlSurface},
    utils::{Rectangle, Serial},
    wayland::shell::xdg::{
        PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    },
};
use tracing::{debug, warn};

use super::log_window;
use crate::{focus::WindowId, layout::Layout, state::State, window::ManagedWindow};

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let Some(output) = self.primary_output() else {
            warn!("new window but no output; closing it");
            surface.send_close();
            return;
        };
        let id = self.next_window_id();
        log_window("window created", id.0, &surface);

        // Placement is decided on the first commit, once the client has had
        // the chance to say whether this is a dialog with a parent.
        self.windows.push(ManagedWindow {
            id,
            window: Window::new_wayland_window(surface),
            layout: None,
            output,
            mapped: false,
            unzoomed: None,
        });
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(pos) = self
            .windows
            .iter()
            .position(|w| w.wl_surface() == Some(surface.wl_surface()))
        else {
            return;
        };
        let window = self.windows.remove(pos);
        log_window("window destroyed", window.id.0, &surface);

        self.space.unmap_elem(&window.window);
        self.focus.remove(window.id);
        self.groups.remove(window.id);
        self.sync_focus();
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            warn!(?err, "failed to track popup");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    /// A popup (menu) wants all input until it is dismissed.
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        // Windows get no input while the screen is locked.
        if self.is_locked() {
            surface.send_popup_done();
            return;
        }
        let Some(seat) = Seat::<State>::from_resource(&seat) else {
            return;
        };
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) else {
            return;
        };

        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    // Window placement is keyboard-driven, so client requests to maximize or
    // go fullscreen are ignored. The protocol still requires an answering
    // configure, which repeats the current state.
    fn maximize_request(&mut self, surface: ToplevelSurface) {
        reject_state_request(&surface);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        reject_state_request(&surface);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        reject_state_request(&surface);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        reject_state_request(&surface);
    }
}
delegate_xdg_shell!(State);

fn reject_state_request(surface: &ToplevelSurface) {
    if surface.is_initial_configure_sent() {
        surface.send_pending_configure();
    }
}

/// Handles the window-management side of a surface commit.
pub fn handle_commit(state: &mut State, surface: &WlSurface) {
    if let Some(window) = state.window_for_surface(surface) {
        let id = window.id;
        let mapped = window.mapped;
        let Some(toplevel) = window.window.toplevel().cloned() else {
            return;
        };

        if !toplevel.is_initial_configure_sent() {
            state.send_initial_configure(id, &toplevel);
            return;
        }

        let has_buffer =
            with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
        if has_buffer && !mapped {
            state.map_window(id);
        } else if !has_buffer && mapped {
            state.unmap_window(id);
        }
        return;
    }

    if let Some(PopupKind::Xdg(popup)) = state.popups.find_popup(surface)
        && !popup.is_initial_configure_sent()
        && let Err(err) = popup.send_configure()
    {
        warn!(?err, "failed to configure popup");
    }
}

impl State {
    /// Decides a new window's layout and sends the first configure.
    ///
    /// Windows with a parent are dialogs: they keep their own size and float
    /// centred. Everything else starts out filling the output.
    fn send_initial_configure(&mut self, id: WindowId, toplevel: &ToplevelSurface) {
        let is_dialog = toplevel.parent().is_some();
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == id) {
            window.layout = (!is_dialog).then_some(Layout::Fullscreen);
        }
        if !is_dialog {
            self.apply_layout(id);
        }
        toplevel.send_configure();
    }

    /// Shows a window once the client has attached its first buffer.
    fn map_window(&mut self, id: WindowId) {
        let Some(window) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        window.mapped = true;
        let window = window.clone();

        let Some(area) = self.usable_area(&window.output) else {
            return;
        };
        let location = match window.layout {
            Some(layout) => {
                let geo = layout.geometry(area);
                (geo.x, geo.y)
            }
            None => {
                let size = window.window.geometry().size;
                (
                    area.x + (area.width - size.w).max(0) / 2,
                    area.y + (area.height - size.h).max(0) / 2,
                )
            }
        };
        self.space
            .map_element(window.window.clone(), location, false);
        debug!(
            window = id.0,
            ?location,
            size = ?window.window.geometry().size,
            "window placed"
        );

        if let Some(toplevel) = window.window.toplevel() {
            log_window("window mapped", id.0, toplevel);
        }
        self.focus_window(id);
    }

    /// Hides a window whose client removed its buffer. The window stays
    /// managed and is shown again on its next buffer.
    fn unmap_window(&mut self, id: WindowId) {
        let Some(window) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        window.mapped = false;
        let element = window.window.clone();
        if let Some(toplevel) = element.toplevel() {
            log_window("window unmapped", id.0, toplevel);
        }

        self.space.unmap_elem(&element);
        self.focus.remove(id);
        self.sync_focus();
    }

    /// Keeps a popup inside the output of the window or layer surface it
    /// belongs to, flipping or sliding it as its positioner allows.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };

        // The target area is relative to the popup's parent geometry.
        let (output, parent_loc) = if let Some(window) = self.window_for_surface(&root) {
            let Some(geo) = self.space.element_geometry(&window.window) else {
                return;
            };
            let Some(output_geo) = self.space.output_geometry(&window.output) else {
                return;
            };
            (output_geo, geo.loc)
        } else if let Some((output, layer)) = self.layer_for_surface(&root) {
            let Some(output_geo) = self.space.output_geometry(&output) else {
                return;
            };
            let Some(layer_geo) = layer_map_for_output(&output).layer_geometry(&layer) else {
                return;
            };
            (output_geo, output_geo.loc + layer_geo.loc)
        } else {
            return;
        };

        let mut target = Rectangle::new(output.loc, output.size);
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= parent_loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
