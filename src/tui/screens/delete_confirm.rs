use ratatui::{Frame, layout::Rect};

pub fn render(_state: &DeleteConfirmState, _frame: &mut Frame, _area: Rect) {
    // TODO: implement centered overlay rendering
}

/// View model for the delete confirmation dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteConfirmState {
    /// Name of the worktree to delete.
    pub worktree_name: String,
    /// Filesystem path of the worktree.
    pub worktree_path: String,
    /// Branch checked out in the worktree.
    pub branch: String,
    /// Result message after deletion. None = confirm mode, Some = result mode.
    pub result: Option<DeleteResultMessage>,
}

/// Outcome displayed after a delete operation completes.
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteResultMessage {
    pub success: bool,
    pub message: String,
}

impl DeleteConfirmState {
    pub fn new(worktree_name: &str, worktree_path: &str, branch: &str) -> Self {
        Self {
            worktree_name: worktree_name.to_string(),
            worktree_path: worktree_path.to_string(),
            branch: branch.to_string(),
            result: None,
        }
    }

    /// Whether the dialog is showing a result (post-delete).
    pub fn is_result_mode(&self) -> bool {
        self.result.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_confirm_state_holds_worktree_info() {
        let state = DeleteConfirmState::new("feat-auth", "/home/user/.worktrees/repo/feat-auth", "feature/auth");
        assert_eq!(state.worktree_name, "feat-auth");
        assert_eq!(state.worktree_path, "/home/user/.worktrees/repo/feat-auth");
        assert_eq!(state.branch, "feature/auth");
    }

    #[test]
    fn delete_confirm_starts_in_confirm_mode() {
        let state = DeleteConfirmState::new("feat-auth", "/tmp/wt", "feature/auth");
        assert!(!state.is_result_mode());
        assert!(state.result.is_none());
    }

    #[test]
    fn is_result_mode_true_after_setting_result() {
        let mut state = DeleteConfirmState::new("feat-auth", "/tmp/wt", "feature/auth");
        state.result = Some(DeleteResultMessage {
            success: true,
            message: "Removed 'feat-auth'".into(),
        });
        assert!(state.is_result_mode());
    }
}
