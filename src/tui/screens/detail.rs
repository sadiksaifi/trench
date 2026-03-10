use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
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

const METADATA_HEIGHT: u16 = 4;

pub fn render(state: &DetailState, frame: &mut Frame, area: Rect) {
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let chunks = Layout::vertical([
        Constraint::Length(METADATA_HEIGHT),
        Constraint::Length(1), // separator
        Constraint::Min(1),   // body (files + commits)
    ])
    .split(area);

    // — Metadata section —
    let metadata_lines = vec![
        Line::from(vec![
            Span::styled("Branch: ", bold),
            Span::raw(&state.branch),
            Span::raw("  "),
            Span::styled("Name: ", bold),
            Span::raw(&state.name),
        ]),
        Line::from(vec![
            Span::styled("Path:   ", bold),
            Span::raw(&state.path),
        ]),
        Line::from(vec![
            Span::styled("Base:   ", bold),
            Span::raw(&state.base_branch),
            Span::raw("  "),
            Span::styled("Ahead/Behind: ", bold),
            Span::raw(&state.ahead_behind),
        ]),
        Line::from(vec![
            Span::styled("Created: ", bold),
            Span::raw(&state.created),
            Span::raw("  "),
            Span::styled("Last Accessed: ", bold),
            Span::raw(&state.last_accessed),
        ]),
    ];
    frame.render_widget(Paragraph::new(metadata_lines), chunks[0]);

    // — Changed files section —
    let mut file_lines: Vec<Line> = vec![Line::from(Span::styled("Changed Files", bold))];
    if state.changed_files.is_empty() {
        file_lines.push(Line::from("  No changes"));
    } else {
        for (path, status) in &state.changed_files {
            file_lines.push(Line::from(format!("  {status:>10}  {path}")));
        }
    }
    frame.render_widget(Paragraph::new(file_lines), chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn render_to_buffer(state: &DetailState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(state, frame, frame.area()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

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

    #[test]
    fn renders_metadata_section_with_branch_and_path() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("feature/auth"), "should show branch, got: {text}");
        assert!(text.contains("feature-auth"), "should show worktree name");
    }

    #[test]
    fn renders_metadata_with_base_branch_and_ahead_behind() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("main"), "should show base branch");
        assert!(text.contains("+1/-0"), "should show ahead/behind");
    }

    #[test]
    fn renders_metadata_with_created_and_last_accessed() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("2026-03-10 14:30"), "should show created date");
        assert!(text.contains("2026-03-11 09:15"), "should show last accessed");
    }

    #[test]
    fn renders_changed_files_section() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("Changed Files"), "should show section header");
        assert!(text.contains("src/auth.rs"), "should show first changed file");
        assert!(text.contains("modified"), "should show file status");
        assert!(text.contains("tests/auth_test.rs"), "should show second file");
        assert!(text.contains("new"), "should show second file status");
    }

    #[test]
    fn renders_no_changes_when_files_empty() {
        let mut state = sample_detail();
        state.changed_files = vec![];
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("No changes"), "should show empty state message");
    }
}
