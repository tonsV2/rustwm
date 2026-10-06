//! keyboard-shortcuts-inhibit: a virtual machine or remote desktop client
//! asks for rustwm's shortcuts (Super, Alt+Tab, ...) to reach it while it
//! has the keyboard. rustwm allows it right away, as sway does, and keeps a
//! few keys to itself so a client cannot trap the user: `Super+Escape`
//! turns the inhibitor off and on again, and Ctrl+Alt+Backspace, the
//! virtual terminal keys and the brightness, volume and mic mute keys keep
//! working (see [`crate::keyboard`]). A window the user took the shortcuts
//! back from does not get them by asking again, only by Super+Escape.

use smithay::{
    delegate_keyboard_shortcuts_inhibit,
    reexports::wayland_server::Resource,
    wayland::keyboard_shortcuts_inhibit::{
        KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
        KeyboardShortcutsInhibitorSeat,
    },
};
use tracing::info;

use crate::state::State;

impl State {
    /// The inhibitor of the surface with keyboard focus, active or not.
    fn focused_shortcuts_inhibitor(&self) -> Option<KeyboardShortcutsInhibitor> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.seat.keyboard_shortcuts_inhibitor_for_surface(&focus)
    }

    /// Whether the focused window has asked for rustwm's shortcuts.
    pub fn has_shortcuts_inhibitor(&self) -> bool {
        self.focused_shortcuts_inhibitor().is_some()
    }

    /// Whether rustwm's shortcuts reach the focused window instead.
    pub fn shortcuts_inhibited(&self) -> bool {
        self.focused_shortcuts_inhibitor()
            .is_some_and(|inhibitor| inhibitor.is_active())
    }

    /// Turns the focused window's inhibitor off, or on again (Super+Escape).
    pub fn toggle_shortcuts_inhibit(&mut self) {
        let Some(inhibitor) = self.focused_shortcuts_inhibitor() else {
            return;
        };
        let surface = inhibitor.wl_surface().clone();
        self.shortcuts_revoked
            .retain(|s| s.is_alive() && s != &surface);
        if inhibitor.is_active() {
            inhibitor.inactivate();
            self.shortcuts_revoked.push(surface);
            info!("keyboard shortcuts restored");
        } else {
            inhibitor.activate();
            info!("keyboard shortcuts inhibited again");
        }
    }
}

impl KeyboardShortcutsInhibitHandler for State {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.keyboard_shortcuts_inhibit_state
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        if self.shortcuts_revoked.contains(inhibitor.wl_surface()) {
            info!("keyboard shortcuts inhibitor left off, as the user turned it off");
            return;
        }
        info!("keyboard shortcuts inhibited by a client");
        inhibitor.activate();
    }
}
delegate_keyboard_shortcuts_inhibit!(State);
