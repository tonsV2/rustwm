//! ext-idle-notify and idle-inhibit: tell an idle daemon such as swayidle
//! when the user stops using the keyboard and mouse, unless an application
//! playing video asks to keep the screen on.

use smithay::{
    delegate_idle_inhibit, delegate_idle_notify,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    wayland::{
        compositor::get_parent,
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
    },
};

use crate::state::State;

impl IdleNotifierHandler for State {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle_notifier_state
    }
}
delegate_idle_notify!(State);

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.push(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        if let Some(index) = self.idle_inhibitors.iter().position(|s| s == &surface) {
            self.idle_inhibitors.swap_remove(index);
        }
    }
}
delegate_idle_inhibit!(State);

impl State {
    /// Keeps the session from going idle while a surface with an idle
    /// inhibitor is visible. Runs after every batch of events, since
    /// surfaces map, unmap and die at any time.
    pub fn update_idle_inhibit(&mut self) {
        self.idle_inhibitors.retain(|surface| surface.is_alive());
        let inhibited = self
            .idle_inhibitors
            .iter()
            .any(|surface| self.is_visible(surface));
        self.idle_notifier_state.set_is_inhibited(inhibited);
    }

    /// Whether `surface` belongs to something on screen: a mapped window or
    /// a layer surface, or while locked, only a lock surface.
    fn is_visible(&self, surface: &WlSurface) -> bool {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if self.is_locked() {
            return self
                .space
                .outputs()
                .any(|output| self.lock_surface(output).map(|s| s.wl_surface()) == Some(&root));
        }
        self.window_for_surface(&root).is_some_and(|w| w.mapped)
            || self.layer_for_surface(&root).is_some()
    }
}
