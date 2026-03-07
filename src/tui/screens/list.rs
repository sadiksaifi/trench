/// View model for a single worktree row in the TUI list.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeRow {
    pub name: String,
    pub branch: String,
    pub status: String,
    pub ahead_behind: String,
    pub managed: bool,
}

/// State for the worktree list screen.
pub struct ListState {
    pub rows: Vec<WorktreeRow>,
    pub selected: usize,
}

impl ListState {
    pub fn new(rows: Vec<WorktreeRow>) -> Self {
        Self { rows, selected: 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rows() -> Vec<WorktreeRow> {
        vec![
            WorktreeRow {
                name: "feature-auth".into(),
                branch: "feature/auth".into(),
                status: "clean".into(),
                ahead_behind: "+1/-0".into(),
                managed: true,
            },
            WorktreeRow {
                name: "fix-bug".into(),
                branch: "fix/bug".into(),
                status: "~3".into(),
                ahead_behind: "+0/-2".into(),
                managed: true,
            },
            WorktreeRow {
                name: "main".into(),
                branch: "main".into(),
                status: "clean".into(),
                ahead_behind: "-".into(),
                managed: false,
            },
        ]
    }

    #[test]
    fn list_state_starts_with_selection_at_zero() {
        let state = ListState::new(sample_rows());
        assert_eq!(state.selected, 0);
        assert_eq!(state.rows.len(), 3);
    }

    #[test]
    fn list_state_empty_rows() {
        let state = ListState::new(vec![]);
        assert_eq!(state.selected, 0);
        assert!(state.rows.is_empty());
    }
}
