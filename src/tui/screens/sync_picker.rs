use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::cli::commands::sync::Strategy;

const SYNC_OPTIONS: [(&str, &str); 2] = [
    ("Rebase", "Replay your commits on top of the base branch"),
    ("Merge", "Create a merge commit combining both branches"),
];

/// View model for the sync strategy picker screen.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncPickerState {
    /// Name of the worktree being synced.
    pub worktree_name: String,
    /// Currently selected option: 0 = Rebase, 1 = Merge.
    pub selected: usize,
    /// Result message after sync execution. None = picker mode, Some = result mode.
    pub result: Option<SyncResultMessage>,
}

/// Outcome displayed after a sync operation completes.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncResultMessage {
    pub success: bool,
    pub message: String,
}

impl SyncPickerState {
    pub fn new(worktree_name: &str) -> Self {
        Self {
            worktree_name: worktree_name.to_string(),
            selected: 0,
            result: None,
        }
    }

    /// Returns the Strategy corresponding to the current selection.
    pub fn confirmed_strategy(&self) -> Strategy {
        match self.selected {
            0 => Strategy::Rebase,
            1 => Strategy::Merge,
            _ => {
                debug_assert!(false, "invalid sync option index: {}", self.selected);
                Strategy::Rebase
            }
        }
    }

    /// Whether the picker is showing a result (post-sync).
    pub fn is_result_mode(&self) -> bool {
        self.result.is_some()
    }

    /// Returns the two strategy options as (label, description) pairs.
    pub fn options(&self) -> &'static [(&'static str, &'static str)] {
        &SYNC_OPTIONS
    }

    pub fn select_next(&mut self) {
        if self.selected < SYNC_OPTIONS.len() - 1 {
            self.selected += 1;
        }
    }

    pub fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

const SYNC_PICKER_FOOTER: &str = " ↑/↓ or j/k select  Enter confirm  Esc cancel ";
const SYNC_RESULT_FOOTER: &str = " Enter/Space dismiss  Esc back ";

pub fn render(state: &SyncPickerState, frame: &mut Frame, area: Rect, _theme: &crate::tui::theme::Theme) {
    if let Some(ref result) = state.result {
        render_result(state, result, frame, area);
    } else {
        render_picker(state, frame, area);
    }
}

fn render_result(
    state: &SyncPickerState,
    result: &SyncResultMessage,
    frame: &mut Frame,
    area: Rect,
) {
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let chunks = Layout::vertical([
        Constraint::Length(3), // title
        Constraint::Min(1),   // result message
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Title
    let status = if result.success { "Sync Complete" } else { "Sync Failed" };
    let title = Line::from(vec![
        Span::styled(status, bold),
        Span::raw(" — "),
        Span::raw(&state.worktree_name),
    ]);
    frame.render_widget(
        Paragraph::new(title).alignment(Alignment::Center),
        chunks[0],
    );

    // Result message
    let lines: Vec<Line> = result.message.lines().map(Line::from).collect();
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        chunks[1],
    );

    // Footer
    let footer = Paragraph::new(Line::from(SYNC_RESULT_FOOTER))
        .style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_widget(footer, chunks[2]);
}

fn render_picker(state: &SyncPickerState, frame: &mut Frame, area: Rect) {
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let chunks = Layout::vertical([
        Constraint::Length(3), // title + blank line
        Constraint::Min(1),   // options
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Title
    let title = Line::from(vec![
        Span::styled("Sync strategy for ", bold),
        Span::styled(&state.worktree_name, bold),
    ]);
    frame.render_widget(
        Paragraph::new(title).alignment(Alignment::Center),
        chunks[0],
    );

    // Options
    let options = state.options();
    let mut lines: Vec<Line> = Vec::new();
    for (i, (label, desc)) in options.iter().enumerate() {
        let marker = if i == state.selected { "▸ " } else { "  " };
        let style = if i == state.selected {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("{marker}{label}"),
            style,
        )));
        lines.push(Line::from(format!("    {desc}")));
        lines.push(Line::from(""));
    }
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        chunks[1],
    );

    // Footer
    let footer = Paragraph::new(Line::from(SYNC_PICKER_FOOTER))
        .style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_widget(footer, chunks[2]);
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

    fn render_to_buffer(state: &SyncPickerState, width: u16, height: u16) -> ratatui::buffer::Buffer {
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
    fn renders_title_with_worktree_name() {
        let state = SyncPickerState::new("feat-auth");
        let buf = render_to_buffer(&state, 80, 15);
        let text = buffer_text(&buf);
        assert!(text.contains("Sync strategy"), "should show title");
        assert!(text.contains("feat-auth"), "should show worktree name");
    }

    #[test]
    fn renders_rebase_and_merge_options() {
        let state = SyncPickerState::new("feat-auth");
        let buf = render_to_buffer(&state, 80, 15);
        let text = buffer_text(&buf);
        assert!(text.contains("Rebase"), "should show Rebase option");
        assert!(text.contains("Merge"), "should show Merge option");
    }

    #[test]
    fn renders_option_descriptions() {
        let state = SyncPickerState::new("feat-auth");
        let buf = render_to_buffer(&state, 80, 15);
        let text = buffer_text(&buf);
        assert!(text.contains("Replay your commits"), "should show Rebase description");
        assert!(text.contains("merge commit"), "should show Merge description");
    }

    #[test]
    fn renders_footer_with_keybindings() {
        let state = SyncPickerState::new("feat-auth");
        let buf = render_to_buffer(&state, 80, 15);
        let text = buffer_text(&buf);
        assert!(text.contains("Enter confirm"), "footer should show Enter confirm");
        assert!(text.contains("Esc cancel"), "footer should show Esc cancel");
    }

    #[test]
    fn confirmed_strategy_returns_rebase_when_selected_is_zero() {
        let state = SyncPickerState::new("feat-auth");
        assert_eq!(state.confirmed_strategy(), Strategy::Rebase);
    }

    #[test]
    fn confirmed_strategy_returns_merge_when_selected_is_one() {
        let mut state = SyncPickerState::new("feat-auth");
        state.selected = 1;
        assert_eq!(state.confirmed_strategy(), Strategy::Merge);
    }

    #[test]
    #[should_panic(expected = "invalid sync option index")]
    fn confirmed_strategy_panics_in_debug_for_invalid_index() {
        let mut state = SyncPickerState::new("feat-auth");
        state.selected = 99;
        let _ = state.confirmed_strategy();
    }

    #[test]
    fn is_result_mode_false_initially() {
        let state = SyncPickerState::new("feat-auth");
        assert!(!state.is_result_mode());
    }

    #[test]
    fn is_result_mode_true_after_setting_result() {
        let mut state = SyncPickerState::new("feat-auth");
        state.result = Some(SyncResultMessage {
            success: true,
            message: "Synced successfully".into(),
        });
        assert!(state.is_result_mode());
    }

    #[test]
    fn selected_option_has_marker() {
        let state = SyncPickerState::new("feat-auth");
        let buf = render_to_buffer(&state, 80, 15);
        let text = buffer_text(&buf);
        assert!(text.contains("▸"), "should show selection marker");
    }

    #[test]
    fn renders_success_result_message() {
        let mut state = SyncPickerState::new("feat-auth");
        state.result = Some(SyncResultMessage {
            success: true,
            message: "Synced 'feat-auth' via rebase".into(),
        });
        let buf = render_to_buffer(&state, 80, 15);
        let text = buffer_text(&buf);
        assert!(text.contains("Synced"), "should show sync result message");
        assert!(text.contains("rebase"), "should show strategy used");
        assert!(!text.contains("▸"), "should NOT show picker marker in result mode");
    }

    #[test]
    fn renders_failure_result_message() {
        let mut state = SyncPickerState::new("feat-auth");
        state.result = Some(SyncResultMessage {
            success: false,
            message: "Sync failed: worktree has uncommitted changes".into(),
        });
        let buf = render_to_buffer(&state, 80, 15);
        let text = buffer_text(&buf);
        assert!(text.contains("failed"), "should show failure message");
    }

    #[test]
    fn result_mode_shows_dismiss_footer() {
        let mut state = SyncPickerState::new("feat-auth");
        state.result = Some(SyncResultMessage {
            success: true,
            message: "Done".into(),
        });
        let buf = render_to_buffer(&state, 80, 15);
        let text = buffer_text(&buf);
        assert!(text.contains("Enter"), "result footer should show Enter to dismiss");
    }
}
