/// View model for the sync strategy picker screen.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncPickerState {
    /// Name of the worktree being synced.
    pub worktree_name: String,
    /// Currently selected option: 0 = Rebase, 1 = Merge.
    pub selected: usize,
}

impl SyncPickerState {
    pub fn new(worktree_name: &str) -> Self {
        Self {
            worktree_name: worktree_name.to_string(),
            selected: 0,
        }
    }

    /// Returns the two strategy options as (label, description) pairs.
    pub fn options(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("Rebase", "Replay your commits on top of the base branch"),
            ("Merge", "Create a merge commit combining both branches"),
        ]
    }

    pub fn select_next(&mut self) {
        if self.selected < 1 {
            self.selected += 1;
        }
    }

    pub fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_picker_state_holds_worktree_name_and_defaults_to_rebase() {
        let state = SyncPickerState::new("feat-auth");
        assert_eq!(state.worktree_name, "feat-auth");
        assert_eq!(state.selected, 0, "should default to Rebase (index 0)");
    }

    #[test]
    fn sync_picker_has_exactly_two_options() {
        let state = SyncPickerState::new("feat-auth");
        let options = state.options();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].0, "Rebase");
        assert_eq!(options[1].0, "Merge");
    }

    #[test]
    fn sync_picker_options_have_descriptions() {
        let state = SyncPickerState::new("feat-auth");
        let options = state.options();
        assert!(!options[0].1.is_empty(), "Rebase should have a description");
        assert!(!options[1].1.is_empty(), "Merge should have a description");
    }

    #[test]
    fn select_next_moves_from_rebase_to_merge() {
        let mut state = SyncPickerState::new("feat-auth");
        assert_eq!(state.selected, 0);
        state.select_next();
        assert_eq!(state.selected, 1, "should move to Merge");
    }

    #[test]
    fn select_next_clamps_at_merge() {
        let mut state = SyncPickerState::new("feat-auth");
        state.selected = 1;
        state.select_next();
        assert_eq!(state.selected, 1, "should stay at Merge");
    }

    #[test]
    fn select_previous_moves_from_merge_to_rebase() {
        let mut state = SyncPickerState::new("feat-auth");
        state.selected = 1;
        state.select_previous();
        assert_eq!(state.selected, 0, "should move to Rebase");
    }

    #[test]
    fn select_previous_clamps_at_rebase() {
        let mut state = SyncPickerState::new("feat-auth");
        state.select_previous();
        assert_eq!(state.selected, 0, "should stay at Rebase");
    }
}
