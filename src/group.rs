//! Window groups: sets of snapped windows that come into view together and share one Alt+Tab entry.
//!
//! [`Groups`] is pure bookkeeping with no Smithay types, so it can be unit tested in isolation.

use tracing::info;

use crate::{
    focus::WindowId,
    layout::{Layout, visible_tiles},
    state::State,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(u64);

#[derive(Debug, Default)]
pub struct Groups {
    groups: Vec<Group>,
    next_id: u64,
}

#[derive(Debug)]
struct Group {
    id: GroupId,
    /// In the order they joined.
    members: Vec<WindowId>,
}

impl Groups {
    pub fn group_of(&self, window: WindowId) -> Option<GroupId> {
        self.groups
            .iter()
            .find(|g| g.members.contains(&window))
            .map(|g| g.id)
    }

    pub fn members(&self, group: GroupId) -> &[WindowId] {
        self.groups
            .iter()
            .find(|g| g.id == group)
            .map_or(&[], |g| g.members.as_slice())
    }

    /// Puts `windows` in one group. Groups some of them are already in are merged into it, keeping the oldest group's id. Returns `None`, changing nothing, when that would leave a group of fewer than two windows.
    pub fn group(&mut self, windows: &[WindowId]) -> Option<GroupId> {
        let mut merged: Vec<GroupId> = windows.iter().filter_map(|&w| self.group_of(w)).collect();
        merged.sort_by_key(|g| g.0);
        merged.dedup();
        let mut members: Vec<WindowId> = merged
            .iter()
            .flat_map(|&g| self.members(g).iter().copied())
            .collect();
        for &window in windows {
            if !members.contains(&window) {
                members.push(window);
            }
        }
        if members.len() < 2 {
            return None;
        }
        let id = match merged.first() {
            Some(&id) => id,
            None => {
                self.next_id += 1;
                GroupId(self.next_id)
            }
        };
        self.groups.retain(|g| !merged.contains(&g.id));
        self.groups.push(Group { id, members });
        Some(id)
    }

    /// Takes `window` out of its group, dissolving the group if one window is left.
    pub fn remove(&mut self, window: WindowId) {
        for group in &mut self.groups {
            group.members.retain(|&w| w != window);
        }
        self.groups.retain(|g| g.members.len() >= 2);
    }
}

impl State {
    /// Groups the snapped windows that can be seen on the focused window's output (Super+g).
    pub fn group_visible(&mut self) {
        let Some(output) = self
            .focus
            .current()
            .and_then(|id| self.window(id))
            .map(|w| w.output.clone())
            .or_else(|| self.primary_output())
        else {
            return;
        };
        let stack: Vec<(WindowId, Option<Layout>)> = self
            .space
            .elements()
            .rev()
            .filter_map(|element| self.windows.iter().find(|w| &w.window == element))
            .filter(|w| w.output == output)
            .map(|w| (w.id, w.layout))
            .collect();
        let layouts: Vec<Option<Layout>> = stack.iter().map(|&(_, layout)| layout).collect();
        let windows: Vec<WindowId> = visible_tiles(&layouts)
            .into_iter()
            .map(|index| stack[index].0)
            .collect();
        match self.groups.group(&windows) {
            Some(group) => {
                let members: Vec<u64> = self.groups.members(group).iter().map(|w| w.0).collect();
                info!(?group, ?members, "windows grouped");
            }
            None => info!("fewer than two snapped windows on screen; nothing grouped"),
        }
    }

    /// Takes the focused window out of its group (Super+Shift+g).
    pub fn ungroup_focused(&mut self) {
        let Some(id) = self.focus.current() else {
            return;
        };
        if self.groups.group_of(id).is_none() {
            return;
        }
        self.groups.remove(id);
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == id) {
            window.unzoomed = None;
        }
        info!(window = id.0, "window taken out of its group");
    }

    /// Focuses the next or previous member of the focused window's group, in reading order of where they sit on screen.
    pub fn cycle_group(&mut self, forward: bool) {
        let Some(current) = self.focus.current() else {
            return;
        };
        let Some(group) = self.groups.group_of(current) else {
            return;
        };
        let mut members: Vec<(WindowId, (i32, i32))> = self
            .groups
            .members(group)
            .iter()
            .filter_map(|&id| {
                let window = self.window(id).filter(|w| w.mapped)?;
                let loc = self.space.element_location(&window.window)?;
                Some((id, (loc.y, loc.x)))
            })
            .collect();
        members.sort_by_key(|&(id, position)| (position, id.0));
        let Some(index) = members.iter().position(|&(id, _)| id == current) else {
            return;
        };
        let len = members.len();
        let next = if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        };
        self.focus_window(members[next].0);
    }

    /// Raises the other members of `window`'s group above everything else, most recently used on top, so the group comes into view with `window`.
    pub fn raise_group_of(&mut self, window: WindowId) {
        let Some(group) = self.groups.group_of(window) else {
            return;
        };
        let members = self.groups.members(group);
        let mut others: Vec<WindowId> = self
            .focus
            .order()
            .iter()
            .copied()
            .filter(|&id| id != window && members.contains(&id))
            .collect();
        others.reverse();
        for id in others {
            if let Some(element) = self
                .window(id)
                .filter(|w| w.mapped)
                .map(|w| w.window.clone())
            {
                self.space.raise_element(&element, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(ids: &[u64]) -> Vec<WindowId> {
        ids.iter().map(|&id| WindowId(id)).collect()
    }

    #[test]
    fn groups_windows_together() {
        let mut groups = Groups::default();
        let group = groups.group(&ids(&[1, 2, 3])).unwrap();
        assert_eq!(groups.members(group), ids(&[1, 2, 3]));
        assert_eq!(groups.group_of(WindowId(2)), Some(group));
        assert_eq!(groups.group_of(WindowId(4)), None);
    }

    #[test]
    fn a_single_window_is_not_a_group() {
        let mut groups = Groups::default();
        assert_eq!(groups.group(&ids(&[1])), None);
        assert_eq!(groups.group(&ids(&[1, 1])), None);
        assert_eq!(groups.group_of(WindowId(1)), None);
    }

    #[test]
    fn grouping_with_a_member_extends_its_group() {
        let mut groups = Groups::default();
        let group = groups.group(&ids(&[1, 2])).unwrap();
        assert_eq!(groups.group(&ids(&[3, 2])), Some(group));
        assert_eq!(groups.members(group), ids(&[1, 2, 3]));
    }

    #[test]
    fn grouping_members_of_two_groups_merges_them() {
        let mut groups = Groups::default();
        let first = groups.group(&ids(&[1, 2])).unwrap();
        let second = groups.group(&ids(&[3, 4])).unwrap();
        assert_ne!(first, second);
        assert_eq!(groups.group(&ids(&[4, 1])), Some(first));
        assert_eq!(groups.members(first), ids(&[1, 2, 3, 4]));
        assert_eq!(groups.members(second), []);
    }

    #[test]
    fn regrouping_the_same_windows_changes_nothing() {
        let mut groups = Groups::default();
        let group = groups.group(&ids(&[1, 2])).unwrap();
        assert_eq!(groups.group(&ids(&[1, 2])), Some(group));
        assert_eq!(groups.members(group), ids(&[1, 2]));
    }

    #[test]
    fn removing_the_second_last_member_dissolves_the_group() {
        let mut groups = Groups::default();
        let group = groups.group(&ids(&[1, 2, 3])).unwrap();
        groups.remove(WindowId(2));
        assert_eq!(groups.members(group), ids(&[1, 3]));
        groups.remove(WindowId(1));
        assert_eq!(groups.group_of(WindowId(3)), None);
        // Removing a window in no group is harmless.
        groups.remove(WindowId(9));
    }
}
