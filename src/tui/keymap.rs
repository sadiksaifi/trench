//! Keys for the worktree list. Dialogs own their keys.

use crate::tui::line_input::LineEdit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Up,
    Down,
    Edit(LineEdit),
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Switch,
    Open,
    Create,
    Sync,
    Remove,
    Search,
    CloseSearch,
    Refresh,
    SelectNext,
    SelectPrevious,
    Quit,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Cockpit,
    Search,
    Resize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub keys: &'static [Key],
    /// The key as written in the hint bar and in help.
    pub label: &'static str,
    /// The one-word action shown in the hint bar.
    pub hint: &'static str,
    /// The sentence shown in help.
    pub help: &'static str,
    /// Hint bar rank: when the bar is too narrow, higher numbers are dropped
    /// first. `None` keeps the binding out of the bar; it is still in help.
    pub rank: Option<u8>,
    pub action: Action,
}

const COCKPIT_BINDINGS: &[Binding] = &[
    Binding {
        keys: &[Key::Enter],
        label: "Enter",
        hint: "switch",
        help: "switch to the selected worktree",
        rank: Some(0),
        action: Action::Switch,
    },
    Binding {
        keys: &[Key::Char('o')],
        label: "o",
        hint: "open",
        help: "open it in your editor",
        rank: Some(6),
        action: Action::Open,
    },
    Binding {
        keys: &[Key::Char('c')],
        label: "c",
        hint: "create",
        help: "create a worktree",
        rank: Some(1),
        action: Action::Create,
    },
    Binding {
        keys: &[Key::Char('s')],
        label: "s",
        hint: "sync",
        help: "sync it with its base branch",
        rank: Some(5),
        action: Action::Sync,
    },
    Binding {
        keys: &[Key::Char('d')],
        label: "d",
        hint: "remove",
        help: "remove it",
        rank: Some(4),
        action: Action::Remove,
    },
    Binding {
        keys: &[Key::Char('/')],
        label: "/",
        hint: "search",
        help: "search",
        rank: Some(7),
        action: Action::Search,
    },
    Binding {
        keys: &[Key::Char('r')],
        label: "r",
        hint: "refresh",
        help: "refresh the list",
        rank: None,
        action: Action::Refresh,
    },
    Binding {
        keys: &[Key::Down, Key::Char('j')],
        label: "↓ j",
        hint: "next",
        help: "move down",
        rank: None,
        action: Action::SelectNext,
    },
    Binding {
        keys: &[Key::Up, Key::Char('k')],
        label: "↑ k",
        hint: "previous",
        help: "move up",
        rank: None,
        action: Action::SelectPrevious,
    },
    Binding {
        keys: &[Key::Char('?')],
        label: "?",
        hint: "help",
        help: "show these keys",
        rank: Some(2),
        action: Action::Help,
    },
    Binding {
        keys: &[Key::Char('q')],
        label: "q",
        hint: "quit",
        help: "quit",
        rank: Some(3),
        action: Action::Quit,
    },
];

const RESIZE_BINDINGS: &[Binding] = &[Binding {
    keys: &[Key::Char('q')],
    label: "q",
    hint: "quit",
    help: "quit",
    rank: Some(0),
    action: Action::Quit,
}];

const SEARCH_BINDINGS: &[Binding] = &[
    Binding {
        keys: &[Key::Enter],
        label: "Enter",
        hint: "switch",
        help: "switch to the selected worktree",
        rank: Some(0),
        action: Action::Switch,
    },
    Binding {
        keys: &[Key::Down],
        label: "↑/↓",
        hint: "move",
        help: "move down",
        rank: Some(2),
        action: Action::SelectNext,
    },
    Binding {
        keys: &[Key::Up],
        label: "↑",
        hint: "previous",
        help: "move up",
        rank: None,
        action: Action::SelectPrevious,
    },
    Binding {
        keys: &[Key::Escape],
        label: "Esc",
        hint: "close search",
        help: "close search",
        rank: Some(1),
        action: Action::CloseSearch,
    },
];

pub fn bindings(context: Context) -> &'static [Binding] {
    match context {
        Context::Cockpit => COCKPIT_BINDINGS,
        Context::Search => SEARCH_BINDINGS,
        Context::Resize => RESIZE_BINDINGS,
    }
}

pub fn action_for(context: Context, key: Key) -> Option<Action> {
    bindings(context)
        .iter()
        .find(|binding| binding.keys.contains(&key))
        .map(|binding| binding.action)
}

/// The bindings shown in the hint bar, in display order.
pub fn hint_bindings(context: Context) -> Vec<&'static Binding> {
    bindings(context)
        .iter()
        .filter(|binding| binding.rank.is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cockpit_routes_each_primary_key_to_one_action() {
        for (key, action) in [
            (Key::Enter, Action::Switch),
            (Key::Char('o'), Action::Open),
            (Key::Char('c'), Action::Create),
            (Key::Char('s'), Action::Sync),
            (Key::Char('d'), Action::Remove),
            (Key::Char('/'), Action::Search),
            (Key::Char('r'), Action::Refresh),
            (Key::Down, Action::SelectNext),
            (Key::Char('j'), Action::SelectNext),
            (Key::Up, Action::SelectPrevious),
            (Key::Char('k'), Action::SelectPrevious),
            (Key::Char('q'), Action::Quit),
            (Key::Char('?'), Action::Help),
        ] {
            assert_eq!(action_for(Context::Cockpit, key), Some(action), "{key:?}");
        }
        assert_eq!(action_for(Context::Cockpit, Key::Char('n')), None);
        assert_eq!(action_for(Context::Cockpit, Key::Char('i')), None);
    }

    #[test]
    fn every_cockpit_key_is_in_help_and_the_hint_bar_keeps_the_essentials_longest() {
        let hints = hint_bindings(Context::Cockpit);
        let ranked = |action| {
            hints
                .iter()
                .find(|binding| binding.action == action)
                .and_then(|binding| binding.rank)
                .unwrap()
        };

        assert!(bindings(Context::Cockpit)
            .iter()
            .all(|binding| !binding.help.is_empty()));
        for essential in [Action::Switch, Action::Create, Action::Help, Action::Quit] {
            for droppable in [Action::Remove, Action::Sync, Action::Open, Action::Search] {
                assert!(ranked(essential) < ranked(droppable));
            }
        }
        assert!(!hints
            .iter()
            .any(|binding| binding.action == Action::Refresh));
    }

    #[test]
    fn resize_context_has_only_quit() {
        assert_eq!(bindings(Context::Resize).len(), 1);
        assert_eq!(
            action_for(Context::Resize, Key::Char('q')),
            Some(Action::Quit)
        );
        assert_eq!(action_for(Context::Resize, Key::Enter), None);
    }

    #[test]
    fn search_reserves_only_non_printable_keys_so_every_character_is_text() {
        for (key, action) in [
            (Key::Enter, Action::Switch),
            (Key::Escape, Action::CloseSearch),
            (Key::Down, Action::SelectNext),
            (Key::Up, Action::SelectPrevious),
        ] {
            assert_eq!(action_for(Context::Search, key), Some(action));
        }
        for character in ['j', 'k', 'q', 'c', 'd', '?', '/'] {
            assert_eq!(action_for(Context::Search, Key::Char(character)), None);
        }
    }
}
