//! ext-foreign-toplevel-list: tells other clients which windows exist,
//! from when they are first shown until they close, with their title and
//! app id. xdg-desktop-portal-wlr offers these windows
//! for sharing, and captures the chosen one through a capture source made
//! from its handle (see `crate::capture::image_copy`).

use smithay::{
    delegate_foreign_toplevel_list,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::foreign_toplevel_list::{
        ForeignToplevelHandle, ForeignToplevelListHandler, ForeignToplevelListState,
    },
};

use crate::{focus::WindowId, state::State, switcher::app_id_and_title, window::ManagedWindow};

impl ForeignToplevelListHandler for State {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.foreign_toplevel_list
    }
}
delegate_foreign_toplevel_list!(State);

impl State {
    /// Lists a window when it is first shown. A window that hides and shows
    /// again, as tray apps do, keeps its entry, so a share of it pauses
    /// rather than ends.
    pub fn announce_window(&mut self, id: WindowId) {
        let Some(window) = self.window(id) else {
            return;
        };
        if window.foreign_toplevel.is_some() {
            return;
        }
        let Some(surface) = window.wl_surface().cloned() else {
            return;
        };
        let (app_id, title) = app_id_and_title(&surface);
        let handle = self
            .foreign_toplevel_list
            .new_toplevel::<Self>(title, app_id);
        // A capture source only holds the handle; this finds its window.
        handle.user_data().insert_if_missing_threadsafe(|| id);
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == id) {
            window.foreign_toplevel = Some(handle);
        }
    }

    /// Takes a closed window off the list. Clients see its handle close,
    /// and a capture of it stops.
    pub fn withdraw_window(&mut self, id: WindowId) {
        let Some(window) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        if let Some(handle) = window.foreign_toplevel.take() {
            handle.send_closed();
            self.foreign_toplevel_list.cleanup_closed_handles();
        }
    }

    /// Passes a changed title or app id on to the clients listing windows.
    pub fn update_window_title(&self, surface: &WlSurface) {
        let Some(handle) = self
            .window_for_surface(surface)
            .and_then(|w| w.foreign_toplevel.as_ref())
        else {
            return;
        };
        let (app_id, title) = app_id_and_title(surface);
        handle.send_title(&title);
        handle.send_app_id(&app_id);
        handle.send_done();
    }

    /// The window a listed handle stands for, until it closes.
    pub fn window_for_foreign_toplevel(
        &self,
        handle: &ForeignToplevelHandle,
    ) -> Option<&ManagedWindow> {
        let id = handle.user_data().get::<WindowId>()?;
        self.window(*id).filter(|w| {
            w.foreign_toplevel
                .as_ref()
                .is_some_and(|h| h.identifier() == handle.identifier())
        })
    }
}
