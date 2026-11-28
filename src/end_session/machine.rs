//! The end-session dialog's state machine: what it shows, which button is
//! selected and what the compositor must do next. Pure logic with no
//! Smithay types; the time is passed in, so it can be unit tested.

use std::time::{Duration, Instant};

use crate::logind::{Availability, Inhibitor, PowerAction};

/// How long the dialog counts down before acting as if confirmed.
pub const COUNTDOWN: Duration = Duration::from_secs(60);

/// What the dialog confirms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndAction {
    LogOut,
    Restart,
    PowerOff,
}

impl EndAction {
    /// What logind is asked to do; `None` for Log Out, which only stops
    /// rustwm.
    pub fn power(self) -> Option<PowerAction> {
        match self {
            EndAction::LogOut => None,
            EndAction::Restart => Some(PowerAction::Reboot),
            EndAction::PowerOff => Some(PowerAction::PowerOff),
        }
    }

    /// The dialog's title, which is also its confirming button's label.
    pub fn title(self) -> &'static str {
        match self {
            EndAction::LogOut => "Log Out",
            EndAction::Restart => "Restart",
            EndAction::PowerOff => "Power Off",
        }
    }

    /// The line under the title, as GNOME words it.
    pub fn countdown_text(self, seconds: u32) -> String {
        let unit = if seconds == 1 { "second" } else { "seconds" };
        match self {
            EndAction::LogOut => {
                format!("You will be logged out automatically in {seconds} {unit}.")
            }
            EndAction::Restart => {
                format!("The system will restart automatically in {seconds} {unit}.")
            }
            EndAction::PowerOff => {
                format!("The system will power off automatically in {seconds} {unit}.")
            }
        }
    }

    /// The error dialog's title when logind refuses.
    pub fn failure_title(self) -> &'static str {
        match self {
            EndAction::LogOut => "Could not log out",
            EndAction::Restart => "Could not restart",
            EndAction::PowerOff => "Could not power off",
        }
    }
}

/// Why the user can no longer see the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interruption {
    Locked,
    /// Another virtual terminal or user took over the screen.
    Paused,
    /// The screen turned off, such as after a while without input.
    ScreenOff,
}

/// A button in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Cancel,
    Confirm(EndAction),
    /// Goes ahead although some windows are still open.
    Anyway(EndAction),
    /// Closes the error dialog.
    Close,
}

impl Button {
    pub fn label(self) -> String {
        match self {
            Button::Cancel => "Cancel".to_string(),
            Button::Confirm(action) => action.title().to_string(),
            Button::Anyway(action) => format!("{} Anyway", action.title()),
            Button::Close => "Close".to_string(),
        }
    }

    /// Whether the button acts rather than closing the dialog.
    pub fn goes_ahead(self) -> bool {
        matches!(self, Button::Confirm(_) | Button::Anyway(_))
    }
}

/// A confirmed Restart or Power Off waiting for logind to say whether it
/// is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checking {
    pub action: EndAction,
    /// The countdown, stopped where it was.
    pub seconds_left: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// The dialog in the middle of the screen, counting down.
    Confirm {
        action: EndAction,
        buttons: Vec<Button>,
        selected: usize,
        deadline: Instant,
        /// Programs blocking a shutdown, listed in the dialog.
        inhibitors: Vec<Inhibitor>,
        checking: Option<Checking>,
    },
    /// The windows were asked to close; nothing is shown for a few seconds
    /// so their save prompts can be answered at once.
    Closing {
        action: EndAction,
    },
    /// Some windows are still open after the grace time: a panel at the
    /// bottom lists them, with Cancel and an Anyway button.
    Blocked {
        action: EndAction,
        selected: usize,
    },
    /// logind was asked to restart or power off and has not answered.
    Acting {
        action: EndAction,
    },
    /// logind refused, or did not answer.
    Error {
        title: String,
        message: String,
    },
}

/// What the compositor must do after the machine moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// The dialog went away without acting.
    Cancelled,
    /// Ask logind whether the action is allowed before closing windows.
    Check(EndAction),
    /// Ask every window to close, then wait for them.
    CloseWindows(EndAction),
    /// Every window is gone, or the user said to go ahead anyway.
    Perform(EndAction),
}

#[derive(Debug)]
pub struct Machine {
    pub phase: Phase,
}

impl Default for Machine {
    fn default() -> Self {
        Self { phase: Phase::Idle }
    }
}

impl Machine {
    /// Shows the dialog for `action`, counting down from `now`, with the
    /// confirming button selected.
    pub fn open(&mut self, action: EndAction, availability: Availability, now: Instant) {
        let buttons = buttons(action, availability);
        self.phase = Phase::Confirm {
            action,
            selected: buttons.len() - 1,
            buttons,
            deadline: now + COUNTDOWN,
            inhibitors: Vec::new(),
            checking: None,
        };
    }

    /// The whole seconds the countdown shows at `now`, rounded up: 60 when
    /// it starts and 1 in the last second. `None` without a countdown.
    pub fn seconds_left(&self, now: Instant) -> Option<u32> {
        match &self.phase {
            Phase::Confirm {
                checking: Some(checking),
                ..
            } => Some(checking.seconds_left),
            Phase::Confirm { deadline, .. } => Some(whole_seconds_left(*deadline, now)),
            _ => None,
        }
    }

    /// Whether the countdown is running, so its timer must keep going.
    pub fn is_counting(&self) -> bool {
        matches!(&self.phase, Phase::Confirm { checking: None, .. })
    }

    /// Moves the countdown on: at zero the confirming button is pressed.
    pub fn tick(&mut self, now: Instant) -> Step {
        if !self.is_counting() || self.seconds_left(now) != Some(0) {
            return Step::Nothing;
        }
        match self.buttons().len().checked_sub(1) {
            Some(last) => self.press(last, now),
            None => Step::Nothing,
        }
    }

    /// The buttons shown, left to right; the confirming one is last.
    pub fn buttons(&self) -> Vec<Button> {
        match &self.phase {
            Phase::Confirm { buttons, .. } => buttons.clone(),
            Phase::Blocked { action, .. } => vec![Button::Cancel, Button::Anyway(*action)],
            Phase::Error { .. } => vec![Button::Close],
            Phase::Idle | Phase::Closing { .. } | Phase::Acting { .. } => Vec::new(),
        }
    }

    /// The selected button's index, which Return presses.
    pub fn selected(&self) -> usize {
        match &self.phase {
            Phase::Confirm { selected, .. } | Phase::Blocked { selected, .. } => *selected,
            _ => 0,
        }
    }

    /// Selects the next button, or the previous one, wrapping round.
    pub fn move_selection(&mut self, forward: bool) {
        let count = self.buttons().len();
        if count == 0 {
            return;
        }
        let selected = self.selected();
        let next = if forward {
            (selected + 1) % count
        } else {
            (selected + count - 1) % count
        };
        self.select(next);
    }

    /// Selects button `index`, such as the one under the pointer.
    pub fn select(&mut self, index: usize) {
        let count = self.buttons().len();
        match &mut self.phase {
            Phase::Confirm { selected, .. } | Phase::Blocked { selected, .. } if index < count => {
                *selected = index;
            }
            _ => {}
        }
    }

    pub fn press_selected(&mut self, now: Instant) -> Step {
        self.press(self.selected(), now)
    }

    /// Presses button `index` at `now`.
    pub fn press(&mut self, index: usize, now: Instant) -> Step {
        let Some(button) = self.buttons().get(index).copied() else {
            return Step::Nothing;
        };
        match button {
            Button::Cancel | Button::Close => self.cancel(),
            Button::Confirm(action) => self.confirm(action, now),
            Button::Anyway(action) => Step::Perform(action),
        }
    }

    fn confirm(&mut self, action: EndAction, now: Instant) -> Step {
        let seconds_left = self.seconds_left(now).unwrap_or(0);
        let Phase::Confirm { checking, .. } = &mut self.phase else {
            return Step::Nothing;
        };
        // Pressed again while logind is being asked.
        if checking.is_some() {
            return Step::Nothing;
        }
        if action.power().is_none() {
            return Step::CloseWindows(action);
        }
        *checking = Some(Checking {
            action,
            seconds_left,
        });
        Step::Check(action)
    }

    /// Closes the dialog without acting. Once logind has been asked to act,
    /// that cannot be taken back, so nothing changes then.
    pub fn cancel(&mut self) -> Step {
        match self.phase {
            Phase::Idle | Phase::Acting { .. } => Step::Nothing,
            _ => {
                self.phase = Phase::Idle;
                Step::Cancelled
            }
        }
    }

    /// logind's answer to whether `action` is allowed: if so, the windows
    /// are closed; if not, the error is shown before any window is.
    pub fn checked(&mut self, action: EndAction, result: Result<(), String>) -> Step {
        let Phase::Confirm {
            checking: Some(checking),
            ..
        } = &self.phase
        else {
            return Step::Nothing;
        };
        if checking.action != action {
            return Step::Nothing;
        }
        match result {
            Ok(()) => Step::CloseWindows(action),
            Err(message) => {
                self.failed(action.failure_title().to_string(), message);
                Step::Nothing
            }
        }
    }

    pub fn start_closing(&mut self, action: EndAction) {
        self.phase = Phase::Closing { action };
    }

    /// `count` windows are still open: with none left, the action goes ahead.
    pub fn windows_left(&mut self, count: usize) -> Step {
        match self.phase {
            Phase::Closing { action } | Phase::Blocked { action, .. } if count == 0 => {
                Step::Perform(action)
            }
            _ => Step::Nothing,
        }
    }

    /// The grace time after asking the windows to close is over: if some
    /// are still open, the panel listing them shows, with Anyway selected.
    pub fn grace_over(&mut self, count: usize) {
        if let Phase::Closing { action } = self.phase
            && count > 0
        {
            self.phase = Phase::Blocked {
                action,
                selected: 1,
            };
        }
    }

    /// The action is under way: logind was asked, or Log Out is stopping
    /// rustwm. Nothing cancels it and the windows are no longer waited for.
    pub fn acting(&mut self, action: EndAction) {
        self.phase = Phase::Acting { action };
    }

    /// Forgets the action, even one under way, such as when logind refused
    /// it while the screen was locked and there is nobody to tell.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
    }

    pub fn failed(&mut self, title: String, message: String) {
        self.phase = Phase::Error { title, message };
    }

    /// The user can no longer see the dialog, for `why`. Returns whether
    /// anything went away.
    ///
    /// The lock or another virtual terminal or user taking over cancels
    /// everything but a request logind already has: the countdown and the
    /// wait for windows too, so nothing acts for a user who is not there.
    /// The screen turning off cancels only the dialog that is counting down
    /// or showing an error. Waiting for windows survives it, since that
    /// only goes ahead when the user closes the last window, and dropping
    /// it would silently forget a confirmed action while the user answers
    /// a save prompt.
    pub fn interrupt(&mut self, why: Interruption) -> bool {
        if why == Interruption::ScreenOff && !self.is_modal() {
            return false;
        }
        self.cancel() == Step::Cancelled
    }

    pub fn set_inhibitors(&mut self, list: Vec<Inhibitor>) {
        if let Phase::Confirm { inhibitors, .. } = &mut self.phase {
            *inhibitors = list;
        }
    }

    /// Whether the dialog takes all input, as the overview does.
    pub fn is_modal(&self) -> bool {
        matches!(self.phase, Phase::Confirm { .. } | Phase::Error { .. })
    }

    /// Whether the machine waits for the windows to close.
    pub fn waiting_for_windows(&self) -> bool {
        matches!(self.phase, Phase::Closing { .. } | Phase::Blocked { .. })
    }
}

/// The confirmation dialog's buttons, as in GNOME: Cancel, then the action.
/// Power Off also offers Restart, when logind allows it.
pub fn buttons(action: EndAction, availability: Availability) -> Vec<Button> {
    let mut buttons = vec![Button::Cancel];
    if action == EndAction::PowerOff && availability.allows(PowerAction::Reboot) {
        buttons.push(Button::Confirm(EndAction::Restart));
    }
    buttons.push(Button::Confirm(action));
    buttons
}

/// The whole seconds from `now` to `deadline`, rounded up.
fn whole_seconds_left(deadline: Instant, now: Instant) -> u32 {
    let remaining = deadline.saturating_duration_since(now);
    remaining.as_nanos().div_ceil(1_000_000_000) as u32
}

/// How long until the countdown shows one second less: the part of the
/// remaining time past whole seconds, or a full second on a whole second.
pub fn until_next_tick(deadline: Instant, now: Instant) -> Duration {
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return Duration::ZERO;
    }
    let past_whole = remaining.as_nanos() % 1_000_000_000;
    if past_whole == 0 {
        Duration::from_secs(1)
    } else {
        Duration::from_nanos(past_whole as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Availability = Availability {
        suspend: true,
        reboot: true,
        power_off: true,
    };

    fn opened(action: EndAction) -> (Machine, Instant) {
        let now = Instant::now();
        let mut machine = Machine::default();
        machine.open(action, ALL, now);
        (machine, now)
    }

    #[test]
    fn buttons_follow_the_action_and_availability() {
        use EndAction::*;
        assert_eq!(
            buttons(LogOut, ALL),
            [Button::Cancel, Button::Confirm(LogOut)]
        );
        assert_eq!(
            buttons(Restart, ALL),
            [Button::Cancel, Button::Confirm(Restart)]
        );
        assert_eq!(
            buttons(PowerOff, ALL),
            [
                Button::Cancel,
                Button::Confirm(Restart),
                Button::Confirm(PowerOff)
            ]
        );
        let no_reboot = Availability {
            reboot: false,
            ..ALL
        };
        assert_eq!(
            buttons(PowerOff, no_reboot),
            [Button::Cancel, Button::Confirm(PowerOff)]
        );
        assert_eq!(Button::Anyway(Restart).label(), "Restart Anyway");
    }

    #[test]
    fn confirming_button_is_selected_and_selection_wraps() {
        let (mut machine, _) = opened(EndAction::PowerOff);
        assert_eq!(machine.selected(), 2);
        machine.move_selection(true);
        assert_eq!(machine.selected(), 0);
        machine.move_selection(false);
        assert_eq!(machine.selected(), 2);
        machine.select(1);
        assert_eq!(machine.selected(), 1);
        machine.select(7);
        assert_eq!(machine.selected(), 1);
    }

    #[test]
    fn countdown_shows_whole_seconds_rounded_up() {
        let (machine, now) = opened(EndAction::LogOut);
        let at = |secs: f64| machine.seconds_left(now + Duration::from_secs_f64(secs));
        assert_eq!(at(0.0), Some(60));
        assert_eq!(at(1.0), Some(59));
        assert_eq!(at(59.5), Some(1));
        assert_eq!(at(60.0), Some(0));
        assert_eq!(at(61.0), Some(0));
        assert_eq!(
            EndAction::LogOut.countdown_text(60),
            "You will be logged out automatically in 60 seconds."
        );
        assert_eq!(
            EndAction::PowerOff.countdown_text(1),
            "The system will power off automatically in 1 second."
        );
    }

    #[test]
    fn next_tick_is_at_the_next_whole_second() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(60);
        assert_eq!(until_next_tick(deadline, now), Duration::from_secs(1));
        assert_eq!(
            until_next_tick(deadline, now + Duration::from_millis(250)),
            Duration::from_millis(750)
        );
        assert_eq!(until_next_tick(deadline, deadline), Duration::ZERO);
    }

    #[test]
    fn countdown_at_zero_confirms() {
        let (mut machine, now) = opened(EndAction::LogOut);
        assert_eq!(machine.tick(now + Duration::from_secs(30)), Step::Nothing);
        assert_eq!(
            machine.tick(now + COUNTDOWN),
            Step::CloseWindows(EndAction::LogOut)
        );

        let (mut machine, now) = opened(EndAction::PowerOff);
        assert_eq!(
            machine.tick(now + COUNTDOWN),
            Step::Check(EndAction::PowerOff)
        );
        // The countdown stopped while logind is asked.
        assert!(!machine.is_counting());
        assert_eq!(machine.tick(now + COUNTDOWN), Step::Nothing);
    }

    #[test]
    fn restart_in_the_power_off_dialog_checks_restart() {
        let (mut machine, _) = opened(EndAction::PowerOff);
        assert_eq!(
            machine.press(1, Instant::now()),
            Step::Check(EndAction::Restart)
        );
        // Pressing again while checking does nothing; Cancel still works.
        assert_eq!(machine.press(2, Instant::now()), Step::Nothing);
        assert_eq!(machine.press_selected(Instant::now()), Step::Nothing);
        assert!(machine.seconds_left(Instant::now()).is_some());
        assert_eq!(machine.press(0, Instant::now()), Step::Cancelled);
        assert_eq!(machine.phase, Phase::Idle);
    }

    #[test]
    fn cancel_closes_the_dialog() {
        let (mut machine, _) = opened(EndAction::Restart);
        assert_eq!(machine.cancel(), Step::Cancelled);
        assert_eq!(machine.phase, Phase::Idle);
        assert_eq!(machine.cancel(), Step::Nothing);
    }

    #[test]
    fn check_result_closes_windows_or_shows_the_error() {
        let (mut machine, _) = opened(EndAction::Restart);
        machine.press_selected(Instant::now());
        assert_eq!(
            machine.checked(EndAction::Restart, Ok(())),
            Step::CloseWindows(EndAction::Restart)
        );

        let (mut machine, _) = opened(EndAction::Restart);
        machine.press_selected(Instant::now());
        assert_eq!(
            machine.checked(EndAction::Restart, Err("Access denied".into())),
            Step::Nothing
        );
        assert_eq!(
            machine.phase,
            Phase::Error {
                title: "Could not restart".into(),
                message: "Access denied".into()
            }
        );
        assert!(machine.is_modal());
        assert_eq!(machine.buttons(), [Button::Close]);
        assert_eq!(machine.press_selected(Instant::now()), Step::Cancelled);
    }

    #[test]
    fn closing_waits_for_the_windows() {
        let mut machine = Machine::default();
        machine.start_closing(EndAction::LogOut);
        assert!(machine.waiting_for_windows());
        assert!(!machine.is_modal());
        assert_eq!(machine.windows_left(2), Step::Nothing);
        assert_eq!(machine.windows_left(0), Step::Perform(EndAction::LogOut));

        machine.grace_over(2);
        assert_eq!(
            machine.phase,
            Phase::Blocked {
                action: EndAction::LogOut,
                selected: 1
            }
        );
        assert!(!machine.is_modal());
        assert_eq!(machine.windows_left(0), Step::Perform(EndAction::LogOut));
        assert_eq!(
            machine.press_selected(Instant::now()),
            Step::Perform(EndAction::LogOut)
        );
    }

    #[test]
    fn grace_over_with_no_windows_left_stays_closing() {
        let mut machine = Machine::default();
        machine.start_closing(EndAction::Restart);
        machine.grace_over(0);
        assert_eq!(
            machine.phase,
            Phase::Closing {
                action: EndAction::Restart
            }
        );
    }

    #[test]
    fn lock_and_pause_cancel_all_but_acting() {
        for why in [Interruption::Locked, Interruption::Paused] {
            let (mut machine, now) = opened(EndAction::PowerOff);
            assert!(machine.interrupt(why));
            assert_eq!(machine.phase, Phase::Idle);
            // The countdown is gone, so reaching zero does nothing.
            assert_eq!(machine.tick(now + COUNTDOWN), Step::Nothing);

            machine.start_closing(EndAction::LogOut);
            assert!(machine.interrupt(why));
            assert_eq!(machine.windows_left(0), Step::Nothing);

            machine.start_closing(EndAction::LogOut);
            machine.grace_over(1);
            assert!(machine.interrupt(why));
            // The last window closing later does not log out.
            assert_eq!(machine.windows_left(0), Step::Nothing);
            assert!(!machine.waiting_for_windows());

            machine.failed("Could not restart".into(), "No".into());
            assert!(machine.interrupt(why));

            machine.acting(EndAction::PowerOff);
            assert!(!machine.interrupt(why));
        }
    }

    #[test]
    fn screen_off_cancels_only_the_modal_dialog() {
        let off = Interruption::ScreenOff;
        let (mut machine, _) = opened(EndAction::PowerOff);
        assert!(machine.interrupt(off));
        assert_eq!(machine.phase, Phase::Idle);

        // Checking with logind is still the modal dialog.
        let (mut machine, now) = opened(EndAction::PowerOff);
        machine.press_selected(now);
        assert!(machine.interrupt(off));

        machine.failed("Could not restart".into(), "No".into());
        assert!(machine.interrupt(off));

        // Waiting for windows goes on, and still acts once they are gone.
        machine.start_closing(EndAction::PowerOff);
        assert!(!machine.interrupt(off));
        machine.grace_over(1);
        assert!(!machine.interrupt(off));
        assert_eq!(machine.windows_left(0), Step::Perform(EndAction::PowerOff));

        machine.acting(EndAction::PowerOff);
        assert!(!machine.interrupt(off));
        assert_eq!(machine.cancel(), Step::Nothing);
        assert_eq!(
            machine.phase,
            Phase::Acting {
                action: EndAction::PowerOff
            }
        );
    }

    #[test]
    fn logging_out_happens_once() {
        let mut machine = Machine::default();
        machine.start_closing(EndAction::LogOut);
        assert_eq!(machine.windows_left(0), Step::Perform(EndAction::LogOut));
        machine.acting(EndAction::LogOut);
        assert!(!machine.waiting_for_windows());
        assert_eq!(machine.windows_left(0), Step::Nothing);
        machine.reset();
        assert_eq!(machine.phase, Phase::Idle);
    }

    #[test]
    fn only_the_dialog_and_the_error_are_modal() {
        let mut machine = Machine::default();
        assert!(!machine.is_modal());
        machine.open(EndAction::LogOut, ALL, Instant::now());
        assert!(machine.is_modal());
        machine.start_closing(EndAction::LogOut);
        assert!(!machine.is_modal());
        machine.acting(EndAction::Restart);
        assert!(!machine.is_modal());
        machine.failed(String::new(), String::new());
        assert!(machine.is_modal());
    }

    #[test]
    fn inhibitors_are_kept_only_while_confirming() {
        let (mut machine, _) = opened(EndAction::PowerOff);
        let inhibitor = Inhibitor {
            what: "shutdown".into(),
            who: "Updates".into(),
            why: "Installing".into(),
            mode: "block".into(),
        };
        machine.set_inhibitors(vec![inhibitor.clone()]);
        let Phase::Confirm { inhibitors, .. } = &machine.phase else {
            panic!("not confirming");
        };
        assert_eq!(inhibitors, std::slice::from_ref(&inhibitor));
        machine.cancel();
        machine.set_inhibitors(vec![inhibitor]);
        assert_eq!(machine.phase, Phase::Idle);
    }
}
