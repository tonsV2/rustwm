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
    Delete,
    Home,
    End,
    F4,
    Return,
    /// A letter key, lowercase: `'l'` for Super+l.
    Char(char),
    /// A digit key on the top row, 1 to 9.
    Digit(u8),
    /// One of the XF86Switch_VT_* keysyms, which the default keymap
    /// produces for Ctrl+Alt+F1..F12.
    SwitchVt(i32),
    /// A brightness, volume or Print key, bound to an outside command.
    Command(Command),
    Other,
}

/// Brightness, volume and Print keys. The compositor has no audio, power
/// or screenshot logic of its own; each runs a configurable outside command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    BrightnessUp,
    BrightnessDown,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MicMute,
    Screenshot,
}

impl Command {
    /// Whether holding the key runs the command again at the key repeat
    /// rate. Toggles do not repeat, or a held key would flip back and forth,
    /// and neither does a screenshot.
    pub fn repeats(self) -> bool {
        !matches!(
            self,
            Command::VolumeMute | Command::MicMute | Command::Screenshot
        )
    }
}

/// Applications started with a Super shortcut, each through a configurable
/// command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum App {
    Terminal,
    FileManager,
    Browser,
    Lock,
    ScreenshotArea,
}

impl App {
    /// The app Super plus `key`, and Shift if `shift`, starts.
    pub fn for_key(key: Key, shift: bool) -> Option<Self> {
        match (key, shift) {
            (Key::Return, false) => Some(App::Terminal),
            (Key::Char('f'), false) => Some(App::FileManager),
            (Key::Char('b'), false) => Some(App::Browser),
            (Key::Char('l'), false) => Some(App::Lock),
            (Key::Char('s'), true) => Some(App::ScreenshotArea),
            _ => None,
        }
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
    /// Open the overview, or close it if it is open (a Super tap).
    ToggleOverview,
    /// Close the overview without doing anything (Escape).
    CloseOverview,
    /// Focus the highlighted window or start the highlighted search result
    /// in the overview (Return).
    OverviewActivate,
    /// Move the overview's highlight.
    OverviewMove(Direction),
    /// Edit the overview's search text.
    OverviewEdit(Edit),
    /// Add the key's character to the overview's search. The caller reads
    /// the character from the key.
    OverviewType,
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
    /// Move the Alt+Tab highlight a row down or up the switcher's grid.
    SwitchRow {
        down: bool,
    },
    FinishSwitch,
    /// Close the switcher and keep the current window.
    CancelSwitch,
    /// Change to another virtual terminal (DRM backend only).
    SwitchVt(i32),
    /// Run the outside command bound to a brightness, volume or Print key.
    Run(Command),
    /// Start an application with a Super shortcut.
    Spawn(App),
    /// Ask the focused window to close (Super+q).
    Close,
    /// Focus the nth window in the order they were opened, from 1 (Super+1
    /// to Super+9).
    FocusNth(usize),
    /// Group the snapped windows on screen (Super+g).
    Group,
    /// Take the focused window out of its group (Super+Shift+g).
    Ungroup,
    /// Focus the next (or previous) window of the focused window's group.
    CycleGroup {
        forward: bool,
    },
    /// Turn the focused window's keyboard shortcuts inhibitor off, or on
    /// again (Super+Escape).
    ToggleShortcutsInhibit,
    /// Stop the compositor. On real hardware this is the way out when
    /// something goes wrong.
    Quit,
}

/// An edit of the overview's search text at its caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Delete the character before the caret (Backspace).
    Backspace,
    /// Delete the character after the caret (Delete).
    Delete,
    /// Delete back to the start of the word (Ctrl+Backspace, Ctrl+w).
    DeleteWordBack,
    /// Delete forward to the end of the word (Ctrl+Delete).
    DeleteWordForward,
    /// Delete everything (Ctrl+u, or Escape with text).
    Clear,
    Left,
    Right,
    /// To the start of the word (Ctrl+Left).
    WordLeft,
    /// To the end of the word (Ctrl+Right).
    WordRight,
    Home,
    End,
}

impl Action {
    /// Whether holding the key repeats the action at the key repeat rate.
    /// Toggles do not repeat, or a held key would flip back and forth, and
    /// neither does a screenshot or anything that closes the overview.
    pub fn repeats(self) -> bool {
        match self {
            Action::Run(command) => command.repeats(),
            Action::OverviewMove(_) | Action::OverviewEdit(_) | Action::OverviewType => true,
            _ => false,
        }
    }
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
    /// toggles the overview. Deciding on release avoids any timeout.
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
    /// Whether the focused window is in a group. The caller sets this
    /// before each key; Super+Tab reaches the client otherwise.
    pub grouped: bool,
    /// Whether the screen is locked. The caller sets this before each key.
    /// While locked, every key goes to the lock screen except the ones in
    /// [`locked_action`].
    pub locked: bool,
    /// Whether the overview is open. The caller sets this before each key.
    /// While open, no key reaches a window: they type into the search, move
    /// the highlight, are Super shortcuts, or are the few in
    /// [`overview_action`].
    pub overview: bool,
    /// Whether the overview's search has text. The caller sets this before
    /// each key; with text, Left, Right and Escape work on the text.
    pub overview_text: bool,
    /// Whether the focused window inhibits rustwm's shortcuts, as a virtual
    /// machine does. The caller sets this before each key. While set, every
    /// key reaches the window except the ones in [`inhibited_action`].
    pub inhibited: bool,
    /// Whether the focused window has a shortcuts inhibitor, active or not,
    /// so that Super+Escape has one to turn off or on. The caller sets this
    /// before each key; without one, Super+Escape reaches the window.
    pub has_inhibitor: bool,
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
    /// modifier, so it must not open the overview on release.
    pub fn pointer_button(&mut self) {
        self.super_tap = false;
    }

    fn press(&mut self, keycode: u32, key: Key, mods: Mods) -> Outcome {
        if self.locked {
            self.super_tap = false;
            return self.consume(keycode, locked_action(key));
        }
        if self.inhibited {
            // Super goes to the window, so its release must not open the
            // overview.
            self.super_tap = false;
            return self.consume(keycode, inhibited_action(key, mods));
        }
        if key == Key::Super {
            self.super_tap = !(mods.ctrl || mods.alt || mods.shift);
            self.first_in_hold = true;
            return Outcome::forward(None);
        }
        self.super_tap = false;
        if self.overview {
            self.suppressed.insert(keycode);
            let action = self
                .super_shortcut(key, mods)
                .or_else(|| overview_action(key, mods, self.overview_text));
            return Outcome::intercept(action);
        }
        if let Some(action) = self.super_shortcut(key, mods) {
            return self.consume(keycode, Some(action));
        }
        if self.has_inhibitor && is_super_escape(key, mods) {
            return self.consume(keycode, Some(Action::ToggleShortcutsInhibit));
        }

        let action = match key {
            Key::Tab if mods.alt && !mods.logo && !mods.ctrl => Some(Action::Switch {
                forward: !mods.shift,
            }),
            Key::Tab if mods.logo && !mods.alt && !mods.ctrl && self.grouped => {
                Some(Action::CycleGroup {
                    forward: !mods.shift,
                })
            }
            Key::Arrow(Direction::Up) if self.switching => Some(Action::SwitchRow { down: false }),
            Key::Arrow(Direction::Down) if self.switching => Some(Action::SwitchRow { down: true }),
            Key::Arrow(direction) if self.switching => Some(Action::Switch {
                forward: direction == Direction::Right,
            }),
            Key::Escape if self.switching => Some(Action::CancelSwitch),
            Key::F4 if mods.alt && !mods.ctrl && !mods.logo && !mods.shift => Some(Action::Close),
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

    /// What Super plus `key`, with or without Shift, does: snap, close,
    /// group, focus by number or start an app. These work the same on the
    /// desktop and in the overview.
    fn super_shortcut(&mut self, key: Key, mods: Mods) -> Option<Action> {
        if !mods.logo || mods.alt || mods.ctrl {
            return None;
        }
        match key {
            Key::Arrow(direction) if !mods.shift => {
                let first_in_hold = self.first_in_hold;
                self.first_in_hold = false;
                Some(Action::Snap {
                    direction,
                    first_in_hold,
                })
            }
            Key::Char('g') if mods.shift => Some(Action::Ungroup),
            key if mods.shift => App::for_key(key, true).map(Action::Spawn),
            key => super_action(key),
        }
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
        // Focus can move to an inhibiting window while Super is held; its
        // release then belongs to that window.
        if self.inhibited {
            self.super_tap = false;
        }
        let action = match key {
            Key::Super if std::mem::take(&mut self.super_tap) => Some(Action::ToggleOverview),
            Key::Alt => Some(Action::FinishSwitch),
            _ => None,
        };
        Outcome::forward(action)
    }
}

/// What Super plus `key`, with no other modifier, does.
fn super_action(key: Key) -> Option<Action> {
    if let Some(app) = App::for_key(key, false) {
        return Some(Action::Spawn(app));
    }
    match key {
        Key::Char('q') => Some(Action::Close),
        Key::Char('g') => Some(Action::Group),
        Key::Digit(n) => Some(Action::FocusNth(n as usize)),
        _ => None,
    }
}

/// What a key does while the overview is open. Keys with a letter or symbol
/// type into the search, which edits like a text field: Ctrl with
/// Backspace, Delete, Left and Right works on words, Ctrl+w and Ctrl+u
/// delete a word and the line as in a shell. With `text` in the field, Left
/// and Right move the caret and Escape clears the text; otherwise they move
/// the highlight and close the overview. The Super shortcuts are handled
/// before this, in [`Bindings::super_shortcut`]. Other Ctrl, Alt and Super
/// combinations do nothing, except the ones that work everywhere:
/// Ctrl+Alt+Backspace, the virtual terminal keys and the brightness and
/// volume keys.
fn overview_action(key: Key, mods: Mods, text: bool) -> Option<Action> {
    let plain = !(mods.ctrl || mods.alt || mods.logo);
    let ctrl = mods.ctrl && !(mods.alt || mods.logo);
    let edit = |edit: Edit| Some(Action::OverviewEdit(edit));
    match key {
        Key::SwitchVt(vt) => Some(Action::SwitchVt(vt)),
        Key::Command(command) => Some(Action::Run(command)),
        Key::Backspace if mods.ctrl && mods.alt => Some(Action::Quit),
        Key::Backspace if ctrl => edit(Edit::DeleteWordBack),
        Key::Delete if ctrl => edit(Edit::DeleteWordForward),
        Key::Arrow(Direction::Left) if ctrl => edit(Edit::WordLeft),
        Key::Arrow(Direction::Right) if ctrl => edit(Edit::WordRight),
        Key::Char('w') if ctrl => edit(Edit::DeleteWordBack),
        Key::Char('u') if ctrl => edit(Edit::Clear),
        _ if !plain => None,
        Key::Escape if text => edit(Edit::Clear),
        Key::Escape => Some(Action::CloseOverview),
        Key::Return => Some(Action::OverviewActivate),
        Key::Arrow(Direction::Left) if text => edit(Edit::Left),
        Key::Arrow(Direction::Right) if text => edit(Edit::Right),
        Key::Arrow(direction) => Some(Action::OverviewMove(direction)),
        Key::Tab if mods.shift => Some(Action::OverviewMove(Direction::Left)),
        Key::Tab => Some(Action::OverviewMove(Direction::Right)),
        Key::Backspace => edit(Edit::Backspace),
        Key::Delete => edit(Edit::Delete),
        Key::Home => edit(Edit::Home),
        Key::End => edit(Edit::End),
        Key::Char(_) | Key::Digit(_) | Key::Other => Some(Action::OverviewType),
        Key::Super | Key::Alt | Key::F4 => None,
    }
}

/// The only bindings that work while the screen is locked. Switching to a
/// text console is safe, since it has its own login, and lets the user
/// recover from a broken lock screen. The brightness and volume keys only
/// run their outside commands. Everything else, including
/// Ctrl+Alt+Backspace and Print (screen captures are refused while locked),
/// would get around the lock and goes to the lock screen instead.
fn locked_action(key: Key) -> Option<Action> {
    match key {
        Key::SwitchVt(vt) => Some(Action::SwitchVt(vt)),
        Key::Command(Command::Screenshot) => None,
        Key::Command(command) => Some(Action::Run(command)),
        _ => None,
    }
}

/// The only bindings that work while the focused window inhibits
/// shortcuts: the ways out, so a misbehaving client cannot trap the user.
/// `Super+Escape` gives rustwm its shortcuts back; Ctrl+Alt+Backspace, the
/// virtual terminal keys and the brightness, volume and mic mute keys work
/// as ever. Print goes to the window, for the desktop inside it.
fn inhibited_action(key: Key, mods: Mods) -> Option<Action> {
    match key {
        _ if is_super_escape(key, mods) => Some(Action::ToggleShortcutsInhibit),
        Key::Backspace if mods.ctrl && mods.alt && !mods.logo && !mods.shift => Some(Action::Quit),
        Key::SwitchVt(vt) => Some(Action::SwitchVt(vt)),
        Key::Command(Command::Screenshot) => None,
        Key::Command(command) => Some(Action::Run(command)),
        _ => None,
    }
}

/// Super+Escape, which turns the focused window's shortcuts inhibitor off
/// and on.
fn is_super_escape(key: Key, mods: Mods) -> bool {
    key == Key::Escape && mods.logo && !(mods.alt || mods.ctrl || mods.shift)
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
    const L: u32 = 46;
    const F: u32 = 41;
    const B: u32 = 56;
    const Q: u32 = 24;
    const S: u32 = 39;
    const THREE: u32 = 12;
    const G: u32 = 42;
    const RETURN: u32 = 36;
    const BACKSPACE: u32 = 22;
    const F2: u32 = 68;
    const F4: u32 = 70;
    const ESCAPE: u32 = 9;
    const DELETE: u32 = 119;
    const HOME: u32 = 110;
    const END: u32 = 115;
    const W: u32 = 25;
    const U: u32 = 30;

    fn ctrl() -> Mods {
        Mods {
            ctrl: true,
            ..Mods::default()
        }
    }

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
            Outcome::forward(Some(Action::ToggleOverview))
        );
    }

    #[test]
    fn super_shortcuts_work_in_the_overview() {
        let mut b = Bindings {
            overview: true,
            ..Bindings::default()
        };
        let shift_logo = Mods {
            shift: true,
            logo: true,
            ..none()
        };
        b.key(SUPER, Key::Super, true, logo());
        let cases = [
            (RETURN, Key::Return, logo(), Action::Spawn(App::Terminal)),
            (L, Key::Char('l'), logo(), Action::Spawn(App::Lock)),
            (Q, Key::Char('q'), logo(), Action::Close),
            (G, Key::Char('g'), logo(), Action::Group),
            (G, Key::Char('g'), shift_logo, Action::Ungroup),
            (THREE, Key::Digit(3), logo(), Action::FocusNth(3)),
            (
                LEFT,
                Key::Arrow(Direction::Left),
                logo(),
                Action::Snap {
                    direction: Direction::Left,
                    first_in_hold: true,
                },
            ),
            (
                UP,
                Key::Arrow(Direction::Up),
                logo(),
                Action::Snap {
                    direction: Direction::Up,
                    first_in_hold: false,
                },
            ),
        ];
        for (keycode, key, mods, action) in cases {
            assert_eq!(
                b.key(keycode, key, true, mods),
                Outcome::intercept(Some(action)),
                "{key:?}"
            );
            assert_eq!(
                b.key(keycode, key, false, mods),
                Outcome::intercept(None),
                "{key:?}"
            );
        }
        // Super was used for shortcuts, so releasing it is not a tap.
        assert_eq!(
            b.key(SUPER, Key::Super, false, none()),
            Outcome::forward(None)
        );
    }

    #[test]
    fn overview_takes_every_key() {
        let mut b = Bindings {
            overview: true,
            ..Bindings::default()
        };
        let cases = [
            (A, Key::Char('a'), none(), Some(Action::OverviewType)),
            (THREE, Key::Digit(3), none(), Some(Action::OverviewType)),
            (
                A,
                Key::Other,
                Mods {
                    shift: true,
                    ..none()
                },
                Some(Action::OverviewType),
            ),
            (
                BACKSPACE,
                Key::Backspace,
                none(),
                Some(Action::OverviewEdit(Edit::Backspace)),
            ),
            (
                BACKSPACE,
                Key::Backspace,
                ctrl(),
                Some(Action::OverviewEdit(Edit::DeleteWordBack)),
            ),
            (
                DELETE,
                Key::Delete,
                none(),
                Some(Action::OverviewEdit(Edit::Delete)),
            ),
            (
                DELETE,
                Key::Delete,
                ctrl(),
                Some(Action::OverviewEdit(Edit::DeleteWordForward)),
            ),
            (
                HOME,
                Key::Home,
                none(),
                Some(Action::OverviewEdit(Edit::Home)),
            ),
            (END, Key::End, none(), Some(Action::OverviewEdit(Edit::End))),
            (
                W,
                Key::Char('w'),
                ctrl(),
                Some(Action::OverviewEdit(Edit::DeleteWordBack)),
            ),
            (
                U,
                Key::Char('u'),
                ctrl(),
                Some(Action::OverviewEdit(Edit::Clear)),
            ),
            (
                LEFT,
                Key::Arrow(Direction::Left),
                ctrl(),
                Some(Action::OverviewEdit(Edit::WordLeft)),
            ),
            (RETURN, Key::Return, none(), Some(Action::OverviewActivate)),
            (ESCAPE, Key::Escape, none(), Some(Action::CloseOverview)),
            (
                LEFT,
                Key::Arrow(Direction::Left),
                none(),
                Some(Action::OverviewMove(Direction::Left)),
            ),
            (
                TAB,
                Key::Tab,
                none(),
                Some(Action::OverviewMove(Direction::Right)),
            ),
            (
                TAB,
                Key::Tab,
                Mods {
                    shift: true,
                    ..none()
                },
                Some(Action::OverviewMove(Direction::Left)),
            ),
            // Other shortcuts do nothing, but their keys still do not reach a window.
            (TAB, Key::Tab, alt(), None),
            (TAB, Key::Tab, logo(), None),
            (
                L,
                Key::Char('l'),
                Mods {
                    ctrl: true,
                    ..none()
                },
                None,
            ),
            (F4, Key::F4, alt(), None),
            // The keys that work everywhere.
            (BACKSPACE, Key::Backspace, ctrl_alt(), Some(Action::Quit)),
            (F2, Key::SwitchVt(2), ctrl_alt(), Some(Action::SwitchVt(2))),
            (
                MUTE,
                Key::Command(Command::VolumeMute),
                none(),
                Some(Action::Run(Command::VolumeMute)),
            ),
        ];
        for (keycode, key, mods, action) in cases {
            assert_eq!(
                b.key(keycode, key, true, mods),
                Outcome::intercept(action),
                "{key:?}"
            );
            assert_eq!(
                b.key(keycode, key, false, mods),
                Outcome::intercept(None),
                "{key:?}"
            );
        }
        // With text in the field, Left and Right move the caret and Escape clears.
        b.overview_text = true;
        assert_eq!(
            b.key(LEFT, Key::Arrow(Direction::Left), true, none())
                .action,
            Some(Action::OverviewEdit(Edit::Left))
        );
        b.key(LEFT, Key::Arrow(Direction::Left), false, none());
        assert_eq!(
            b.key(ESCAPE, Key::Escape, true, none()).action,
            Some(Action::OverviewEdit(Edit::Clear))
        );
        b.key(ESCAPE, Key::Escape, false, none());
        assert_eq!(
            b.key(UP, Key::Arrow(Direction::Up), true, none()).action,
            Some(Action::OverviewMove(Direction::Up))
        );
        b.key(UP, Key::Arrow(Direction::Up), false, none());
        b.overview_text = false;
        // A Super tap closes the overview.
        b.key(SUPER, Key::Super, true, logo());
        assert_eq!(
            b.key(SUPER, Key::Super, false, none()).action,
            Some(Action::ToggleOverview)
        );
        // Super with another key is neither a tap nor typing.
        b.key(SUPER, Key::Super, true, logo());
        assert_eq!(
            b.key(A, Key::Char('a'), true, logo()),
            Outcome::intercept(None)
        );
        b.key(A, Key::Char('a'), false, logo());
        assert_eq!(b.key(SUPER, Key::Super, false, none()).action, None);
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
        assert_eq!(
            b.key(UP, up, true, alt()),
            Outcome::intercept(Some(Action::SwitchRow { down: false }))
        );
        let left = Key::Arrow(Direction::Left);
        assert_eq!(b.key(LEFT, left, true, alt()).action, PREV);
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
    const PRINT: u32 = 107;
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
    fn held_keys_repeat_in_the_overview_but_not_the_ones_that_close_it() {
        assert!(Action::OverviewMove(Direction::Down).repeats());
        assert!(Action::OverviewEdit(Edit::Backspace).repeats());
        assert!(Action::OverviewType.repeats());
        assert!(Action::Run(Command::VolumeUp).repeats());
        assert!(!Action::Run(Command::VolumeMute).repeats());
        assert!(!Action::ToggleOverview.repeats());
        assert!(!Action::CloseOverview.repeats());
        assert!(!Action::OverviewActivate.repeats());
    }

    #[test]
    fn only_up_and_down_commands_repeat() {
        assert!(Command::BrightnessUp.repeats());
        assert!(Command::BrightnessDown.repeats());
        assert!(Command::VolumeUp.repeats());
        assert!(Command::VolumeDown.repeats());
        assert!(!Command::VolumeMute.repeats());
        assert!(!Command::MicMute.repeats());
        assert!(!Command::Screenshot.repeats());
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
    fn super_shortcuts() {
        let mut b = Bindings::default();
        let cases = [
            (RETURN, Key::Return, Action::Spawn(App::Terminal)),
            (F, Key::Char('f'), Action::Spawn(App::FileManager)),
            (B, Key::Char('b'), Action::Spawn(App::Browser)),
            (Q, Key::Char('q'), Action::Close),
            (THREE, Key::Digit(3), Action::FocusNth(3)),
            (G, Key::Char('g'), Action::Group),
        ];
        let super_shift = Mods {
            shift: true,
            ..logo()
        };
        assert_eq!(
            b.key(S, Key::Char('s'), true, super_shift),
            Outcome::intercept(Some(Action::Spawn(App::ScreenshotArea)))
        );
        b.key(S, Key::Char('s'), false, super_shift);
        assert_eq!(
            b.key(S, Key::Char('s'), true, logo()),
            Outcome::forward(None)
        );
        for (keycode, key, action) in cases {
            assert_eq!(
                b.key(keycode, key, true, logo()),
                Outcome::intercept(Some(action))
            );
            assert_eq!(b.key(keycode, key, false, logo()), Outcome::intercept(None));
            // Without Super they are typing.
            assert_eq!(b.key(keycode, key, true, none()), Outcome::forward(None));
        }
    }

    #[test]
    fn super_shift_g_ungroups() {
        let mut b = Bindings::default();
        let super_shift = Mods {
            shift: true,
            ..logo()
        };
        assert_eq!(
            b.key(G, Key::Char('g'), true, super_shift),
            Outcome::intercept(Some(Action::Ungroup))
        );
        assert_eq!(
            b.key(G, Key::Char('g'), false, super_shift),
            Outcome::intercept(None)
        );
    }

    #[test]
    fn super_tab_cycles_only_in_a_group() {
        let mut b = Bindings::default();
        b.key(SUPER, Key::Super, true, logo());
        assert_eq!(b.key(TAB, Key::Tab, true, logo()), Outcome::forward(None));
        b.key(TAB, Key::Tab, false, logo());
        b.grouped = true;
        assert_eq!(
            b.key(TAB, Key::Tab, true, logo()),
            Outcome::intercept(Some(Action::CycleGroup { forward: true }))
        );
        assert_eq!(
            b.key(TAB, Key::Tab, false, logo()),
            Outcome::intercept(None)
        );
        let super_shift = Mods {
            shift: true,
            ..logo()
        };
        assert_eq!(
            b.key(TAB, Key::Tab, true, super_shift).action,
            Some(Action::CycleGroup { forward: false })
        );
        b.key(TAB, Key::Tab, false, super_shift);
        // Super was used as a modifier, so releasing it does not launch.
        assert_eq!(b.key(SUPER, Key::Super, false, none()).action, None);
        // Alt+Tab still switches.
        assert_eq!(
            b.key(TAB, Key::Tab, true, alt()).action,
            Some(Action::Switch { forward: true })
        );
    }

    #[test]
    fn alt_f4_closes() {
        let mut b = Bindings::default();
        assert_eq!(
            b.key(F4, Key::F4, true, alt()),
            Outcome::intercept(Some(Action::Close))
        );
        assert_eq!(b.key(F4, Key::F4, false, alt()), Outcome::intercept(None));
        assert_eq!(b.key(F4, Key::F4, true, none()), Outcome::forward(None));
    }

    #[test]
    fn super_l_locks() {
        let mut b = Bindings::default();
        b.key(SUPER, Key::Super, true, logo());
        let l = Key::Char('l');
        assert_eq!(
            b.key(L, l, true, logo()),
            Outcome::intercept(Some(Action::Spawn(App::Lock)))
        );
        assert_eq!(b.key(L, l, false, logo()), Outcome::intercept(None));
        // Super was used as a modifier, so releasing it does not launch.
        assert_eq!(
            b.key(SUPER, Key::Super, false, none()),
            Outcome::forward(None)
        );
        // A plain l, or with other modifiers, is typing.
        assert_eq!(b.key(L, l, true, none()), Outcome::forward(None));
        let super_shift = Mods {
            shift: true,
            ..logo()
        };
        assert_eq!(b.key(L, l, true, super_shift), Outcome::forward(None));
    }

    #[test]
    fn locked_forwards_window_management_keys() {
        let mut b = Bindings {
            locked: true,
            grouped: true,
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
            (L, Key::Char('l'), logo()),
            (Q, Key::Char('q'), logo()),
            (G, Key::Char('g'), logo()),
            (TAB, Key::Tab, logo()),
            (THREE, Key::Digit(3), logo()),
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
    fn inhibited_forwards_shortcuts_to_the_window() {
        let mut b = Bindings {
            inhibited: true,
            grouped: true,
            ..Bindings::default()
        };
        let left = Key::Arrow(Direction::Left);
        let presses = [
            (LEFT, left, logo()),
            (TAB, Key::Tab, alt()),
            (F4, Key::F4, alt()),
            (L, Key::Char('l'), logo()),
            (Q, Key::Char('q'), logo()),
            (G, Key::Char('g'), logo()),
            (TAB, Key::Tab, logo()),
            (THREE, Key::Digit(3), logo()),
            (RETURN, Key::Return, logo()),
        ];
        for (keycode, key, mods) in presses {
            assert_eq!(b.key(keycode, key, true, mods), Outcome::forward(None));
            assert_eq!(b.key(keycode, key, false, mods), Outcome::forward(None));
        }
        // A Super tap reaches the window and leaves the overview closed.
        assert_eq!(
            b.key(SUPER, Key::Super, true, none()),
            Outcome::forward(None)
        );
        assert_eq!(
            b.key(SUPER, Key::Super, false, none()),
            Outcome::forward(None)
        );
    }

    #[test]
    fn inhibited_keeps_the_ways_out() {
        let mut b = Bindings {
            inhibited: true,
            ..Bindings::default()
        };
        assert_eq!(
            b.key(ESCAPE, Key::Escape, true, logo()),
            Outcome::intercept(Some(Action::ToggleShortcutsInhibit))
        );
        assert_eq!(
            b.key(ESCAPE, Key::Escape, false, logo()),
            Outcome::intercept(None)
        );
        assert_eq!(
            b.key(ESCAPE, Key::Escape, true, none()),
            Outcome::forward(None)
        );
        assert_eq!(
            b.key(BACKSPACE, Key::Backspace, true, ctrl_alt()),
            Outcome::intercept(Some(Action::Quit))
        );
        assert_eq!(
            b.key(F2, Key::SwitchVt(2), true, ctrl_alt()),
            Outcome::intercept(Some(Action::SwitchVt(2)))
        );
        assert_eq!(
            b.key(F4, Key::Command(Command::VolumeUp), true, none()),
            Outcome::intercept(Some(Action::Run(Command::VolumeUp)))
        );
    }

    #[test]
    fn super_escape_turns_the_inhibitor_back_on() {
        let mut b = Bindings {
            has_inhibitor: true,
            ..Bindings::default()
        };
        assert_eq!(
            b.key(ESCAPE, Key::Escape, true, logo()),
            Outcome::intercept(Some(Action::ToggleShortcutsInhibit))
        );
    }

    #[test]
    fn super_escape_reaches_a_window_without_an_inhibitor() {
        let mut b = Bindings::default();
        assert_eq!(
            b.key(ESCAPE, Key::Escape, true, logo()),
            Outcome::forward(None)
        );
    }

    #[test]
    fn print_reaches_an_inhibiting_window() {
        let mut b = Bindings {
            inhibited: true,
            ..Bindings::default()
        };
        let print = Key::Command(Command::Screenshot);
        assert_eq!(b.key(F4, print, true, none()), Outcome::forward(None));
    }

    #[test]
    fn super_released_in_an_inhibiting_window_does_not_open_the_overview() {
        let mut b = Bindings::default();
        assert_eq!(
            b.key(SUPER, Key::Super, true, logo()),
            Outcome::forward(None)
        );
        // The focus moved to a window that inhibits shortcuts.
        b.inhibited = true;
        assert_eq!(
            b.key(SUPER, Key::Super, false, none()),
            Outcome::forward(None)
        );
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
        // Captures are refused while locked, so Print goes to the lock
        // screen rather than running grim.
        assert_eq!(
            b.key(PRINT, Key::Command(Command::Screenshot), true, none()),
            Outcome::forward(None)
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
