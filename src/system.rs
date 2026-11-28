//! The system actions the overview's search offers next to the installed
//! applications: Suspend, Log Out, Restart and Power Off, as in GNOME. Pure
//! logic with no Smithay types, so it can be unit tested; acting on them is
//! in `src/end_session/`.

use crate::{
    apps,
    end_session::EndAction,
    logind::{Availability, PowerAction},
};

/// The shortest query that finds Suspend.
const SUSPEND_MIN_QUERY: usize = 3;

/// How well Suspend matches `query`. Suspend acts without a dialog, so a
/// stray query must not find it: it needs [`SUSPEND_MIN_QUERY`] characters
/// and matches only at the start of a word, so "end" or "eep" never puts
/// the machine to sleep. 0 for the start of its name, 3 for a keyword.
fn suspend_score(query: &str) -> Option<u8> {
    if query.chars().count() < SUSPEND_MIN_QUERY {
        return None;
    }
    let action = SystemAction::Suspend;
    let starts_a_word = |text: &str| text.split_whitespace().any(|word| word.starts_with(query));
    if action.name().to_lowercase().starts_with(query) {
        Some(0)
    } else if starts_a_word(action.keywords()) {
        Some(3)
    } else {
        None
    }
}

/// Something the session or the machine can do, found by name or keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemAction {
    Suspend,
    LogOut,
    Restart,
    PowerOff,
}

impl SystemAction {
    /// Every action, in the order GNOME lists them.
    pub const ALL: [SystemAction; 4] = [
        SystemAction::Suspend,
        SystemAction::LogOut,
        SystemAction::Restart,
        SystemAction::PowerOff,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SystemAction::Suspend => "Suspend",
            SystemAction::LogOut => "Log Out",
            SystemAction::Restart => "Restart",
            SystemAction::PowerOff => "Power Off",
        }
    }

    /// The line under the name in the search results.
    pub fn detail(self) -> &'static str {
        match self {
            SystemAction::Suspend => "Put the computer to sleep",
            SystemAction::LogOut => "End the session",
            SystemAction::Restart => "Restart the computer",
            SystemAction::PowerOff => "Turn the computer off",
        }
    }

    /// The symbolic icon, from the icon theme's actions.
    pub fn icon_name(self) -> &'static str {
        match self {
            SystemAction::Suspend => "media-playback-pause-symbolic",
            SystemAction::LogOut => "system-log-out-symbolic",
            SystemAction::Restart => "system-reboot-symbolic",
            SystemAction::PowerOff => "system-shutdown-symbolic",
        }
    }

    /// Other words the action is found by, lowercased. "reboot" belongs to
    /// Restart only, so typing it never puts Power Off first.
    fn keywords(self) -> &'static str {
        match self {
            SystemAction::Suspend => "suspend sleep",
            SystemAction::LogOut => "logout log out log off sign out signout quit exit leave",
            SystemAction::Restart => "restart reboot",
            SystemAction::PowerOff => "power off poweroff shutdown shut down turn off halt",
        }
    }

    /// How well the action matches `query` (lowercased, not empty), on the
    /// same scale as the applications, so the two can be ranked together.
    /// Suspend acts without a dialog, so it needs at least
    /// [`SUSPEND_MIN_QUERY`] characters: "s" or "su" and Return must start
    /// Settings or Sublime Text, not put the machine to sleep.
    pub fn score(self, query: &str) -> Option<u8> {
        if self == SystemAction::Suspend {
            return suspend_score(query);
        }
        apps::score(self.name(), self.keywords(), query)
    }

    /// Whether the action wins a tie with an application. Suspend does not,
    /// since it acts at once; the others only open a dialog.
    pub fn wins_ties(self) -> bool {
        self != SystemAction::Suspend
    }

    /// The action the confirmation dialog asks about; `None` for Suspend,
    /// which acts at once, as in GNOME.
    pub fn end_action(self) -> Option<EndAction> {
        match self {
            SystemAction::Suspend => None,
            SystemAction::LogOut => Some(EndAction::LogOut),
            SystemAction::Restart => Some(EndAction::Restart),
            SystemAction::PowerOff => Some(EndAction::PowerOff),
        }
    }
}

/// The actions to offer. Log Out always; the others when logind says it
/// can do them. Nested, only Log Out, which closes the nested rustwm: the
/// others would act on the host.
pub fn offered(availability: Availability, nested: bool) -> Vec<SystemAction> {
    SystemAction::ALL
        .into_iter()
        .filter(|action| match action {
            SystemAction::LogOut => true,
            _ if nested => false,
            SystemAction::Suspend => availability.allows(PowerAction::Suspend),
            SystemAction::Restart => availability.allows(PowerAction::Reboot),
            SystemAction::PowerOff => availability.allows(PowerAction::PowerOff),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actions matching `query`, best first.
    fn ranked(query: &str) -> Vec<SystemAction> {
        let mut matches: Vec<(u8, usize, SystemAction)> = SystemAction::ALL
            .into_iter()
            .enumerate()
            .filter_map(|(index, action)| action.score(query).map(|score| (score, index, action)))
            .collect();
        matches.sort_by_key(|&(score, index, _)| (score, index));
        matches.into_iter().map(|(_, _, action)| action).collect()
    }

    #[test]
    fn names_and_keywords_find_the_right_action() {
        use SystemAction::*;
        assert_eq!(ranked("pow"), [PowerOff]);
        assert_eq!(ranked("reboot"), [Restart]);
        assert_eq!(ranked("sleep"), [Suspend]);
        for query in ["logout", "sign out", "quit"] {
            assert_eq!(ranked(query), [LogOut], "{query}");
        }
        for query in ["shutdown", "poweroff", "turn off"] {
            assert_eq!(ranked(query), [PowerOff], "{query}");
        }
        // A word of Power Off's name beats Log Out's "log off" keyword.
        assert_eq!(ranked("off"), [PowerOff, LogOut]);
    }

    #[test]
    fn suspend_needs_three_characters() {
        let suspend = SystemAction::Suspend;
        assert_eq!(suspend.score("s"), None);
        assert_eq!(suspend.score("su"), None);
        assert_eq!(suspend.score("sus"), Some(0));
        assert_eq!(suspend.score("sleep"), Some(3));
        assert_eq!(suspend.score("susp"), Some(0));
        assert_eq!(suspend.score("sle"), Some(3));
        // Only at the start of a word.
        for query in ["end", "eep", "usp", "pend", "spend"] {
            assert_eq!(suspend.score(query), None, "{query}");
        }
        // The others match from the first character.
        assert_eq!(SystemAction::PowerOff.score("p"), Some(0));
    }

    #[test]
    fn only_available_actions_are_offered() {
        let all = Availability {
            suspend: true,
            reboot: true,
            power_off: true,
        };
        assert_eq!(offered(all, false), SystemAction::ALL);
        assert_eq!(offered(all, true), [SystemAction::LogOut]);
        assert_eq!(
            offered(Availability::default(), false),
            [SystemAction::LogOut]
        );
        let no_reboot = Availability {
            reboot: false,
            ..all
        };
        assert_eq!(
            offered(no_reboot, false),
            [
                SystemAction::Suspend,
                SystemAction::LogOut,
                SystemAction::PowerOff
            ]
        );
    }
}
