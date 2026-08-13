#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Up,
    Down,
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

const RESIZE_BINDINGS: &[Binding] = &[
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

pub fn bindings(context: Context) -> &'static [Binding] {
    match context {
        Context::Cockpit => COCKPIT_BINDINGS,
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
                    || matches!(
                        binding.action,
                        Action::Switch | Action::Create | Action::Search | Action::Help
                    ))
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
