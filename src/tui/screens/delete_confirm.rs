use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

const CONFIRM_FOOTER: &str = " Enter/y confirm  Esc/n cancel ";
const RESULT_FOOTER: &str = " Enter/Space dismiss ";

pub fn render(state: &DeleteConfirmState, frame: &mut Frame, area: Rect, theme: &crate::tui::theme::Theme) {
    if let Some(ref result) = state.result {
        render_result(state, result, frame, area, theme);
    } else {
        render_confirm(state, frame, area, theme);
    }
}

/// Compute a centered rectangle within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    area
}

fn render_confirm(state: &DeleteConfirmState, frame: &mut Frame, area: Rect, theme: &crate::tui::theme::Theme) {
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let dialog_area = centered_rect(60, 10, area);
    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(" Delete Worktree ")
        .title_alignment(Alignment::Center);
    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    let chunks = Layout::vertical([
        Constraint::Length(1), // blank
        Constraint::Length(1), // name + branch
        Constraint::Length(1), // path
        Constraint::Length(1), // blank
        Constraint::Length(1), // warning
        Constraint::Min(0),   // spacer
        Constraint::Length(1), // footer
    ])
    .split(inner);

    // Name + branch (truncate branch if combined line exceeds inner width)
    let max_line_chars = 56;
    let overhead = 11; // "Delete " (7) + "  (" (3) + ")" (1)
    let max_content = max_line_chars - overhead;
    let name_len = state.worktree_name.chars().count();
    let branch_len = state.branch.chars().count();
    let branch_display = if name_len + branch_len > max_content {
        let available = max_content.saturating_sub(name_len);
        if available >= 2 {
            let keep = available - 1;
            format!(
                "{}\u{2026}",
                state.branch.chars().take(keep).collect::<String>()
            )
        } else {
            "\u{2026}".to_string()
        }
    } else {
        state.branch.clone()
    };
    let name_line = Line::from(vec![
        Span::styled("Delete ", bold),
        Span::styled(&state.worktree_name, bold),
        Span::raw("  ("),
        Span::raw(branch_display),
        Span::raw(")"),
    ]);
    frame.render_widget(
        Paragraph::new(name_line).alignment(Alignment::Center),
        chunks[1],
    );

    // Path (truncate with leading ellipsis if too long for 60-col dialog)
    let max_path_chars = 56;
    let path_len = state.worktree_path.chars().count();
    let path_display = if path_len > max_path_chars {
        let keep = max_path_chars - 1;
        let suffix: String = state.worktree_path.chars().skip(path_len - keep).collect();
        format!("\u{2026}{suffix}")
    } else {
        state.worktree_path.clone()
    };
    let path_line = Line::from(Span::raw(path_display));
    frame.render_widget(
        Paragraph::new(path_line).alignment(Alignment::Center),
        chunks[2],
    );

    // Warning
    let warning = Line::from(Span::styled(
        "⚠ Pre-remove hooks will run before deletion",
        Style::default().fg(theme.warning),
    ));
    frame.render_widget(
        Paragraph::new(warning).alignment(Alignment::Center),
        chunks[4],
    );

    // Footer
    let footer = Paragraph::new(Line::from(CONFIRM_FOOTER))
        .style(Style::default().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD));
    frame.render_widget(footer, chunks[6]);
}

fn render_result(
    state: &DeleteConfirmState,
    result: &DeleteResultMessage,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
) {
    let status_style = Style::default()
        .fg(if result.success { theme.success } else { theme.error })
        .add_modifier(Modifier::BOLD);

    let dialog_area = centered_rect(60, 8, area);
    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(if result.success {
            " Removed "
        } else {
            " Delete Failed "
        })
        .title_alignment(Alignment::Center);
    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    let chunks = Layout::vertical([
        Constraint::Length(1), // blank
        Constraint::Length(1), // title
        Constraint::Min(1),    // message (multi-line errors)
        Constraint::Length(1), // footer
    ])
    .split(inner);

    let status = if result.success {
        "Worktree removed"
    } else {
        "Deletion failed"
    };
    let title_line = Line::from(vec![
        Span::styled(status, status_style),
        Span::raw(" — "),
        Span::raw(&state.worktree_name),
    ]);
    frame.render_widget(
        Paragraph::new(title_line).alignment(Alignment::Center),
        chunks[1],
    );

    let msg_lines: Vec<Line> = result.message.lines().map(Line::from).collect();
    frame.render_widget(
        Paragraph::new(msg_lines).alignment(Alignment::Center),
        chunks[2],
    );

    let footer = Paragraph::new(Line::from(RESULT_FOOTER))
        .style(Style::default().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD));
    frame.render_widget(footer, chunks[3]);
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

    fn render_to_buffer(state: &DeleteConfirmState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::from_name("catppuccin");
        terminal
            .draw(|frame| render(state, frame, frame.area(), &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

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

    #[test]
    fn renders_worktree_name_in_confirm_dialog() {
        let state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("feat-auth"), "should show worktree name, got: {text}");
    }

    #[test]
    fn renders_worktree_path_in_confirm_dialog() {
        let state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("/tmp/wt/feat-auth"), "should show worktree path");
    }

    #[test]
    fn renders_branch_in_confirm_dialog() {
        let state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("feature/auth"), "should show branch name");
    }

    #[test]
    fn renders_warning_about_hooks() {
        let state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("hook"), "should show warning about hooks");
    }

    #[test]
    fn renders_confirm_footer_keybindings() {
        let state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Enter"), "footer should show Enter");
        assert!(text.contains("confirm"), "footer should show confirm");
        assert!(text.contains("cancel"), "footer should show cancel");
    }

    #[test]
    fn renders_dialog_title() {
        let state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Delete Worktree"), "should show dialog title");
    }

    #[test]
    fn renders_success_result() {
        let mut state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        state.result = Some(DeleteResultMessage {
            success: true,
            message: "Removed successfully".into(),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Removed"), "should show removed title");
        assert!(text.contains("successfully"), "should show result message");
    }

    #[test]
    fn renders_failure_result() {
        let mut state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        state.result = Some(DeleteResultMessage {
            success: false,
            message: "pre_remove hook failed".into(),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Failed"), "should show failed title");
        assert!(text.contains("hook failed"), "should show failure message");
    }

    #[test]
    fn result_mode_shows_dismiss_footer() {
        let mut state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        state.result = Some(DeleteResultMessage {
            success: true,
            message: "Done".into(),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Enter/Space dismiss"), "result footer should show Enter/Space dismiss");
    }

    #[test]
    fn long_path_is_truncated_with_ellipsis() {
        let long_path = "/home/user/projects/company/very-long-project-name/.worktrees/feature-branch-with-extra-long-name";
        let state = DeleteConfirmState::new("feat-auth", long_path, "feature/auth");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("\u{2026}"), "long path should be truncated with ellipsis");
        assert!(!text.contains(long_path), "full long path should NOT appear in dialog");
    }

    #[test]
    fn long_path_with_multibyte_chars_does_not_panic() {
        // "/tmp/ñ" = 7 bytes (ñ at indices 5-6). With 54 ASCII chars appended,
        // total = 61 bytes. Byte-slice at [61-55..] = [6..] lands on 0xB1,
        // the second byte of ñ — a byte-based slice would panic here.
        let long_path = format!("/tmp/ñ{}", "a".repeat(54));
        let state = DeleteConfirmState::new("feat-auth", &long_path, "feature/auth");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("\u{2026}"),
            "long multibyte path should be truncated with ellipsis"
        );
    }

    #[test]
    fn multiline_result_message_is_fully_visible() {
        let mut state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        state.result = Some(DeleteResultMessage {
            success: false,
            message: "Delete failed: git error\ncaused by: lock file exists".into(),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("git error"),
            "first line of message should be visible"
        );
        assert!(
            text.contains("lock file"),
            "second line of message should be visible"
        );
    }

    #[test]
    fn long_branch_name_is_truncated_in_confirm_dialog() {
        let long_branch =
            "feature/very-long-descriptive-branch-name-that-exceeds-dialog-width";
        let state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", long_branch);
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("\u{2026}"),
            "long branch should be truncated with ellipsis"
        );
        assert!(
            !text.contains(long_branch),
            "full branch name should NOT appear in dialog"
        );
    }

    #[test]
    fn result_footer_mentions_space() {
        let mut state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        state.result = Some(DeleteResultMessage {
            success: true,
            message: "Done".into(),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Space"), "result footer should mention Space key");
    }

    #[test]
    fn confirm_dialog_uses_themed_border_color() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        let buf = render_to_buffer(&state, 80, 20);
        // The dialog is centered in an 80x20 area with width=60, height=10.
        // Centered: x_offset = (80-60)/2 = 10, y_offset = (20-10)/2 = 5
        // Top-left border cell is at (10, 5)
        let cell = buf.cell((10, 5)).unwrap();
        assert_eq!(
            cell.fg, theme.border,
            "confirm dialog border should use theme.border color, got: {:?}",
            cell.fg
        );
    }

    #[test]
    fn result_dialog_uses_themed_border_color() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let mut state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        state.result = Some(DeleteResultMessage {
            success: true,
            message: "Removed successfully".into(),
        });
        let buf = render_to_buffer(&state, 80, 20);
        // Result dialog: centered width=60, height=8
        // x_offset = (80-60)/2 = 10, y_offset = (20-8)/2 = 6
        let cell = buf.cell((10, 6)).unwrap();
        assert_eq!(
            cell.fg, theme.border,
            "result dialog border should use theme.border color, got: {:?}",
            cell.fg
        );
    }

    #[test]
    fn confirm_dialog_renders_as_overlay_through_app() {
        // Verify the dialog renders ON TOP of the list (overlay, not full screen)
        use crate::tui::screens::list::{ListState, WorktreeRow};
        use crate::tui::{App, Screen};

        let mut app = App::new();
        app.list_state = ListState::new(vec![WorktreeRow {
            name: "feat-a".into(),
            branch: "feat/a".into(),
            path: "/tmp/wt/feat-a".into(),
            status: "clean".into(),
            ahead_behind: "+0/-0".into(),
            managed: true,
        }]);
        app.delete_confirm_state = Some(DeleteConfirmState::new(
            "feat-a",
            "/tmp/wt/feat-a",
            "feat/a",
        ));
        app.push_screen(Screen::DeleteConfirm);

        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        // Dialog content should be visible
        assert!(content.contains("Delete Worktree"), "dialog should be visible");
        assert!(content.contains("feat-a"), "dialog should show worktree name");
    }

    #[test]
    fn result_success_title_uses_theme_success_color() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let mut state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        state.result = Some(DeleteResultMessage {
            success: true,
            message: "Removed successfully".into(),
        });
        let buf = render_to_buffer(&state, 80, 20);
        // Find the cell with 'W' from "Worktree removed" by scanning cells
        let has_success_color = buf
            .content()
            .iter()
            .any(|cell| cell.symbol() == "W" && cell.fg == theme.success);
        assert!(
            has_success_color,
            "success result title should have a 'W' cell with theme.success color"
        );
    }

    #[test]
    fn result_failure_title_uses_theme_error_color() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let mut state = DeleteConfirmState::new("feat-auth", "/tmp/wt/feat-auth", "feature/auth");
        state.result = Some(DeleteResultMessage {
            success: false,
            message: "pre_remove hook failed".into(),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let has_error_color = buf
            .content()
            .iter()
            .any(|cell| cell.symbol() == "D" && cell.fg == theme.error);
        assert!(
            has_error_color,
            "failure result title should have a 'D' cell with theme.error color"
        );
    }
}
