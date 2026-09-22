//! Keybinding state machine.
//!
//! Decides which key events the compositor consumes and which reach
//! clients. Pure logic with no Smithay types, so it can be unit tested.

use std::collections::HashSet;

use crate::layout::Direction;

/// The keys the bindings care about, already translated from keysyms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Super,
    Alt,
    Tab,
    Arrow(Direction),
    Backspace,
    /// One of the XF86Switch_VT_* keysyms, which the default keymap
    /// produces for Ctrl+Alt+F1..F12.
    SwitchVt(i32),
    Other,
}

/// Modifier state after the current key event was applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub logo: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
}

/// A window-management action triggered by a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Launch,
    /// Snap the focused window. `first_in_hold` is true for the first arrow
    /// pressed since Super went down.
    Snap {
        direction: Direction,
        first_in_hold: bool,
    },
    SwitchNext,
    FinishSwitch,
    /// Change to another virtual terminal (DRM backend only).
    SwitchVt(i32),
    /// Stop the compositor. On real hardware this is the way out when
    /// something goes wrong.
    Quit,
}

/// What to do with one key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Whether the event is passed on to the focused client.
    pub forward: bool,
    pub action: Option<Action>,
}

impl Outcome {
    fn forward(action: Option<Action>) -> Self {
        Self {
            forward: true,
            action,
        }
    }

    fn intercept(action: Option<Action>) -> Self {
        Self {
            forward: false,
            action,
        }
    }
}

#[derive(Debug, Default)]
pub struct Bindings {
    /// Super is down and nothing else has happened since, so releasing it
    /// launches the launcher. Deciding on release avoids any timeout.
    super_tap: bool,
    /// No arrow has been pressed yet in the current Super hold.
    first_in_hold: bool,
    /// Keycodes whose press was consumed. Their release must be consumed
    /// too, or clients would see a release without a press.
    suppressed: HashSet<u32>,
}

impl Bindings {
    pub fn key(&mut self, keycode: u32, key: Key, pressed: bool, mods: Mods) -> Outcome {
        if pressed {
            self.press(keycode, key, mods)
        } else {
            self.release(keycode, key)
        }
    }

    /// A pointer button press while Super is held means Super was used as a
    /// modifier, so it must not launch the launcher on release.
    pub fn pointer_button(&mut self) {
        self.super_tap = false;
    }

    fn press(&mut self, keycode: u32, key: Key, mods: Mods) -> Outcome {
        if key == Key::Super {
            self.super_tap = !(mods.ctrl || mods.alt || mods.shift);
            self.first_in_hold = true;
            return Outcome::forward(None);
        }
        self.super_tap = false;

        let action = match key {
            Key::Arrow(direction) if mods.logo && !mods.alt && !mods.ctrl && !mods.shift => {
                let first_in_hold = self.first_in_hold;
                self.first_in_hold = false;
                Some(Action::Snap {
                    direction,
                    first_in_hold,
                })
            }
            Key::Tab if mods.alt && !mods.logo && !mods.ctrl && !mods.shift => {
                Some(Action::SwitchNext)
            }
            Key::Backspace if mods.ctrl && mods.alt && !mods.logo && !mods.shift => {
                Some(Action::Quit)
            }
            Key::SwitchVt(vt) => Some(Action::SwitchVt(vt)),
            _ => None,
        };

        match action {
            Some(action) => {
                self.suppressed.insert(keycode);
                Outcome::intercept(Some(action))
            }
            None => Outcome::forward(None),
        }
    }

    fn release(&mut self, keycode: u32, key: Key) -> Outcome {
        if self.suppressed.remove(&keycode) {
            return Outcome::intercept(None);
        }
        let action = match key {
            Key::Super if std::mem::take(&mut self.super_tap) => Some(Action::Launch),
            Key::Alt => Some(Action::FinishSwitch),
            _ => None,
        };
        Outcome::forward(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPER: u32 = 133;
    const ALT: u32 = 64;
    const TAB: u32 = 23;
    const LEFT: u32 = 113;
    const UP: u32 = 111;
    const A: u32 = 38;
    const BACKSPACE: u32 = 22;
    const F2: u32 = 68;

    fn logo() -> Mods {
        Mods {
            logo: true,
            ..Mods::default()
        }
    }

    fn alt() -> Mods {
        Mods {
            alt: true,
            ..Mods::default()
        }
    }

    fn none() -> Mods {
        Mods::default()
    }

    #[test]
    fn super_tap_launches_on_release() {
        let mut b = Bindings::default();
        assert_eq!(
            b.key(SUPER, Key::Super, true, logo()),
            Outcome::forward(None)
        );
        assert_eq!(
            b.key(SUPER, Key::Super, false, none()),
            Outcome::forward(Some(Action::Launch))
        );
    }

    #[test]
    fn super_with_arrow_snaps_and_does_not_launch() {
        let mut b = Bindings::default();
        b.key(SUPER, Key::Super, true, logo());
        let left = Key::Arrow(Direction::Left);
        assert_eq!(
            b.key(LEFT, left, true, logo()),
            Outcome::intercept(Some(Action::Snap {
                direction: Direction::Left,
                first_in_hold: true
            }))
        );
        // The arrow's release is consumed as well.
        assert_eq!(b.key(LEFT, left, false, logo()), Outcome::intercept(None));
        assert_eq!(
            b.key(SUPER, Key::Super, false, none()),
            Outcome::forward(None)
        );
    }

    #[test]
    fn second_arrow_in_same_hold_combines() {
        let mut b = Bindings::default();
        b.key(SUPER, Key::Super, true, logo());
        b.key(LEFT, Key::Arrow(Direction::Left), true, logo());
        assert_eq!(
            b.key(UP, Key::Arrow(Direction::Up), true, logo()).action,
            Some(Action::Snap {
                direction: Direction::Up,
                first_in_hold: false
            })
        );

        // A new hold starts over.
        b.key(SUPER, Key::Super, false, none());
        b.key(SUPER, Key::Super, true, logo());
        assert_eq!(
            b.key(UP, Key::Arrow(Direction::Up), true, logo()).action,
            Some(Action::Snap {
                direction: Direction::Up,
                first_in_hold: true
            })
        );
    }

    #[test]
    fn super_with_other_key_does_not_launch() {
        let mut b = Bindings::default();
        b.key(SUPER, Key::Super, true, logo());
        assert_eq!(b.key(A, Key::Other, true, logo()), Outcome::forward(None));
        b.key(A, Key::Other, false, logo());
        assert_eq!(b.key(SUPER, Key::Super, false, none()).action, None);
    }

    #[test]
    fn super_with_pointer_click_does_not_launch() {
        let mut b = Bindings::default();
        b.key(SUPER, Key::Super, true, logo());
        b.pointer_button();
        assert_eq!(b.key(SUPER, Key::Super, false, none()).action, None);
    }

    #[test]
    fn super_after_other_modifier_does_not_launch() {
        let mut b = Bindings::default();
        let ctrl_logo = Mods {
            ctrl: true,
            logo: true,
            ..Mods::default()
        };
        b.key(SUPER, Key::Super, true, ctrl_logo);
        assert_eq!(b.key(SUPER, Key::Super, false, none()).action, None);
    }

    #[test]
    fn alt_tab_switches_and_alt_release_finishes() {
        let mut b = Bindings::default();
        b.key(ALT, Key::Alt, true, alt());
        assert_eq!(
            b.key(TAB, Key::Tab, true, alt()),
            Outcome::intercept(Some(Action::SwitchNext))
        );
        assert_eq!(b.key(TAB, Key::Tab, false, alt()), Outcome::intercept(None));
        assert_eq!(
            b.key(TAB, Key::Tab, true, alt()).action,
            Some(Action::SwitchNext)
        );
        assert_eq!(
            b.key(ALT, Key::Alt, false, none()),
            Outcome::forward(Some(Action::FinishSwitch))
        );
    }

    #[test]
    fn ctrl_alt_backspace_quits() {
        let mut b = Bindings::default();
        let ctrl_alt = Mods {
            ctrl: true,
            alt: true,
            ..Mods::default()
        };
        assert_eq!(
            b.key(BACKSPACE, Key::Backspace, true, ctrl_alt),
            Outcome::intercept(Some(Action::Quit))
        );
        assert_eq!(
            b.key(BACKSPACE, Key::Backspace, true, none()),
            Outcome::forward(None)
        );
    }

    #[test]
    fn vt_keysym_switches_vt() {
        let mut b = Bindings::default();
        let ctrl_alt = Mods {
            ctrl: true,
            alt: true,
            ..Mods::default()
        };
        assert_eq!(
            b.key(F2, Key::SwitchVt(2), true, ctrl_alt),
            Outcome::intercept(Some(Action::SwitchVt(2)))
        );
        assert_eq!(
            b.key(F2, Key::SwitchVt(2), false, ctrl_alt),
            Outcome::intercept(None)
        );
    }

    #[test]
    fn plain_keys_are_forwarded() {
        let mut b = Bindings::default();
        assert_eq!(b.key(TAB, Key::Tab, true, none()), Outcome::forward(None));
        assert_eq!(
            b.key(LEFT, Key::Arrow(Direction::Left), true, none()),
            Outcome::forward(None)
        );
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        assert_eq!(b.key(TAB, Key::Tab, true, ctrl), Outcome::forward(None));
    }
}
