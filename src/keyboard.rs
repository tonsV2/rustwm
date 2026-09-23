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
    /// Tab, or Shift+Tab (ISO_Left_Tab).
    Tab,
    Escape,
    Arrow(Direction),
    Backspace,
    /// One of the XF86Switch_VT_* keysyms, which the default keymap
    /// produces for Ctrl+Alt+F1..F12.
    SwitchVt(i32),
    /// A brightness or volume key, bound to an outside command.
    Command(Command),
    Other,
}

/// Brightness and volume keys. The compositor has no audio or power logic
/// of its own; each runs a configurable outside command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    BrightnessUp,
    BrightnessDown,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MicMute,
}

impl Command {
    /// Whether holding the key runs the command again at the key repeat
    /// rate. Toggles do not repeat, or a held key would flip back and forth.
    pub fn repeats(self) -> bool {
        !matches!(self, Command::VolumeMute | Command::MicMute)
    }
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
    /// Move the Alt+Tab highlight to the next (less recently used) window,
    /// or the previous one.
    Switch {
        forward: bool,
    },
    FinishSwitch,
    /// Close the switcher and keep the current window.
    CancelSwitch,
    /// Change to another virtual terminal (DRM backend only).
    SwitchVt(i32),
    /// Run the outside command bound to a brightness or volume key.
    Run(Command),
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
    /// Whether an Alt+Tab switch is in progress. The caller sets this before
    /// each key, since a switch can also end outside the bindings, for
    /// example when a click focuses a window.
    pub switching: bool,
    /// Whether the screen is locked. The caller sets this before each key.
    /// While locked, every key goes to the lock screen except the ones in
    /// [`locked_action`].
    pub locked: bool,
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
        if self.locked {
            self.super_tap = false;
            return self.consume(keycode, locked_action(key));
        }
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
            Key::Tab if mods.alt && !mods.logo && !mods.ctrl => Some(Action::Switch {
                forward: !mods.shift,
            }),
            Key::Arrow(direction) if self.switching => Some(Action::Switch {
                forward: matches!(direction, Direction::Right | Direction::Down),
            }),
            Key::Escape if self.switching => Some(Action::CancelSwitch),
            Key::Backspace if mods.ctrl && mods.alt && !mods.logo && !mods.shift => {
                Some(Action::Quit)
            }
            Key::SwitchVt(vt) => Some(Action::SwitchVt(vt)),
            // With any modifiers, and while switching, since these keys
            // mean nothing else.
            Key::Command(command) => Some(Action::Run(command)),
            _ => None,
        };
        self.consume(keycode, action)
    }

    /// Intercepts a press that triggers `action`, and remembers to intercept
    /// its release too. Presses without an action reach the client.
    fn consume(&mut self, keycode: u32, action: Option<Action>) -> Outcome {
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
        if self.locked {
            return Outcome::forward(None);
        }
        let action = match key {
            Key::Super if std::mem::take(&mut self.super_tap) => Some(Action::Launch),
            Key::Alt => Some(Action::FinishSwitch),
            _ => None,
        };
        Outcome::forward(action)
    }
}

/// The only bindings that work while the screen is locked. Switching to a
/// text console is safe, since it has its own login, and lets the user
/// recover from a broken lock screen. The brightness and volume keys only
/// run their outside commands. Everything else, including
/// Ctrl+Alt+Backspace, would get around the lock and goes to the lock
/// screen instead.
fn locked_action(key: Key) -> Option<Action> {
    match key {
        Key::SwitchVt(vt) => Some(Action::SwitchVt(vt)),
        Key::Command(command) => Some(Action::Run(command)),
        _ => None,
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
    const ESCAPE: u32 = 9;

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

    const NEXT: Option<Action> = Some(Action::Switch { forward: true });
    const PREV: Option<Action> = Some(Action::Switch { forward: false });

    #[test]
    fn alt_tab_switches_and_alt_release_finishes() {
        let mut b = Bindings::default();
        b.key(ALT, Key::Alt, true, alt());
        assert_eq!(b.key(TAB, Key::Tab, true, alt()), Outcome::intercept(NEXT));
        assert_eq!(b.key(TAB, Key::Tab, false, alt()), Outcome::intercept(None));
        assert_eq!(b.key(TAB, Key::Tab, true, alt()).action, NEXT);
        assert_eq!(
            b.key(ALT, Key::Alt, false, none()),
            Outcome::forward(Some(Action::FinishSwitch))
        );
    }

    #[test]
    fn shift_tab_and_arrows_move_the_highlight() {
        let mut b = Bindings::default();
        let alt_shift = Mods {
            alt: true,
            shift: true,
            ..Mods::default()
        };
        assert_eq!(b.key(TAB, Key::Tab, true, alt_shift).action, PREV);

        // Arrows and Escape only mean something while switching.
        let up = Key::Arrow(Direction::Up);
        assert_eq!(b.key(UP, up, true, alt()), Outcome::forward(None));
        assert_eq!(
            b.key(ESCAPE, Key::Escape, true, alt()),
            Outcome::forward(None)
        );

        b.switching = true;
        assert_eq!(b.key(UP, up, true, alt()), Outcome::intercept(PREV));
        let right = Key::Arrow(Direction::Right);
        assert_eq!(b.key(LEFT, right, true, alt()).action, NEXT);
        assert_eq!(
            b.key(ESCAPE, Key::Escape, true, alt()),
            Outcome::intercept(Some(Action::CancelSwitch))
        );
        assert_eq!(
            b.key(ESCAPE, Key::Escape, false, alt()),
            Outcome::intercept(None)
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

    const VOLUME_UP: u32 = 123;
    const MUTE: u32 = 121;

    #[test]
    fn command_keys_run_with_any_modifiers() {
        let mut b = Bindings::default();
        let up = Key::Command(Command::VolumeUp);
        let run = Outcome::intercept(Some(Action::Run(Command::VolumeUp)));
        assert_eq!(b.key(VOLUME_UP, up, true, none()), run);
        assert_eq!(
            b.key(VOLUME_UP, up, false, none()),
            Outcome::intercept(None)
        );
        let everything = Mods {
            logo: true,
            alt: true,
            ctrl: true,
            shift: true,
        };
        assert_eq!(b.key(VOLUME_UP, up, true, everything), run);
    }

    #[test]
    fn command_key_does_not_end_a_switch_or_launch() {
        let mut b = Bindings::default();
        let mute = Key::Command(Command::VolumeMute);
        b.key(ALT, Key::Alt, true, alt());
        b.key(TAB, Key::Tab, true, alt());
        b.switching = true;
        assert_eq!(
            b.key(MUTE, mute, true, alt()).action,
            Some(Action::Run(Command::VolumeMute))
        );
        b.key(MUTE, mute, false, alt());
        assert_eq!(b.key(TAB, Key::Tab, true, alt()).action, NEXT);
        assert_eq!(
            b.key(ALT, Key::Alt, false, none()).action,
            Some(Action::FinishSwitch)
        );

        // Super held while changing the volume is a modifier, not a tap.
        b.key(SUPER, Key::Super, true, logo());
        b.key(MUTE, mute, true, logo());
        b.key(MUTE, mute, false, logo());
        assert_eq!(b.key(SUPER, Key::Super, false, none()).action, None);
    }

    #[test]
    fn only_up_and_down_commands_repeat() {
        assert!(Command::BrightnessUp.repeats());
        assert!(Command::BrightnessDown.repeats());
        assert!(Command::VolumeUp.repeats());
        assert!(Command::VolumeDown.repeats());
        assert!(!Command::VolumeMute.repeats());
        assert!(!Command::MicMute.repeats());
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

    fn ctrl_alt() -> Mods {
        Mods {
            ctrl: true,
            alt: true,
            ..Mods::default()
        }
    }

    #[test]
    fn locked_forwards_window_management_keys() {
        let mut b = Bindings {
            locked: true,
            ..Bindings::default()
        };
        let left = Key::Arrow(Direction::Left);
        let presses = [
            (SUPER, Key::Super, logo()),
            (LEFT, left, logo()),
            (TAB, Key::Tab, alt()),
            (ESCAPE, Key::Escape, alt()),
            (BACKSPACE, Key::Backspace, ctrl_alt()),
            (A, Key::Other, none()),
        ];
        for (keycode, key, mods) in presses {
            assert_eq!(b.key(keycode, key, true, mods), Outcome::forward(None));
            assert_eq!(b.key(keycode, key, false, mods), Outcome::forward(None));
        }
        // Not even while a switch seemed to be in progress.
        b.switching = true;
        assert_eq!(b.key(LEFT, left, true, alt()), Outcome::forward(None));
        assert_eq!(b.key(ALT, Key::Alt, false, none()), Outcome::forward(None));
    }

    #[test]
    fn locked_allows_vt_switch_and_command_keys() {
        let mut b = Bindings {
            locked: true,
            ..Bindings::default()
        };
        assert_eq!(
            b.key(F2, Key::SwitchVt(2), true, ctrl_alt()),
            Outcome::intercept(Some(Action::SwitchVt(2)))
        );
        assert_eq!(
            b.key(F2, Key::SwitchVt(2), false, ctrl_alt()),
            Outcome::intercept(None)
        );
        let up = Key::Command(Command::VolumeUp);
        assert_eq!(
            b.key(VOLUME_UP, up, true, none()),
            Outcome::intercept(Some(Action::Run(Command::VolumeUp)))
        );
        assert_eq!(
            b.key(VOLUME_UP, up, false, none()),
            Outcome::intercept(None)
        );
    }

    #[test]
    fn super_held_across_lock_does_not_launch() {
        let mut b = Bindings::default();
        b.key(SUPER, Key::Super, true, logo());
        b.locked = true;
        assert_eq!(
            b.key(SUPER, Key::Super, false, none()),
            Outcome::forward(None)
        );
        // Nor does a tap that started while locked, released after unlock.
        b.key(SUPER, Key::Super, true, logo());
        b.locked = false;
        assert_eq!(b.key(SUPER, Key::Super, false, none()).action, None);
    }

    #[test]
    fn key_pressed_before_lock_has_its_release_consumed() {
        let mut b = Bindings::default();
        let left = Key::Arrow(Direction::Left);
        b.key(SUPER, Key::Super, true, logo());
        assert!(b.key(LEFT, left, true, logo()).action.is_some());
        b.locked = true;
        assert_eq!(b.key(LEFT, left, false, logo()), Outcome::intercept(None));
    }
}
