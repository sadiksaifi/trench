use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::paths;

/// Which form field is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateField {
    Branch,
    Base,
    Hooks,
}

/// Result message displayed after a create attempt.
pub struct CreateResultMessage {
    pub success: bool,
    pub message: String,
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
    pub result: Option<CreateResultMessage>,
    pub repo_name: String,
    pub worktree_template: String,
}

impl CreateState {
    /// Whether the form is showing a result (post-create).
    pub fn is_result_mode(&self) -> bool {
        self.result.is_some()
    }
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
            result: None,
            repo_name,
            worktree_template,
        }
    }

    /// Insert a character at the current cursor position.
    pub fn insert_char(&mut self, ch: char) {
        self.branch_input.insert(self.cursor_pos, ch);
        self.cursor_pos += ch.len_utf8();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.branch_input[..self.cursor_pos]
                .chars()
                .last()
                .unwrap()
                .len_utf8();
            self.cursor_pos -= prev;
            self.branch_input.remove(self.cursor_pos);
        }
    }

    /// Move cursor one character to the left.
    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.branch_input[..self.cursor_pos]
                .chars()
                .last()
                .unwrap()
                .len_utf8();
            self.cursor_pos -= prev;
        }
    }

    /// Move focus to the next form field (Tab).
    pub fn focus_next(&mut self) {
        self.focused_field = match self.focused_field {
            CreateField::Branch => CreateField::Base,
            CreateField::Base => CreateField::Hooks,
            CreateField::Hooks => CreateField::Branch,
        };
    }

    /// Move focus to the previous form field (Shift-Tab).
    pub fn focus_previous(&mut self) {
        self.focused_field = match self.focused_field {
            CreateField::Branch => CreateField::Hooks,
            CreateField::Base => CreateField::Branch,
            CreateField::Hooks => CreateField::Base,
        };
    }

    /// Cycle base branch selection forward (wrapping).
    pub fn select_next_base(&mut self) {
        if !self.base_branches.is_empty() {
            self.selected_base = (self.selected_base + 1) % self.base_branches.len();
        }
    }

    /// Cycle base branch selection backward (wrapping).
    pub fn select_previous_base(&mut self) {
        if !self.base_branches.is_empty() {
            self.selected_base = if self.selected_base == 0 {
                self.base_branches.len() - 1
            } else {
                self.selected_base - 1
            };
        }
    }

    /// Return the currently selected base branch name, if any.
    pub fn selected_base_branch(&self) -> Option<&str> {
        self.base_branches.get(self.selected_base).map(|s| s.as_str())
    }

    /// Recompute the path preview from the current branch input and repo name.
    pub fn update_path_preview(&mut self) {
        if self.branch_input.is_empty() {
            self.path_preview.clear();
            return;
        }
        match paths::render_worktree_path(&self.worktree_template, &self.repo_name, &self.branch_input) {
            Ok(p) => self.path_preview = p.to_string_lossy().into_owned(),
            Err(_) => self.path_preview.clear(),
        }
    }

    /// Validate the form. Sets `self.error` on failure, clears it on success.
    pub fn validate(&mut self) -> Result<(), ()> {
        if let Err(reason) = paths::validate_branch_name(&self.branch_input) {
            self.error = Some(reason);
            return Err(());
        }
        self.error = None;
        Ok(())
    }

    /// Toggle hooks on/off.
    pub fn toggle_hooks(&mut self) {
        self.hooks_enabled = !self.hooks_enabled;
    }

    /// Move cursor one character to the right.
    pub fn cursor_right(&mut self) {
        if self.cursor_pos < self.branch_input.len() {
            let next = self.branch_input[self.cursor_pos..]
                .chars()
                .next()
                .unwrap()
                .len_utf8();
            self.cursor_pos += next;
        }
    }
}

const FOOTER_KEYS: &str = " Tab next field  Enter create  Esc cancel ";
const FOOTER_RESULT: &str = " Enter dismiss ";

pub fn render(state: &CreateState, frame: &mut Frame, area: Rect) {
    // Result mode — show outcome + dismiss footer
    if let Some(ref result) = state.result {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
        let title = Paragraph::new(Line::from(vec![
            Span::styled("Create Worktree", Style::default().add_modifier(Modifier::BOLD)),
        ]));
        frame.render_widget(title, chunks[0]);
        let msg = Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::raw(&result.message),
        ]));
        frame.render_widget(msg, chunks[1]);
        let footer = Paragraph::new(Line::from(FOOTER_RESULT))
            .style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_widget(footer, chunks[2]);
        return;
    }

    // Layout: title (1) + form rows (7) + path preview (1) + error (1) + spacer + footer (1)
    let chunks = Layout::vertical([
        Constraint::Length(2), // title
        Constraint::Length(2), // branch label + input
        Constraint::Length(2), // base label + selector
        Constraint::Length(2), // hooks label + toggle
        Constraint::Length(1), // separator
        Constraint::Length(1), // path preview
        Constraint::Length(1), // error
        Constraint::Min(0),   // spacer
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Title
    let title = Paragraph::new(Line::from(vec![
        Span::styled("Create Worktree", Style::default().add_modifier(Modifier::BOLD)),
    ]));
    frame.render_widget(title, chunks[0]);

    // Branch input
    let branch_style = if state.focused_field == CreateField::Branch {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let branch_spans = if state.focused_field == CreateField::Branch && !state.is_result_mode() {
        let (before, after) = state.branch_input.split_at(state.cursor_pos);
        vec![
            Span::styled("  Branch: ", branch_style),
            Span::raw(before.to_string()),
            Span::styled("\u{2588}", Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(after.to_string()),
        ]
    } else {
        vec![
            Span::styled("  Branch: ", branch_style),
            Span::raw(&state.branch_input),
        ]
    };
    let branch_label = Paragraph::new(Line::from(branch_spans));
    frame.render_widget(branch_label, chunks[1]);

    // Base branch selector
    let base_style = if state.focused_field == CreateField::Base {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let base_value = state
        .selected_base_branch()
        .unwrap_or("-");
    let base_label = Paragraph::new(Line::from(vec![
        Span::styled("  Base:   ", base_style),
        Span::raw(format!("< {} >", base_value)),
    ]));
    frame.render_widget(base_label, chunks[2]);

    // Hooks toggle
    let hooks_style = if state.focused_field == CreateField::Hooks {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let hooks_value = if state.hooks_enabled { "ON" } else { "OFF" };
    let hooks_label = Paragraph::new(Line::from(vec![
        Span::styled("  Hooks:  ", hooks_style),
        Span::raw(format!("[{}]", hooks_value)),
    ]));
    frame.render_widget(hooks_label, chunks[3]);

    // Path preview
    if !state.path_preview.is_empty() {
        let preview = Paragraph::new(Line::from(vec![
            Span::styled("  Path:   ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(&state.path_preview, Style::default().add_modifier(Modifier::DIM)),
        ]));
        frame.render_widget(preview, chunks[5]);
    }

    // Error
    if let Some(ref err) = state.error {
        let error_line = Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(err.as_str(), Style::default().add_modifier(Modifier::BOLD)),
        ]));
        frame.render_widget(error_line, chunks[6]);
    }

    // Footer
    let footer = Paragraph::new(Line::from(FOOTER_KEYS))
        .style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_widget(footer, chunks[8]);
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

    fn sample_state() -> CreateState {
        CreateState::new(
            vec!["main".into(), "develop".into()],
            "repo".into(),
            "{{ repo }}/{{ branch | sanitize }}".into(),
        )
    }

    #[test]
    fn insert_char_appends_to_branch_input() {
        let mut state = sample_state();
        state.insert_char('f');
        state.insert_char('o');
        state.insert_char('o');
        assert_eq!(state.branch_input, "foo");
        assert_eq!(state.cursor_pos, 3);
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut state = sample_state();
        state.insert_char('a');
        state.insert_char('b');
        state.backspace();
        assert_eq!(state.branch_input, "a");
        assert_eq!(state.cursor_pos, 1);
    }

    #[test]
    fn backspace_on_empty_does_nothing() {
        let mut state = sample_state();
        state.backspace();
        assert_eq!(state.branch_input, "");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn insert_char_at_middle_position() {
        let mut state = sample_state();
        state.insert_char('a');
        state.insert_char('c');
        state.cursor_pos = 1; // move cursor between 'a' and 'c'
        state.insert_char('b');
        assert_eq!(state.branch_input, "abc");
        assert_eq!(state.cursor_pos, 2);
    }

    #[test]
    fn backspace_at_middle_position() {
        let mut state = sample_state();
        state.branch_input = "abc".into();
        state.cursor_pos = 2;
        state.backspace();
        assert_eq!(state.branch_input, "ac");
        assert_eq!(state.cursor_pos, 1);
    }

    #[test]
    fn cursor_left_moves_cursor_back() {
        let mut state = sample_state();
        state.branch_input = "abc".into();
        state.cursor_pos = 3;
        state.cursor_left();
        assert_eq!(state.cursor_pos, 2);
    }

    #[test]
    fn cursor_left_clamps_at_zero() {
        let mut state = sample_state();
        state.cursor_left();
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn cursor_right_moves_cursor_forward() {
        let mut state = sample_state();
        state.branch_input = "abc".into();
        state.cursor_pos = 1;
        state.cursor_right();
        assert_eq!(state.cursor_pos, 2);
    }

    #[test]
    fn cursor_right_clamps_at_end() {
        let mut state = sample_state();
        state.branch_input = "abc".into();
        state.cursor_pos = 3;
        state.cursor_right();
        assert_eq!(state.cursor_pos, 3);
    }

    #[test]
    fn focus_next_moves_branch_to_base() {
        let mut state = sample_state();
        assert_eq!(state.focused_field, CreateField::Branch);
        state.focus_next();
        assert_eq!(state.focused_field, CreateField::Base);
    }

    #[test]
    fn focus_next_moves_base_to_hooks() {
        let mut state = sample_state();
        state.focused_field = CreateField::Base;
        state.focus_next();
        assert_eq!(state.focused_field, CreateField::Hooks);
    }

    #[test]
    fn focus_next_wraps_hooks_to_branch() {
        let mut state = sample_state();
        state.focused_field = CreateField::Hooks;
        state.focus_next();
        assert_eq!(state.focused_field, CreateField::Branch);
    }

    #[test]
    fn focus_previous_moves_branch_to_hooks() {
        let mut state = sample_state();
        state.focus_previous();
        assert_eq!(state.focused_field, CreateField::Hooks);
    }

    #[test]
    fn focus_previous_moves_base_to_branch() {
        let mut state = sample_state();
        state.focused_field = CreateField::Base;
        state.focus_previous();
        assert_eq!(state.focused_field, CreateField::Branch);
    }

    #[test]
    fn focus_previous_moves_hooks_to_base() {
        let mut state = sample_state();
        state.focused_field = CreateField::Hooks;
        state.focus_previous();
        assert_eq!(state.focused_field, CreateField::Base);
    }

    #[test]
    fn select_next_base_cycles_forward() {
        let mut state = CreateState::new(
            vec!["main".into(), "develop".into(), "staging".into()],
            "repo".into(),
            "t".into(),
        );
        assert_eq!(state.selected_base, 0);
        state.select_next_base();
        assert_eq!(state.selected_base, 1);
        state.select_next_base();
        assert_eq!(state.selected_base, 2);
        state.select_next_base();
        assert_eq!(state.selected_base, 0, "should wrap around");
    }

    #[test]
    fn select_previous_base_cycles_backward() {
        let mut state = CreateState::new(
            vec!["main".into(), "develop".into(), "staging".into()],
            "repo".into(),
            "t".into(),
        );
        state.select_previous_base();
        assert_eq!(state.selected_base, 2, "should wrap to last");
        state.select_previous_base();
        assert_eq!(state.selected_base, 1);
    }

    #[test]
    fn select_base_on_empty_list_does_nothing() {
        let mut state = CreateState::new(vec![], "repo".into(), "t".into());
        state.select_next_base();
        assert_eq!(state.selected_base, 0);
        state.select_previous_base();
        assert_eq!(state.selected_base, 0);
    }

    #[test]
    fn selected_base_branch_returns_current_selection() {
        let state = CreateState::new(
            vec!["main".into(), "develop".into()],
            "repo".into(),
            "t".into(),
        );
        assert_eq!(state.selected_base_branch(), Some("main"));
    }

    #[test]
    fn selected_base_branch_returns_none_on_empty() {
        let state = CreateState::new(vec![], "repo".into(), "t".into());
        assert_eq!(state.selected_base_branch(), None);
    }

    #[test]
    fn update_path_preview_renders_sanitized_path() {
        let mut state = CreateState::new(
            vec!["main".into()],
            "my-project".into(),
            "{{ repo }}/{{ branch | sanitize }}".into(),
        );
        state.branch_input = "feature/auth".into();
        state.update_path_preview();
        assert_eq!(state.path_preview, "my-project/feature-auth");
    }

    #[test]
    fn update_path_preview_empty_branch_clears_preview() {
        let mut state = CreateState::new(
            vec!["main".into()],
            "my-project".into(),
            "{{ repo }}/{{ branch | sanitize }}".into(),
        );
        state.update_path_preview();
        assert_eq!(state.path_preview, "");
    }

    #[test]
    fn update_path_preview_with_custom_template() {
        let mut state = CreateState::new(
            vec!["main".into()],
            "trench".into(),
            "custom/{{ repo }}/{{ branch | sanitize }}".into(),
        );
        state.branch_input = "fix@home".into();
        state.update_path_preview();
        assert_eq!(state.path_preview, "custom/trench/fix-home");
    }

    #[test]
    fn toggle_hooks_flips_enabled() {
        let mut state = sample_state();
        assert!(state.hooks_enabled);
        state.toggle_hooks();
        assert!(!state.hooks_enabled);
        state.toggle_hooks();
        assert!(state.hooks_enabled);
    }

    #[test]
    fn validate_empty_branch_returns_error() {
        let mut state = sample_state();
        let result = state.validate();
        assert!(result.is_err());
        assert!(state.error.is_some());
        assert!(state.error.as_ref().unwrap().contains("Branch name"));
    }

    #[test]
    fn validate_whitespace_only_branch_returns_error() {
        let mut state = sample_state();
        state.branch_input = "   ".into();
        let result = state.validate();
        assert!(result.is_err());
    }

    #[test]
    fn validate_valid_branch_clears_error() {
        let mut state = sample_state();
        state.branch_input = "feature/auth".into();
        state.error = Some("old error".into());
        let result = state.validate();
        assert!(result.is_ok());
        assert!(state.error.is_none());
    }

    #[test]
    fn validate_branch_with_double_dots_returns_error() {
        let mut state = sample_state();
        state.branch_input = "..".into();
        let result = state.validate();
        assert!(result.is_err());
        assert!(state.error.is_some());
    }

    #[test]
    fn validate_branch_with_spaces_returns_error() {
        let mut state = sample_state();
        state.branch_input = "foo bar".into();
        let result = state.validate();
        assert!(result.is_err());
        assert!(state.error.is_some());
    }

    fn render_to_buffer(state: &CreateState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(state, frame, frame.area()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn render_shows_title() {
        let state = sample_state();
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Create Worktree"), "should show title, got: {text}");
    }

    #[test]
    fn render_shows_branch_label() {
        let state = sample_state();
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Branch"), "should show Branch label");
    }

    #[test]
    fn render_shows_base_label_and_selected_branch() {
        let state = sample_state();
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Base"), "should show Base label");
        assert!(text.contains("main"), "should show selected base branch 'main'");
    }

    #[test]
    fn render_shows_hooks_label() {
        let state = sample_state();
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Hooks"), "should show Hooks label");
    }

    #[test]
    fn render_shows_hooks_enabled_state() {
        let mut state = sample_state();
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("ON"), "should show ON when hooks enabled");

        state.hooks_enabled = false;
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("OFF"), "should show OFF when hooks disabled");
    }

    #[test]
    fn render_shows_path_preview_when_branch_entered() {
        let mut state = sample_state();
        state.branch_input = "feature/auth".into();
        state.update_path_preview();
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Path"), "should show Path label");
        assert!(text.contains("repo/feature-auth"), "should show path preview");
    }

    #[test]
    fn render_shows_error_message() {
        let mut state = sample_state();
        state.error = Some("Branch name is required".into());
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Branch name is required"), "should show error");
    }

    #[test]
    fn render_shows_branch_input_text() {
        let mut state = sample_state();
        state.branch_input = "my-feature".into();
        state.cursor_pos = 10;
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("my-feature"), "should show typed branch name");
    }

    #[test]
    fn render_shows_footer_keybindings() {
        let state = sample_state();
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Tab"), "should show Tab in footer");
        assert!(text.contains("Esc"), "should show Esc in footer");
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

    #[test]
    fn render_shows_cursor_in_branch_field() {
        let state = sample_state();
        // Branch is focused by default and empty
        assert_eq!(state.focused_field, CreateField::Branch);
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains('\u{2588}'),
            "should show block cursor in focused Branch field, got: {text}"
        );
    }
}
