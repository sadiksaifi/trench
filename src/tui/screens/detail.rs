use ratatui::{
    layout::Rect,
    Frame,
};

/// View model for the detail screen showing a single worktree's information.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailState {
    pub name: String,
    pub branch: String,
    pub path: String,
    pub base_branch: String,
    pub ahead_behind: String,
    pub created: String,
    pub last_accessed: String,
    pub hook_status: String,
    pub hook_timestamp: String,
    pub changed_files: Vec<(String, String)>,
    pub commits: Vec<(String, String)>,
}

pub fn render(_state: &DetailState, _frame: &mut Frame, _area: Rect) {
    // TODO: implement
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_detail() -> DetailState {
        DetailState {
            name: "feature-auth".into(),
            branch: "feature/auth".into(),
            path: "/home/user/.worktrees/myproject/feature-auth".into(),
            base_branch: "main".into(),
            ahead_behind: "+1/-0".into(),
            created: "2026-03-10 14:30".into(),
            last_accessed: "2026-03-11 09:15".into(),
            hook_status: "success".into(),
            hook_timestamp: "2026-03-10 14:31".into(),
            changed_files: vec![
                ("src/auth.rs".into(), "modified".into()),
                ("tests/auth_test.rs".into(), "new".into()),
            ],
            commits: vec![
                ("abc1234".into(), "feat: add auth module".into()),
                ("def5678".into(), "test: add auth tests".into()),
            ],
        }
    }

    #[test]
    fn detail_state_holds_all_metadata_fields() {
        let state = sample_detail();
        assert_eq!(state.name, "feature-auth");
        assert_eq!(state.branch, "feature/auth");
        assert_eq!(state.path, "/home/user/.worktrees/myproject/feature-auth");
        assert_eq!(state.base_branch, "main");
        assert_eq!(state.ahead_behind, "+1/-0");
        assert_eq!(state.created, "2026-03-10 14:30");
        assert_eq!(state.last_accessed, "2026-03-11 09:15");
    }

    #[test]
    fn detail_state_holds_hook_status() {
        let state = sample_detail();
        assert_eq!(state.hook_status, "success");
        assert_eq!(state.hook_timestamp, "2026-03-10 14:31");
    }

    #[test]
    fn detail_state_holds_changed_files() {
        let state = sample_detail();
        assert_eq!(state.changed_files.len(), 2);
        assert_eq!(state.changed_files[0], ("src/auth.rs".into(), "modified".into()));
    }

    #[test]
    fn detail_state_holds_commits() {
        let state = sample_detail();
        assert_eq!(state.commits.len(), 2);
        assert_eq!(state.commits[0], ("abc1234".into(), "feat: add auth module".into()));
    }

    #[test]
    fn detail_state_supports_empty_lists() {
        let state = DetailState {
            name: "empty".into(),
            branch: "empty-branch".into(),
            path: "/tmp/empty".into(),
            base_branch: "-".into(),
            ahead_behind: "-".into(),
            created: "-".into(),
            last_accessed: "never".into(),
            hook_status: "none".into(),
            hook_timestamp: "-".into(),
            changed_files: vec![],
            commits: vec![],
        };
        assert!(state.changed_files.is_empty());
        assert!(state.commits.is_empty());
    }
}
