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
    /// The MRU order when the session started, with each group of windows
    /// standing in as its most recently used member. It is frozen so that
    /// repeated Tab presses walk deeper instead of bouncing between two
    /// entries.
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

    /// All windows, most recently focused first.
    pub fn order(&self) -> &[WindowId] {
        &self.order
    }

    pub fn is_switching(&self) -> bool {
        self.switch.is_some()
    }

    /// The entries of the Alt+Tab session in progress, most recent first,
    /// and the index of the highlighted one. A group is its most recently
    /// used member.
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
    /// and returns the highlighted window. Windows for which `group_of`
    /// gives the same group share one entry.
    ///
    /// Returns `None` when there is nothing to switch to.
    pub fn step<G: PartialEq>(
        &mut self,
        forward: bool,
        group_of: impl Fn(WindowId) -> Option<G>,
    ) -> Option<WindowId> {
        if self.switch.is_none() {
            let snapshot = entries(&self.order, group_of);
            if snapshot.len() < 2 {
                return None;
            }
            self.switch = Some(Switch { snapshot, index: 0 });
        }
        let switch = self.switch.as_mut()?;
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

/// `order` with only the first, most recently used, member of each group.
fn entries<G: PartialEq>(
    order: &[WindowId],
    group_of: impl Fn(WindowId) -> Option<G>,
) -> Vec<WindowId> {
    let mut seen = Vec::new();
    order
        .iter()
        .copied()
        .filter(|&id| match group_of(id) {
            Some(group) if seen.contains(&group) => false,
            Some(group) => {
                seen.push(group);
                true
            }
            None => true,
        })
        .collect()
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

    fn ungrouped(_: WindowId) -> Option<u64> {
        None
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
        assert_eq!(h.step(true, ungrouped), None);
        assert!(!h.is_switching());
    }

    #[test]
    fn quick_alt_tab_toggles_between_two_most_recent() {
        let mut h = history(&[1, 2, 3]);

        assert_eq!(h.step(true, ungrouped), Some(WindowId(2)));
        h.finish_switch();
        assert_eq!(order(&h), [2, 1, 3]);

        assert_eq!(h.step(true, ungrouped), Some(WindowId(1)));
        h.finish_switch();
        assert_eq!(order(&h), [1, 2, 3]);
    }

    #[test]
    fn holding_alt_steps_deeper_and_wraps() {
        let mut h = history(&[1, 2, 3]);
        assert_eq!(h.step(true, ungrouped), Some(WindowId(2)));
        assert_eq!(h.step(true, ungrouped), Some(WindowId(3)));
        // Order is frozen while switching.
        assert_eq!(order(&h), [1, 2, 3]);
        assert_eq!(h.step(true, ungrouped), Some(WindowId(1)));
        assert_eq!(h.step(true, ungrouped), Some(WindowId(2)));
    }

    #[test]
    fn release_commits_selection() {
        let mut h = history(&[1, 2, 3]);
        h.step(true, ungrouped);
        h.step(true, ungrouped);
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
        h.step(true, ungrouped);
        h.step(true, ungrouped);
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
        assert_eq!(h.step(false, ungrouped), Some(WindowId(3)));
        assert_eq!(h.step(false, ungrouped), Some(WindowId(2)));
        assert_eq!(h.step(true, ungrouped), Some(WindowId(3)));
    }

    #[test]
    fn cancel_keeps_order() {
        let mut h = history(&[1, 2, 3]);
        h.step(true, ungrouped);
        h.cancel_switch();
        assert!(!h.is_switching());
        h.finish_switch();
        assert_eq!(order(&h), [1, 2, 3]);
    }

    #[test]
    fn promote_cancels_switch() {
        let mut h = history(&[1, 2, 3]);
        h.step(true, ungrouped);
        h.promote(WindowId(3));
        assert!(!h.is_switching());
        assert_eq!(order(&h), [3, 1, 2]);
    }

    #[test]
    fn a_group_is_one_entry_at_its_most_recent_member() {
        // Windows 2 and 4 are one group, 3 and 5 another.
        let group_of = |id: WindowId| match id.0 {
            2 | 4 => Some('a'),
            3 | 5 => Some('b'),
            _ => None,
        };
        let mut h = history(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(h.step(true, group_of), Some(WindowId(2)));
        assert_eq!(h.switch_list().unwrap().0, [1, 2, 3, 6].map(WindowId));
        assert_eq!(h.step(true, group_of), Some(WindowId(3)));
        h.finish_switch();
        assert_eq!(order(&h), [3, 1, 2, 4, 5, 6]);
    }

    #[test]
    fn a_single_group_does_nothing() {
        let mut h = history(&[1, 2]);
        assert_eq!(h.step(true, |_| Some(())), None);
        assert!(!h.is_switching());
    }
}
