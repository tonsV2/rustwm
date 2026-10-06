//! keyboard-shortcuts-inhibit: a virtual machine or remote desktop client
//! asks for rustwm's shortcuts (Super, Alt+Tab, ...) to reach it while it
//! has the keyboard. rustwm allows it right away, as sway does, and keeps a
//! few keys to itself so a client cannot trap the user: `Super+Escape`
//! turns the inhibitor off and on again, and Ctrl+Alt+Backspace, the
//! virtual terminal keys and the brightness and volume keys keep working
//! (see [`crate::keyboard`]).

use smithay::{
    delegate_keyboard_shortcuts_inhibit,
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
        if inhibitor.is_active() {
            inhibitor.inactivate();
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
        info!("keyboard shortcuts inhibited by a client");
        inhibitor.activate();
    }
}
delegate_keyboard_shortcuts_inhibit!(State);
