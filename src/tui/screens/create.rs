use ratatui::{
    layout::Rect,
    Frame,
};

/// Which form field is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateField {
    Branch,
    Base,
    Hooks,
}

/// State for the TUI create-worktree form (FR-46).
pub struct CreateState {
    pub branch_input: String,
    pub cursor_pos: usize,
    pub base_branches: Vec<String>,
    pub selected_base: usize,
    pub hooks_enabled: bool,
    pub focused_field: CreateField,
    pub path_preview: String,
    pub error: Option<String>,
    pub repo_name: String,
    pub worktree_template: String,
}

impl CreateState {
    pub fn new(base_branches: Vec<String>, repo_name: String, worktree_template: String) -> Self {
        Self {
            branch_input: String::new(),
            cursor_pos: 0,
            base_branches,
            selected_base: 0,
            hooks_enabled: true,
            focused_field: CreateField::Branch,
            path_preview: String::new(),
            error: None,
            repo_name,
            worktree_template,
        }
    }
}

pub fn render(_state: &CreateState, _frame: &mut Frame, _area: Rect) {
    // placeholder — will be implemented in rendering cycle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_state_defaults_hooks_enabled() {
        let state = CreateState::new(vec!["main".into()], "repo".into(), "{{ repo }}/{{ branch | sanitize }}".into());
        assert!(state.hooks_enabled, "hooks should be enabled by default");
    }

    #[test]
    fn create_state_starts_on_branch_field() {
        let state = CreateState::new(vec!["main".into()], "repo".into(), "{{ repo }}/{{ branch | sanitize }}".into());
        assert_eq!(state.focused_field, CreateField::Branch);
    }

    #[test]
    fn create_state_branch_input_starts_empty() {
        let state = CreateState::new(vec!["main".into()], "repo".into(), "{{ repo }}/{{ branch | sanitize }}".into());
        assert!(state.branch_input.is_empty());
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn create_state_selected_base_starts_at_zero() {
        let state = CreateState::new(vec!["main".into(), "develop".into()], "repo".into(), "{{ repo }}/{{ branch | sanitize }}".into());
        assert_eq!(state.selected_base, 0);
    }

    #[test]
    fn create_state_stores_base_branches() {
        let branches = vec!["main".into(), "develop".into(), "staging".into()];
        let state = CreateState::new(branches.clone(), "repo".into(), "{{ repo }}/{{ branch | sanitize }}".into());
        assert_eq!(state.base_branches, branches);
    }

    #[test]
    fn create_state_no_error_initially() {
        let state = CreateState::new(vec!["main".into()], "repo".into(), "{{ repo }}/{{ branch | sanitize }}".into());
        assert!(state.error.is_none());
    }

    #[test]
    fn create_field_enum_has_three_variants() {
        let fields = [CreateField::Branch, CreateField::Base, CreateField::Hooks];
        for (i, a) in fields.iter().enumerate() {
            for (j, b) in fields.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }
}
