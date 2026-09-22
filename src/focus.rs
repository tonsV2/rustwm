//! Most-recently-used focus history and Alt+Tab switching.
//!
//! Pure logic with no Smithay types, so it can be unit tested in isolation.

/// Stable identifier the compositor assigns to each managed window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

/// Focus history, most recently focused window first.
#[derive(Debug, Default)]
pub struct FocusHistory {
    order: Vec<WindowId>,
    switch: Option<Switch>,
}

/// An Alt+Tab session in progress, from the first Tab until Alt is released.
#[derive(Debug)]
struct Switch {
    /// The MRU order when the session started. It is frozen so that repeated
    /// Tab presses walk deeper instead of bouncing between two windows.
    snapshot: Vec<WindowId>,
    index: usize,
}

impl FocusHistory {
    /// The window that should currently have focus.
    ///
    /// While Alt+Tab is held this stays the window focused before the
    /// switch; the highlighted entry only takes focus when Alt is released.
    pub fn current(&self) -> Option<WindowId> {
        self.order.first().copied()
    }

    pub fn is_switching(&self) -> bool {
        self.switch.is_some()
    }

    /// The windows of the Alt+Tab session in progress, most recent first,
    /// and the index of the highlighted one.
    pub fn switch_list(&self) -> Option<(&[WindowId], usize)> {
        self.switch
            .as_ref()
            .map(|switch| (switch.snapshot.as_slice(), switch.index))
    }

    /// Records that `id` was focused by something other than Alt+Tab,
    /// such as a new window appearing or a pointer click.
    ///
    /// This ends any Alt+Tab session, since the user picked a window
    /// another way.
    pub fn promote(&mut self, id: WindowId) {
        self.switch = None;
        self.move_to_front(id);
    }

    /// Moves the Alt+Tab highlight one entry forwards (towards less recently
    /// used windows) or backwards, starting a session if none is running,
    /// and returns the highlighted window.
    ///
    /// Returns `None` when there is nothing to switch to.
    pub fn step(&mut self, forward: bool) -> Option<WindowId> {
        if self.order.len() < 2 {
            return None;
        }
        let switch = self.switch.get_or_insert_with(|| Switch {
            snapshot: self.order.clone(),
            index: 0,
        });
        let len = switch.snapshot.len();
        switch.index = if forward {
            (switch.index + 1) % len
        } else {
            (switch.index + len - 1) % len
        };
        switch.snapshot.get(switch.index).copied()
    }

    /// Ends an Alt+Tab session without changing focus.
    pub fn cancel_switch(&mut self) {
        self.switch = None;
    }

    /// Ends an Alt+Tab session, moving the selected window to the front.
    pub fn finish_switch(&mut self) {
        if let Some(switch) = self.switch.take()
            && let Some(&id) = switch.snapshot.get(switch.index)
        {
            self.move_to_front(id);
        }
    }

    /// Forgets a destroyed or unmapped window.
    pub fn remove(&mut self, id: WindowId) {
        self.order.retain(|&w| w != id);

        if let Some(switch) = &mut self.switch {
            if let Some(pos) = switch.snapshot.iter().position(|&w| w == id) {
                switch.snapshot.remove(pos);
                if pos < switch.index {
                    switch.index -= 1;
                }
            }
            if switch.snapshot.len() < 2 {
                self.switch = None;
            } else if switch.index >= switch.snapshot.len() {
                switch.index = 0;
            }
        }
    }

    fn move_to_front(&mut self, id: WindowId) {
        self.order.retain(|&w| w != id);
        self.order.insert(0, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(ids: &[u64]) -> FocusHistory {
        // Promote in reverse so the first id ends up most recent.
        let mut h = FocusHistory::default();
        for &id in ids.iter().rev() {
            h.promote(WindowId(id));
        }
        h
    }

    fn order(h: &FocusHistory) -> Vec<u64> {
        h.order.iter().map(|w| w.0).collect()
    }

    fn selected(h: &FocusHistory) -> Option<u64> {
        h.switch_list().map(|(list, index)| list[index].0)
    }

    #[test]
    fn promote_moves_to_front() {
        let mut h = history(&[1, 2, 3]);
        h.promote(WindowId(3));
        assert_eq!(order(&h), [3, 1, 2]);
        assert_eq!(h.current(), Some(WindowId(3)));
    }

    #[test]
    fn single_window_does_nothing() {
        let mut h = history(&[1]);
        assert_eq!(h.step(true), None);
        assert!(!h.is_switching());
    }

    #[test]
    fn quick_alt_tab_toggles_between_two_most_recent() {
        let mut h = history(&[1, 2, 3]);

        assert_eq!(h.step(true), Some(WindowId(2)));
        h.finish_switch();
        assert_eq!(order(&h), [2, 1, 3]);

        assert_eq!(h.step(true), Some(WindowId(1)));
        h.finish_switch();
        assert_eq!(order(&h), [1, 2, 3]);
    }

    #[test]
    fn holding_alt_steps_deeper_and_wraps() {
        let mut h = history(&[1, 2, 3]);
        assert_eq!(h.step(true), Some(WindowId(2)));
        assert_eq!(h.step(true), Some(WindowId(3)));
        // Order is frozen while switching.
        assert_eq!(order(&h), [1, 2, 3]);
        assert_eq!(h.step(true), Some(WindowId(1)));
        assert_eq!(h.step(true), Some(WindowId(2)));
    }

    #[test]
    fn release_commits_selection() {
        let mut h = history(&[1, 2, 3]);
        h.step(true);
        h.step(true);
        // Focus stays put until the switch finishes.
        assert_eq!(h.current(), Some(WindowId(1)));
        assert_eq!(
            h.switch_list(),
            Some((&[WindowId(1), WindowId(2), WindowId(3)][..], 2))
        );
        h.finish_switch();
        assert!(!h.is_switching());
        assert_eq!(order(&h), [3, 1, 2]);
    }

    #[test]
    fn finish_without_switch_is_harmless() {
        let mut h = history(&[1, 2]);
        h.finish_switch();
        assert_eq!(order(&h), [1, 2]);
    }

    #[test]
    fn removing_focused_falls_back_to_next_most_recent() {
        let mut h = history(&[1, 2, 3]);
        h.remove(WindowId(1));
        assert_eq!(h.current(), Some(WindowId(2)));
        h.remove(WindowId(2));
        h.remove(WindowId(3));
        assert_eq!(h.current(), None);
    }

    #[test]
    fn removing_during_switch_keeps_selection_valid() {
        let mut h = history(&[1, 2, 3, 4]);
        h.step(true);
        h.step(true);
        assert_eq!(selected(&h), Some(3));

        // Removing an earlier entry keeps the same window selected.
        h.remove(WindowId(2));
        assert_eq!(selected(&h), Some(3));

        // Removing the selected one moves on to the next in the snapshot.
        h.remove(WindowId(3));
        assert_eq!(selected(&h), Some(4));

        // Removing the last entry leaves too few windows to switch between.
        h.remove(WindowId(4));
        assert!(!h.is_switching());
        assert_eq!(h.current(), Some(WindowId(1)));
    }

    #[test]
    fn shift_tab_steps_backwards_and_wraps() {
        let mut h = history(&[1, 2, 3]);
        assert_eq!(h.step(false), Some(WindowId(3)));
        assert_eq!(h.step(false), Some(WindowId(2)));
        assert_eq!(h.step(true), Some(WindowId(3)));
    }

    #[test]
    fn cancel_keeps_order() {
        let mut h = history(&[1, 2, 3]);
        h.step(true);
        h.cancel_switch();
        assert!(!h.is_switching());
        h.finish_switch();
        assert_eq!(order(&h), [1, 2, 3]);
    }

    #[test]
    fn promote_cancels_switch() {
        let mut h = history(&[1, 2, 3]);
        h.step(true);
        h.promote(WindowId(3));
        assert!(!h.is_switching());
        assert_eq!(order(&h), [3, 1, 2]);
    }
}
