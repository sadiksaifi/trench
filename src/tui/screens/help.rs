use ratatui::{
    layout::Rect,
    Frame,
};

/// A single keybinding entry: key label + description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingEntry {
    pub key: &'static str,
    pub description: &'static str,
}

/// A group of keybindings sharing a context label (e.g. "Global", "List").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingGroup {
    pub context: &'static str,
    pub bindings: &'static [KeybindingEntry],
}

/// Returns all keybinding groups for the help overlay.
pub fn keybinding_groups() -> &'static [KeybindingGroup] {
    static GROUPS: &[KeybindingGroup] = &[
        KeybindingGroup {
            context: "Global",
            bindings: &[
                KeybindingEntry { key: "?", description: "Toggle help overlay" },
                KeybindingEntry { key: "q / Esc", description: "Back / quit" },
                KeybindingEntry { key: "Ctrl+c", description: "Force quit" },
            ],
        },
        KeybindingGroup {
            context: "List",
            bindings: &[
                KeybindingEntry { key: "j / ↓", description: "Move down" },
                KeybindingEntry { key: "k / ↑", description: "Move up" },
                KeybindingEntry { key: "Enter", description: "Open detail view" },
                KeybindingEntry { key: "n", description: "Create worktree" },
                KeybindingEntry { key: "s", description: "Sync worktree" },
                KeybindingEntry { key: "D", description: "Delete worktree" },
            ],
        },
        KeybindingGroup {
            context: "Detail",
            bindings: &[
                KeybindingEntry { key: "s", description: "Sync worktree" },
                KeybindingEntry { key: "o", description: "Open in $EDITOR" },
            ],
        },
    ];
    GROUPS
}

pub fn render(_frame: &mut Frame, _area: Rect) {
    // TODO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybinding_groups_returns_global_list_and_detail_contexts() {
        let groups = keybinding_groups();

        // Must have at least 3 groups: Global, List, Detail
        assert!(groups.len() >= 3, "expected at least 3 groups, got {}", groups.len());

        let contexts: Vec<&str> = groups.iter().map(|g| g.context).collect();
        assert!(contexts.contains(&"Global"), "missing Global group");
        assert!(contexts.contains(&"List"), "missing List group");
        assert!(contexts.contains(&"Detail"), "missing Detail group");
    }

    #[test]
    fn each_group_has_at_least_one_binding() {
        let groups = keybinding_groups();
        for group in groups {
            assert!(
                !group.bindings.is_empty(),
                "group '{}' has no bindings",
                group.context
            );
        }
    }

    #[test]
    fn global_group_contains_help_and_quit_bindings() {
        let groups = keybinding_groups();
        let global = groups.iter().find(|g| g.context == "Global").unwrap();
        let keys: Vec<&str> = global.bindings.iter().map(|b| b.key).collect();
        assert!(keys.contains(&"?"), "Global group missing '?' keybinding");
        assert!(keys.contains(&"q / Esc"), "Global group missing quit keybinding");
    }
}
