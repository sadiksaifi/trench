#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Up,
    Down,
    Backspace,
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Switch,
    Open,
    Create,
    Sync,
    Remove,
    DeleteBranch,
    Search,
    CloseSearch,
    Refresh,
    ToggleInspector,
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
    pub label: &'static str,
    pub description: &'static str,
    pub action: Action,
}

const COCKPIT_BINDINGS: &[Binding] = &[
    Binding {
        keys: &[Key::Enter],
        label: "Enter",
        description: "switch",
        action: Action::Switch,
    },
    Binding {
        keys: &[Key::Char('o')],
        label: "o",
        description: "open",
        action: Action::Open,
    },
    Binding {
        keys: &[Key::Char('c')],
        label: "c",
        description: "create",
        action: Action::Create,
    },
    Binding {
        keys: &[Key::Char('s')],
        label: "s",
        description: "sync",
        action: Action::Sync,
    },
    Binding {
        keys: &[Key::Char('d')],
        label: "d",
        description: "remove",
        action: Action::Remove,
    },
    Binding {
        keys: &[Key::Char('/')],
        label: "/",
        description: "search",
        action: Action::Search,
    },
    Binding {
        keys: &[Key::Char('r')],
        label: "r",
        description: "refresh",
        action: Action::Refresh,
    },
    Binding {
        keys: &[Key::Char('i')],
        label: "i",
        description: "inspector",
        action: Action::ToggleInspector,
    },
    Binding {
        keys: &[Key::Down, Key::Char('j')],
        label: "j/↓",
        description: "next",
        action: Action::SelectNext,
    },
    Binding {
        keys: &[Key::Up, Key::Char('k')],
        label: "k/↑",
        description: "previous",
        action: Action::SelectPrevious,
    },
    Binding {
        keys: &[Key::Char('q')],
        label: "q",
        description: "quit",
        action: Action::Quit,
    },
    Binding {
        keys: &[Key::Char('?')],
        label: "?",
        description: "help",
        action: Action::Help,
    },
];

const RESIZE_BINDINGS: &[Binding] = &[Binding {
    keys: &[Key::Char('q')],
    label: "q",
    description: "quit",
    action: Action::Quit,
}];

const SEARCH_BINDINGS: &[Binding] = &[
    Binding {
        keys: &[Key::Enter],
        label: "Enter",
        description: "switch",
        action: Action::Switch,
    },
    Binding {
        keys: &[Key::Escape],
        label: "Esc",
        description: "clear",
        action: Action::CloseSearch,
    },
    Binding {
        keys: &[Key::Down],
        label: "↓",
        description: "next",
        action: Action::SelectNext,
    },
    Binding {
        keys: &[Key::Up],
        label: "↑",
        description: "previous",
        action: Action::SelectPrevious,
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

pub fn keybar_bindings(context: Context, narrow: bool) -> Vec<&'static Binding> {
    let mut visible: Vec<_> = bindings(context)
        .iter()
        .filter(|binding| {
            let is_navigation =
                matches!(binding.action, Action::SelectNext | Action::SelectPrevious);
            !is_navigation
                && (!narrow
                    || context == Context::Resize
                    || match context {
                        Context::Cockpit => matches!(
                            binding.action,
                            Action::Switch | Action::Create | Action::Search | Action::Help
                        ),
                        Context::Search => matches!(
                            binding.action,
                            Action::Switch | Action::CloseSearch | Action::Help
                        ),
                        Context::Resize => true,
                    })
        })
        .collect();
    if let Some(help) = visible
        .iter()
        .position(|binding| binding.action == Action::Help)
        .map(|index| visible.remove(index))
    {
        visible.push(help);
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cockpit_contract_routes_exact_primary_keys() {
        let expected = [
            (Key::Enter, Action::Switch),
            (Key::Char('o'), Action::Open),
            (Key::Char('c'), Action::Create),
            (Key::Char('s'), Action::Sync),
            (Key::Char('d'), Action::Remove),
            (Key::Char('/'), Action::Search),
            (Key::Char('r'), Action::Refresh),
            (Key::Char('i'), Action::ToggleInspector),
            (Key::Char('j'), Action::SelectNext),
            (Key::Down, Action::SelectNext),
            (Key::Char('k'), Action::SelectPrevious),
            (Key::Up, Action::SelectPrevious),
            (Key::Char('q'), Action::Quit),
            (Key::Char('?'), Action::Help),
        ];

        for (key, action) in expected {
            assert_eq!(action_for(Context::Cockpit, key), Some(action));
        }
        assert_eq!(action_for(Context::Cockpit, Key::Char('l')), None);
        assert_eq!(action_for(Context::Cockpit, Key::Char('D')), None);
    }

    #[test]
    fn cockpit_keybar_ends_with_help() {
        for narrow in [false, true] {
            let items = keybar_bindings(Context::Cockpit, narrow);
            assert_eq!(
                items.last().map(|binding| binding.action),
                Some(Action::Help)
            );
        }
    }

    #[test]
    fn resize_context_has_only_quit() {
        let actions = bindings(Context::Resize)
            .iter()
            .map(|binding| binding.action)
            .collect::<Vec<_>>();
        assert_eq!(actions, [Action::Quit]);
    }

    #[test]
    fn search_context_reserves_only_non_printable_navigation_and_submit_keys() {
        let reserved = [
            (Key::Enter, Action::Switch),
            (Key::Escape, Action::CloseSearch),
            (Key::Down, Action::SelectNext),
            (Key::Up, Action::SelectPrevious),
        ];
        for (key, action) in reserved {
            assert_eq!(action_for(Context::Search, key), Some(action));
        }
        for editable in [
            Key::Char('o'),
            Key::Char('s'),
            Key::Char('d'),
            Key::Char('j'),
            Key::Char('k'),
            Key::Char('?'),
            Key::Backspace,
        ] {
            assert_eq!(action_for(Context::Search, editable), None);
        }
    }
}
