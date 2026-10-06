//! Dialogs: windows with a parent. They open centred over their parent and
//! follow it when it is snapped or laid out again. A modal dialog
//! (xdg-dialog) also stays above its parent and takes the focus whenever
//! the parent would get it, as in GNOME, so clicking a window that waits on
//! a dialog brings the dialog forward.

use smithay::{
    delegate_xdg_dialog,
    desktop::Window,
    utils::{Logical, Point},
    wayland::{
        compositor::with_states,
        shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData, dialog::XdgDialogHandler},
    },
};
use tracing::info;

use crate::{focus::WindowId, state::State, window::ManagedWindow};

impl State {
    /// The window that `id` is a dialog of.
    pub fn parent_of(&self, id: WindowId) -> Option<WindowId> {
        let parent = self.window(id)?.window.toplevel()?.parent()?;
        self.window_for_surface(&parent).map(|w| w.id)
    }

    /// The shown dialogs of window `id`, modal or not.
    fn dialogs_of(&self, id: WindowId) -> Vec<WindowId> {
        self.windows
            .iter()
            .filter(|w| w.mapped && w.id != id && self.parent_of(w.id) == Some(id))
            .map(|w| w.id)
            .collect()
    }

    /// The modal dialog that window `id` waits on, and the one that dialog
    /// waits on in turn: the window that should have the focus instead.
    pub fn modal_dialog_of(&self, id: WindowId) -> Option<WindowId> {
        let mut found = None;
        let mut current = id;
        // Bounded, so a client making a loop of parents cannot hang us.
        for _ in 0..self.windows.len() {
            let Some(modal) = self
                .dialogs_of(current)
                .into_iter()
                .find(|&dialog| self.window(dialog).is_some_and(is_modal))
            else {
                break;
            };
            found = Some(modal);
            current = modal;
        }
        found
    }

    /// The windows that window `id` is a dialog of, its parent first.
    pub fn ancestors_of(&self, id: WindowId) -> Vec<WindowId> {
        let mut ancestors = Vec::new();
        let mut current = id;
        while let Some(parent) = self.parent_of(current) {
            if parent == id || ancestors.contains(&parent) {
                break;
            }
            ancestors.push(parent);
            current = parent;
        }
        ancestors
    }

    /// Raises window `id` and then its modal dialogs, so they stay above it.
    pub fn raise_with_dialogs(&mut self, id: WindowId, activate: bool) {
        let Some(element) = self
            .window(id)
            .filter(|w| w.mapped)
            .map(|w| w.window.clone())
        else {
            return;
        };
        self.space.raise_element(&element, activate);
        let mut current = id;
        // Bounded, so a client making a loop of parents cannot hang us.
        for _ in 0..self.windows.len() {
            let Some(modal) = self
                .dialogs_of(current)
                .into_iter()
                .find(|&dialog| self.window(dialog).is_some_and(is_modal))
            else {
                break;
            };
            if let Some(element) = self.window(modal).map(|w| w.window.clone()) {
                self.space.raise_element(&element, false);
            }
            current = modal;
        }
    }

    /// Where floating dialog `window` goes: centred over its parent, kept
    /// inside the usable area. `None` without a shown parent.
    pub fn dialog_location(&self, window: &ManagedWindow) -> Option<Point<i32, Logical>> {
        let parent = self.window(self.parent_of(window.id)?)?;
        let parent_geometry = self.space.element_geometry(&parent.window)?;
        let size = window.window.geometry().size;
        let area = self.usable_area(&window.output)?;
        let centre = parent_geometry.loc + parent_geometry.size.downscale(2).to_point();
        let x = (centre.x - size.w / 2).clamp(area.x, (area.x + area.width - size.w).max(area.x));
        let y = (centre.y - size.h / 2).clamp(area.y, (area.y + area.height - size.h).max(area.y));
        Some((x, y).into())
    }

    /// Centres window `id`'s floating dialogs, and theirs, over it again,
    /// after it moved.
    pub fn place_dialogs_of(&mut self, id: WindowId) {
        let mut placed = vec![id];
        let mut pending = self.dialogs_of(id);
        while let Some(dialog) = pending.pop() {
            // Each once, so a client making a loop of parents cannot hang us.
            if placed.contains(&dialog) {
                continue;
            }
            placed.push(dialog);
            if let Some(window) = self.window(dialog).filter(|w| w.layout.is_none()).cloned()
                && let Some(location) = self.dialog_location(&window)
            {
                self.move_element(window.window, location);
            }
            pending.extend(self.dialogs_of(dialog));
        }
    }

    /// Moves a shown window to `location` without changing which windows
    /// are in front of it.
    pub fn move_element(&mut self, element: Window, location: Point<i32, Logical>) {
        // Mapping moves the window to the top, so the windows that were
        // above it are raised again in their order.
        let above: Vec<Window> = self
            .space
            .elements()
            .skip_while(|e| *e != &element)
            .skip(1)
            .cloned()
            .collect();
        self.space.map_element(element, location, false);
        for window in above {
            self.space.raise_element(&window, false);
        }
    }
}

/// Whether `window` is a modal dialog.
fn is_modal(window: &ManagedWindow) -> bool {
    window.mapped
        && window.wl_surface().is_some_and(|surface| {
            with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|data| data.lock().ok())
                    .is_some_and(|data| data.modal)
            })
        })
}

impl XdgDialogHandler for State {
    /// A dialog that becomes modal while its parent has the focus takes it.
    fn modal_changed(&mut self, toplevel: ToplevelSurface, is_modal: bool) {
        let Some(id) = self.window_for_surface(toplevel.wl_surface()).map(|w| w.id) else {
            return;
        };
        info!(window = id.0, is_modal, "dialog modality changed");
        if is_modal && self.parent_of(id).is_some() && self.parent_of(id) == self.focus.current() {
            self.sync_focus();
        }
    }
}
delegate_xdg_dialog!(State);
