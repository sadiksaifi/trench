use std::time::Duration;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub use crate::hooks::types::HookOutputMessage;

/// One section of the hook log, corresponding to a single step (copy/run/shell).
#[derive(Debug, Clone)]
pub struct HookLogSection {
    pub step: String,
    pub lines: Vec<HookLogLine>,
    pub completed: bool,
    pub success: bool,
    pub duration: Option<Duration>,
}

/// A single line of hook output with stream label.
#[derive(Debug, Clone)]
pub struct HookLogLine {
    pub stream: String,
    pub text: String,
}

/// TUI state for the hook log screen.
pub struct HookLogState {
    pub title: String,
    pub sections: Vec<HookLogSection>,
    pub completed: bool,
    pub success: bool,
    pub scroll_offset: usize,
    pub error: Option<String>,
    /// True when viewing historical DB data (replay mode), false during live streaming.
    pub replay: bool,
    /// Last rendered body height from `render()`. Used for scroll calculations.
    /// Uses `Cell` for interior mutability so `render(&self)` can update it.
    pub last_body_height: std::cell::Cell<usize>,
}

impl HookLogState {
    /// Build a completed HookLogState from stored DB hook output lines.
    ///
    /// Groups lines by step into sections, marks all sections as completed.
    /// Used for replaying historical hook executions from the logs table.
    pub fn from_hook_output(
        lines: &[crate::state::HookOutputLine],
        event_type: &str,
        payload: &Option<String>,
    ) -> Self {
        let title = event_type.strip_prefix("hook:").unwrap_or(event_type);

        // Extract success from payload exit_code
        let exit_code = payload
            .as_deref()
            .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
            .and_then(|v| v.get("exit_code")?.as_i64());
        let success = exit_code.map_or(true, |c| c == 0);

        // Track timestamps per section for duration computation
        struct SectionTimestamps {
            first: i64,
            last: i64,
        }
        let mut sections: Vec<HookLogSection> = Vec::new();
        let mut timestamps: Vec<SectionTimestamps> = Vec::new();

        for line in lines {
            let step = line.step.as_deref().unwrap_or("unknown");

            let needs_new = sections.last().map_or(true, |s| s.step != step);
            if needs_new {
                sections.push(HookLogSection {
                    step: step.to_string(),
                    lines: Vec::new(),
                    completed: true,
                    success: true,
                    duration: None,
                });
                timestamps.push(SectionTimestamps {
                    first: line.created_at,
                    last: line.created_at,
                });
            }

            timestamps.last_mut().unwrap().last = line.created_at;

            sections.last_mut().unwrap().lines.push(HookLogLine {
                stream: line.stream.clone(),
                text: line.line.clone(),
            });
        }

        // Compute section durations from timestamps
        for (section, ts) in sections.iter_mut().zip(timestamps.iter()) {
            let delta = (ts.last - ts.first).max(0) as u64;
            if delta > 0 {
                section.duration = Some(Duration::from_secs(delta));
            }
        }

        // Mark the last section as failed when exit_code != 0
        if !success {
            if let Some(last) = sections.last_mut() {
                last.success = false;
            }
        }

        Self {
            title: title.to_string(),
            sections,
            completed: true,
            success,
            scroll_offset: 0,
            error: None,
            replay: true,
            last_body_height: std::cell::Cell::new(20),
        }
    }

    /// Create a state representing "no hook history" for a worktree.
    pub fn no_history() -> Self {
        Self {
            title: "Hook Log".to_string(),
            sections: Vec::new(),
            completed: true,
            success: true,
            scroll_offset: 0,
            error: Some("No hook history for this worktree.".to_string()),
            replay: true,
            last_body_height: std::cell::Cell::new(20),
        }
    }

    pub fn new(title: &str) -> Self {
        Self {
            title: title.to_string(),
            sections: Vec::new(),
            completed: false,
            success: false,
            scroll_offset: 0,
            error: None,
            replay: false,
            last_body_height: std::cell::Cell::new(20),
        }
    }

    /// Total number of renderable lines (section headers + output lines + error).
    pub fn total_lines(&self) -> usize {
        let content: usize = self
            .sections
            .iter()
            .map(|s| 1 + s.lines.len()) // 1 header per section + output lines
            .sum();
        content + if self.error.is_some() { 2 } else { 0 }
    }

    /// Scroll up by one line.
    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Scroll down by one line, clamped to max scrollable range.
    pub fn scroll_down(&mut self, visible_height: usize) {
        let max = self.total_lines().saturating_sub(visible_height);
        if self.scroll_offset < max {
            self.scroll_offset += 1;
        }
    }

    /// Page up by half the visible height.
    pub fn page_up(&mut self, visible_height: usize) {
        let step = visible_height / 2;
        self.scroll_offset = self.scroll_offset.saturating_sub(step);
    }

    /// Page down by half the visible height, clamped to max.
    pub fn page_down(&mut self, visible_height: usize) {
        let step = visible_height / 2;
        let max = self.total_lines().saturating_sub(visible_height);
        self.scroll_offset = (self.scroll_offset + step).min(max);
    }

    /// Auto-scroll to keep the latest output visible.
    pub fn auto_scroll(&mut self, visible_height: usize) {
        let total = self.total_lines();
        if total > visible_height {
            self.scroll_offset = total - visible_height;
        } else {
            self.scroll_offset = 0;
        }
    }

    /// Process an incoming message from the hook runner, updating state.
    pub fn process_message(&mut self, msg: HookOutputMessage) {
        match msg {
            HookOutputMessage::StepStarted { step } => {
                self.sections.push(HookLogSection {
                    step,
                    lines: Vec::new(),
                    completed: false,
                    success: false,
                    duration: None,
                });
            }
            HookOutputMessage::OutputLine { step, stream, line } => {
                let section = self.sections.iter_mut().rfind(|s| s.step == step);
                if let Some(section) = section {
                    section.lines.push(HookLogLine { stream, text: line });
                }
            }
            HookOutputMessage::StepCompleted {
                step,
                success,
                duration,
            } => {
                let section = self.sections.iter_mut().rfind(|s| s.step == step);
                if let Some(section) = section {
                    section.completed = true;
                    section.success = success;
                    section.duration = Some(duration);
                }
            }
            HookOutputMessage::HookCompleted { success, error, .. } => {
                self.completed = true;
                self.success = success;
                self.error = error;
            }
        }
    }
}

const FOOTER_RUNNING: &str = " Esc back (hooks continue) ";
const FOOTER_DONE: &str = " Esc back  Enter dismiss ";
const FOOTER_REPLAY: &str = " ↑/↓ scroll  PgUp/PgDn page  Esc back ";

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else {
        format!("{:.1}s", secs)
    }
}

/// Render the hook log screen.
pub fn render(state: &HookLogState, frame: &mut Frame, area: Rect, theme: &crate::tui::theme::Theme) {
    let chunks = Layout::vertical([
        Constraint::Length(2), // title
        Constraint::Min(1),    // output area
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Title
    let status_text = if state.completed {
        if state.success {
            " — Complete"
        } else {
            " — Failed"
        }
    } else {
        " — running..."
    };
    let title = Line::from(vec![
        Span::styled(
            format!("Hook: {}", state.title),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(status_text),
    ]);
    frame.render_widget(Paragraph::new(title), chunks[0]);

    // Build output lines with scrolling
    let mut lines: Vec<Line> = Vec::new();

    for section in &state.sections {
        // Section header
        let header_style = if section.completed {
            if section.success {
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.error).add_modifier(Modifier::BOLD)
            }
        } else {
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD)
        };

        let status_icon = if section.completed {
            if section.success {
                "✓"
            } else {
                "✗"
            }
        } else {
            "●"
        };

        let elapsed = section
            .duration
            .map(|d| format!(" ({})", format_duration(d)))
            .unwrap_or_default();

        lines.push(Line::from(vec![Span::styled(
            format!("{status_icon} [{step}]{elapsed}", step = section.step),
            header_style,
        )]));

        // Output lines
        for log_line in &section.lines {
            let style = if log_line.stream == "stderr" {
                Style::default().fg(theme.error)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("  {}", log_line.text),
                style,
            )));
        }
    }

    // Error message at bottom
    if let Some(ref err) = state.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(theme.error).add_modifier(Modifier::BOLD),
        )));
    }

    // Apply scroll offset
    let visible_height = chunks[1].height as usize;
    state.last_body_height.set(visible_height);
    let skip = state.scroll_offset.min(lines.len());
    let visible_lines: Vec<Line> = lines.into_iter().skip(skip).take(visible_height).collect();

    frame.render_widget(Paragraph::new(visible_lines), chunks[1]);

    // Footer
    let footer_text = if state.replay {
        FOOTER_REPLAY
    } else if state.completed {
        FOOTER_DONE
    } else {
        FOOTER_RUNNING
    };
    let footer = Paragraph::new(Line::from(footer_text))
        .style(Style::default().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD));
    frame.render_widget(footer, chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_output_message_step_started_holds_step_name() {
        let msg = HookOutputMessage::StepStarted {
            step: "run".to_string(),
        };
        match msg {
            HookOutputMessage::StepStarted { step } => assert_eq!(step, "run"),
            _ => panic!("expected StepStarted"),
        }
    }

    #[test]
    fn hook_output_message_output_line_holds_all_fields() {
        let msg = HookOutputMessage::OutputLine {
            step: "run".to_string(),
            stream: "stdout".to_string(),
            line: "hello world".to_string(),
        };
        match msg {
            HookOutputMessage::OutputLine { step, stream, line } => {
                assert_eq!(step, "run");
                assert_eq!(stream, "stdout");
                assert_eq!(line, "hello world");
            }
            _ => panic!("expected OutputLine"),
        }
    }

    #[test]
    fn hook_output_message_step_completed_holds_status_and_duration() {
        let msg = HookOutputMessage::StepCompleted {
            step: "shell".to_string(),
            success: true,
            duration: Duration::from_millis(1500),
        };
        match msg {
            HookOutputMessage::StepCompleted {
                step,
                success,
                duration,
            } => {
                assert_eq!(step, "shell");
                assert!(success);
                assert_eq!(duration, Duration::from_millis(1500));
            }
            _ => panic!("expected StepCompleted"),
        }
    }

    #[test]
    fn hook_output_message_hook_completed_with_error() {
        let msg = HookOutputMessage::HookCompleted {
            success: false,
            duration: Duration::from_secs(5),
            error: Some("command failed".to_string()),
        };
        match msg {
            HookOutputMessage::HookCompleted {
                success,
                duration,
                error,
            } => {
                assert!(!success);
                assert_eq!(duration, Duration::from_secs(5));
                assert_eq!(error.unwrap(), "command failed");
            }
            _ => panic!("expected HookCompleted"),
        }
    }

    #[test]
    fn hook_log_section_starts_empty() {
        let section = HookLogSection {
            step: "copy".to_string(),
            lines: Vec::new(),
            completed: false,
            success: false,
            duration: None,
        };
        assert_eq!(section.step, "copy");
        assert!(section.lines.is_empty());
        assert!(!section.completed);
    }

    #[test]
    fn hook_log_line_holds_stream_and_text() {
        let line = HookLogLine {
            stream: "stderr".to_string(),
            text: "error: not found".to_string(),
        };
        assert_eq!(line.stream, "stderr");
        assert_eq!(line.text, "error: not found");
    }

    #[test]
    fn hook_log_state_starts_empty_and_incomplete() {
        let state = HookLogState::new("post_create");
        assert_eq!(state.title, "post_create");
        assert!(state.sections.is_empty());
        assert!(!state.completed);
        assert!(!state.success);
        assert_eq!(state.scroll_offset, 0);
        assert!(state.error.is_none());
    }

    #[test]
    fn process_step_started_creates_new_section() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted {
            step: "copy".to_string(),
        });
        assert_eq!(state.sections.len(), 1);
        assert_eq!(state.sections[0].step, "copy");
        assert!(!state.sections[0].completed);
    }

    #[test]
    fn process_output_line_adds_to_current_section() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted {
            step: "run".to_string(),
        });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".to_string(),
            stream: "stdout".to_string(),
            line: "installing deps".to_string(),
        });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".to_string(),
            stream: "stderr".to_string(),
            line: "warning: deprecated".to_string(),
        });
        assert_eq!(state.sections[0].lines.len(), 2);
        assert_eq!(state.sections[0].lines[0].text, "installing deps");
        assert_eq!(state.sections[0].lines[0].stream, "stdout");
        assert_eq!(state.sections[0].lines[1].text, "warning: deprecated");
        assert_eq!(state.sections[0].lines[1].stream, "stderr");
    }

    #[test]
    fn process_step_completed_marks_section_done() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted {
            step: "run".to_string(),
        });
        state.process_message(HookOutputMessage::StepCompleted {
            step: "run".to_string(),
            success: true,
            duration: Duration::from_millis(500),
        });
        assert!(state.sections[0].completed);
        assert!(state.sections[0].success);
        assert_eq!(state.sections[0].duration, Some(Duration::from_millis(500)));
    }

    #[test]
    fn process_step_completed_failure() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted {
            step: "shell".to_string(),
        });
        state.process_message(HookOutputMessage::StepCompleted {
            step: "shell".to_string(),
            success: false,
            duration: Duration::from_secs(2),
        });
        assert!(state.sections[0].completed);
        assert!(!state.sections[0].success);
    }

    #[test]
    fn process_hook_completed_marks_state_done() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::HookCompleted {
            success: true,
            duration: Duration::from_secs(3),
            error: None,
        });
        assert!(state.completed);
        assert!(state.success);
        assert!(state.error.is_none());
    }

    #[test]
    fn process_hook_completed_with_error() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::HookCompleted {
            success: false,
            duration: Duration::from_secs(1),
            error: Some("exit code 1".to_string()),
        });
        assert!(state.completed);
        assert!(!state.success);
        assert_eq!(state.error.as_deref(), Some("exit code 1"));
    }

    #[test]
    fn process_multiple_steps_creates_multiple_sections() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted {
            step: "copy".to_string(),
        });
        state.process_message(HookOutputMessage::StepCompleted {
            step: "copy".to_string(),
            success: true,
            duration: Duration::from_millis(100),
        });
        state.process_message(HookOutputMessage::StepStarted {
            step: "run".to_string(),
        });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".to_string(),
            stream: "stdout".to_string(),
            line: "done".to_string(),
        });
        state.process_message(HookOutputMessage::StepCompleted {
            step: "run".to_string(),
            success: true,
            duration: Duration::from_millis(800),
        });
        assert_eq!(state.sections.len(), 2);
        assert_eq!(state.sections[0].step, "copy");
        assert_eq!(state.sections[1].step, "run");
        assert_eq!(state.sections[1].lines.len(), 1);
    }

    #[test]
    fn total_lines_counts_across_all_sections() {
        let mut state = HookLogState::new("test");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".into(),
            stream: "stdout".into(),
            line: "line1".into(),
        });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".into(),
            stream: "stdout".into(),
            line: "line2".into(),
        });
        state.process_message(HookOutputMessage::StepStarted {
            step: "shell".into(),
        });
        state.process_message(HookOutputMessage::OutputLine {
            step: "shell".into(),
            stream: "stdout".into(),
            line: "line3".into(),
        });
        // total_lines = output lines + section headers (1 per section)
        assert_eq!(state.total_lines(), 5); // 2 headers + 3 output lines
    }

    #[test]
    fn auto_scroll_advances_to_latest() {
        let mut state = HookLogState::new("test");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        for i in 0..20 {
            state.process_message(HookOutputMessage::OutputLine {
                step: "run".into(),
                stream: "stdout".into(),
                line: format!("line {i}"),
            });
        }
        // After many lines, auto_scroll should set offset near the end
        state.auto_scroll(10); // visible_height = 10
                               // scroll_offset should be total_lines - visible_height
        let expected = state.total_lines().saturating_sub(10);
        assert_eq!(state.scroll_offset, expected);
    }

    #[test]
    fn auto_scroll_stays_zero_when_content_fits() {
        let mut state = HookLogState::new("test");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".into(),
            stream: "stdout".into(),
            line: "one".into(),
        });
        state.auto_scroll(20); // plenty of room
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn from_hook_output_single_step_creates_one_section() {
        use crate::state::HookOutputLine;

        let lines = vec![
            HookOutputLine {
                stream: "stdout".into(),
                line: "installing deps".into(),
                step: Some("run".into()),
                line_number: 1,
                created_at: 1700000000,
            },
            HookOutputLine {
                stream: "stderr".into(),
                line: "warning: peer dep".into(),
                step: Some("run".into()),
                line_number: 2,
                created_at: 1700000001,
            },
        ];

        let event_type = "hook:post_create";
        let payload: Option<String> = None;

        let state = HookLogState::from_hook_output(&lines, event_type, &payload);

        assert_eq!(state.title, "post_create");
        assert!(state.completed);
        assert_eq!(state.sections.len(), 1);
        assert_eq!(state.sections[0].step, "run");
        assert_eq!(state.sections[0].lines.len(), 2);
        assert_eq!(state.sections[0].lines[0].text, "installing deps");
        assert_eq!(state.sections[0].lines[0].stream, "stdout");
        assert_eq!(state.sections[0].lines[1].text, "warning: peer dep");
        assert_eq!(state.sections[0].lines[1].stream, "stderr");
        assert!(state.sections[0].completed);
    }

    #[test]
    fn from_hook_output_multiple_steps_creates_separate_sections() {
        use crate::state::HookOutputLine;

        let lines = vec![
            HookOutputLine {
                stream: "stdout".into(),
                line: "copied .env".into(),
                step: Some("copy".into()),
                line_number: 1,
                created_at: 1700000000,
            },
            HookOutputLine {
                stream: "stdout".into(),
                line: "installing deps".into(),
                step: Some("run".into()),
                line_number: 2,
                created_at: 1700000001,
            },
            HookOutputLine {
                stream: "stdout".into(),
                line: "dep installed".into(),
                step: Some("run".into()),
                line_number: 3,
                created_at: 1700000002,
            },
            HookOutputLine {
                stream: "stdout".into(),
                line: "migration done".into(),
                step: Some("shell".into()),
                line_number: 4,
                created_at: 1700000003,
            },
        ];

        let state =
            HookLogState::from_hook_output(&lines, "hook:post_create", &None);

        assert_eq!(state.sections.len(), 3);
        assert_eq!(state.sections[0].step, "copy");
        assert_eq!(state.sections[0].lines.len(), 1);
        assert_eq!(state.sections[1].step, "run");
        assert_eq!(state.sections[1].lines.len(), 2);
        assert_eq!(state.sections[2].step, "shell");
        assert_eq!(state.sections[2].lines.len(), 1);
        // All sections completed
        assert!(state.sections.iter().all(|s| s.completed));
    }

    #[test]
    fn scroll_down_increments_offset() {
        let mut state = HookLogState::new("test");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        for i in 0..30 {
            state.process_message(HookOutputMessage::OutputLine {
                step: "run".into(),
                stream: "stdout".into(),
                line: format!("line {i}"),
            });
        }
        state.scroll_offset = 0;
        state.scroll_down(10); // visible_height = 10
        assert_eq!(state.scroll_offset, 1);
    }

    #[test]
    fn scroll_up_decrements_offset() {
        let mut state = HookLogState::new("test");
        state.scroll_offset = 5;
        state.scroll_up();
        assert_eq!(state.scroll_offset, 4);
    }

    #[test]
    fn scroll_up_does_not_go_below_zero() {
        let mut state = HookLogState::new("test");
        state.scroll_offset = 0;
        state.scroll_up();
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn page_down_advances_by_half_page() {
        let mut state = HookLogState::new("test");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        for i in 0..50 {
            state.process_message(HookOutputMessage::OutputLine {
                step: "run".into(),
                stream: "stdout".into(),
                line: format!("line {i}"),
            });
        }
        state.scroll_offset = 0;
        state.page_down(20); // visible_height = 20
        assert_eq!(state.scroll_offset, 10); // half of visible_height
    }

    #[test]
    fn page_up_retreats_by_half_page() {
        let mut state = HookLogState::new("test");
        state.scroll_offset = 15;
        state.page_up(20); // visible_height = 20
        assert_eq!(state.scroll_offset, 5); // 15 - 10
    }

    #[test]
    fn page_up_clamps_to_zero() {
        let mut state = HookLogState::new("test");
        state.scroll_offset = 3;
        state.page_up(20);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn scroll_down_clamps_to_max() {
        let mut state = HookLogState::new("test");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".into(),
            stream: "stdout".into(),
            line: "only line".into(),
        });
        // total_lines = 2 (header + line), visible = 10 → no scrolling possible
        state.scroll_offset = 0;
        state.scroll_down(10);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn no_hook_history_state_shows_message() {
        let state = HookLogState::no_history();
        assert!(state.completed);
        assert!(state.replay);
        assert!(state.sections.is_empty());
        assert!(state.error.as_deref().unwrap().contains("No hook history"));
    }

    #[test]
    fn render_no_history_shows_message() {
        let state = HookLogState::no_history();
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("No hook history"),
            "should show no history message, got: {text}"
        );
    }

    #[test]
    fn replay_footer_shows_scroll_hint() {
        let state = HookLogState::from_hook_output(&[], "hook:post_create", &None);
        assert!(state.replay, "from_hook_output should set replay flag");

        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        // Replay footer should mention scrolling keys, not "hooks continue"
        assert!(
            !text.contains("hooks continue"),
            "replay footer should not mention running hooks"
        );
        assert!(
            text.contains("Esc"),
            "replay footer should show Esc to go back"
        );
    }

    #[test]
    fn live_mode_does_not_set_replay_flag() {
        let state = HookLogState::new("test");
        assert!(!state.replay, "new() should not set replay flag");
    }

    #[test]
    fn from_hook_output_success_from_payload_exit_code_zero() {
        let payload = Some(r#"{"exit_code": 0, "duration_secs": 2.5}"#.to_string());
        let state = HookLogState::from_hook_output(&[], "hook:post_create", &payload);

        assert!(state.success);
        assert!(state.completed);
        assert!(state.error.is_none());
    }

    #[test]
    fn from_hook_output_failure_from_payload_exit_code_nonzero() {
        let payload = Some(r#"{"exit_code": 1, "duration_secs": 0.5}"#.to_string());
        let state = HookLogState::from_hook_output(&[], "hook:post_create", &payload);

        assert!(!state.success);
        assert!(state.completed);
    }

    #[test]
    fn from_hook_output_section_duration_computed_from_timestamps() {
        use crate::state::HookOutputLine;

        let lines = vec![
            HookOutputLine {
                stream: "stdout".into(),
                line: "start".into(),
                step: Some("run".into()),
                line_number: 1,
                created_at: 1700000000,
            },
            HookOutputLine {
                stream: "stdout".into(),
                line: "end".into(),
                step: Some("run".into()),
                line_number: 2,
                created_at: 1700000003,
            },
        ];

        let state = HookLogState::from_hook_output(&lines, "hook:post_create", &None);

        assert_eq!(state.sections.len(), 1);
        let duration = state.sections[0].duration.expect("should have duration");
        assert_eq!(duration, std::time::Duration::from_secs(3));
    }

    #[test]
    fn from_hook_output_empty_lines_produces_empty_sections() {
        let state = HookLogState::from_hook_output(&[], "hook:post_create", &None);

        assert_eq!(state.title, "post_create");
        assert!(state.completed);
        assert!(state.sections.is_empty());
    }

    #[test]
    fn from_hook_output_missing_step_grouped_as_unknown() {
        use crate::state::HookOutputLine;

        let lines = vec![
            HookOutputLine {
                stream: "stdout".into(),
                line: "some output".into(),
                step: None,
                line_number: 1,
                created_at: 1700000000,
            },
        ];

        let state = HookLogState::from_hook_output(&lines, "hook:post_create", &None);

        assert_eq!(state.sections.len(), 1);
        assert_eq!(state.sections[0].step, "unknown");
        assert_eq!(state.sections[0].lines.len(), 1);
    }

    fn render_to_buffer(state: &HookLogState, width: u16, height: u16) -> ratatui::buffer::Buffer {
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
    fn render_shows_title_with_hook_name() {
        let state = HookLogState::new("post_create");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("post_create"),
            "should show hook name in title, got: {text}"
        );
    }

    #[test]
    fn render_shows_section_header() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("run"),
            "should show section header for 'run' step"
        );
    }

    #[test]
    fn render_shows_output_lines() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".into(),
            stream: "stdout".into(),
            line: "installing packages".into(),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("installing packages"),
            "should show output line"
        );
    }

    #[test]
    fn render_shows_elapsed_time_for_completed_step() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        state.process_message(HookOutputMessage::StepCompleted {
            step: "run".into(),
            success: true,
            duration: Duration::from_millis(1500),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("1.5s"),
            "should show elapsed time, got: {text}"
        );
    }

    #[test]
    fn render_shows_footer_with_esc() {
        let state = HookLogState::new("post_create");
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("Esc"), "footer should show Esc keybinding");
    }

    #[test]
    fn render_completed_success_shows_status() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::HookCompleted {
            success: true,
            duration: Duration::from_secs(2),
            error: None,
        });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("Complete") || text.contains("Success"),
            "should show success status, got: {text}"
        );
    }

    #[test]
    fn render_completed_failure_shows_error() {
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::HookCompleted {
            success: false,
            duration: Duration::from_secs(1),
            error: Some("exit code 1".into()),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("exit code 1"),
            "should show error message, got: {text}"
        );
    }

    #[test]
    fn render_success_step_header_uses_theme_success_color() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        state.process_message(HookOutputMessage::StepCompleted {
            step: "run".into(),
            success: true,
            duration: Duration::from_millis(100),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let has_success = buf
            .content()
            .iter()
            .any(|cell| cell.fg == theme.success);
        assert!(has_success, "successful step should use theme.success color");
    }

    #[test]
    fn render_failure_step_header_uses_theme_error_color() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        state.process_message(HookOutputMessage::StepCompleted {
            step: "run".into(),
            success: false,
            duration: Duration::from_millis(100),
        });
        let buf = render_to_buffer(&state, 80, 20);
        let has_error = buf
            .content()
            .iter()
            .any(|cell| cell.fg == theme.error);
        assert!(has_error, "failed step should use theme.error color");
    }

    #[test]
    fn render_with_minimal_theme_uses_ansi_colors() {
        let theme = crate::tui::theme::from_name("minimal");
        let mut state = HookLogState::new("post_create");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        state.process_message(HookOutputMessage::StepCompleted {
            step: "run".into(),
            success: true,
            duration: Duration::from_millis(100),
        });
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(&state, frame, frame.area(), &theme))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let has_green = buf
            .content()
            .iter()
            .any(|cell| cell.fg == ratatui::style::Color::Green);
        assert!(has_green, "minimal theme should use basic ANSI Green for success");
    }

    #[test]
    fn total_lines_includes_error_lines() {
        let mut state = HookLogState::new("test");
        state.process_message(HookOutputMessage::StepStarted { step: "run".into() });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".into(),
            stream: "stdout".into(),
            line: "line1".into(),
        });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".into(),
            stream: "stdout".into(),
            line: "line2".into(),
        });
        // Without error: 1 header + 2 output = 3
        assert_eq!(state.total_lines(), 3);

        state.process_message(HookOutputMessage::HookCompleted {
            success: false,
            duration: Duration::from_secs(1),
            error: Some("command failed".into()),
        });
        // With error: 3 content + 2 error lines (blank + "Error: ...") = 5
        assert_eq!(state.total_lines(), 5);
    }

    #[test]
    fn scroll_down_clamps_to_last_body_height() {
        let mut state = HookLogState::new("test");
        // Add 30 output lines so total > any reasonable visible height
        state.sections.push(HookLogSection {
            step: "run".into(),
            lines: (0..30)
                .map(|i| HookLogLine {
                    stream: "stdout".into(),
                    text: format!("line {i}"),
                })
                .collect(),
            completed: true,
            success: true,
            duration: None,
        });
        // total_lines = 1 header + 30 output = 31
        assert_eq!(state.total_lines(), 31);

        // Set last_body_height to 10 (simulating small terminal)
        state.last_body_height.set(10);

        // Scroll down repeatedly — should clamp at total_lines - 10 = 21
        for _ in 0..25 {
            state.scroll_down(state.last_body_height.get());
        }
        assert_eq!(
            state.scroll_offset, 21,
            "scroll_down should clamp based on last_body_height (10), not 20"
        );
    }

    #[test]
    fn from_hook_output_marks_last_section_failed_on_nonzero_exit() {
        use crate::state::HookOutputLine;

        let lines = vec![
            HookOutputLine {
                stream: "stdout".into(),
                line: "copying files".into(),
                step: Some("copy".into()),
                line_number: 1,
                created_at: 1000,
            },
            HookOutputLine {
                stream: "stderr".into(),
                line: "command failed".into(),
                step: Some("run".into()),
                line_number: 2,
                created_at: 2000,
            },
        ];
        let payload = Some(r#"{"exit_code": 1}"#.to_string());
        let state = HookLogState::from_hook_output(&lines, "hook:post_create", &payload);

        assert!(!state.success, "overall success should be false");
        assert_eq!(state.sections.len(), 2);
        // First section (copy) should remain success
        assert!(state.sections[0].success, "first section should be success");
        // Last section (run) should be marked as failed
        assert!(
            !state.sections[1].success,
            "last section should be marked failed when exit_code != 0"
        );
    }
}
