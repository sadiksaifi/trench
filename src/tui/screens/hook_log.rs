use std::time::Duration;

/// A message sent from the hook runner to the TUI for live streaming.
#[derive(Debug, Clone)]
pub enum HookOutputMessage {
    /// A new hook step (copy/run/shell) has started.
    StepStarted {
        step: String,
    },
    /// A line of output from the current step.
    OutputLine {
        step: String,
        stream: String,
        line: String,
    },
    /// A step completed (success or failure).
    StepCompleted {
        step: String,
        success: bool,
        duration: Duration,
    },
    /// The entire hook execution completed.
    HookCompleted {
        success: bool,
        duration: Duration,
        error: Option<String>,
    },
}

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
}

impl HookLogState {
    pub fn new(title: &str) -> Self {
        Self {
            title: title.to_string(),
            sections: Vec::new(),
            completed: false,
            success: false,
            scroll_offset: 0,
            error: None,
        }
    }

    /// Total number of renderable lines (section headers + output lines).
    pub fn total_lines(&self) -> usize {
        self.sections
            .iter()
            .map(|s| 1 + s.lines.len()) // 1 header per section + output lines
            .sum()
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
                let section = self
                    .sections
                    .iter_mut()
                    .rfind(|s| s.step == step);
                if let Some(section) = section {
                    section.lines.push(HookLogLine { stream, text: line });
                }
            }
            HookOutputMessage::StepCompleted {
                step,
                success,
                duration,
            } => {
                let section = self
                    .sections
                    .iter_mut()
                    .rfind(|s| s.step == step);
                if let Some(section) = section {
                    section.completed = true;
                    section.success = success;
                    section.duration = Some(duration);
                }
            }
            HookOutputMessage::HookCompleted {
                success,
                error,
                ..
            } => {
                self.completed = true;
                self.success = success;
                self.error = error;
            }
        }
    }
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
            step: "run".into(), stream: "stdout".into(), line: "line1".into(),
        });
        state.process_message(HookOutputMessage::OutputLine {
            step: "run".into(), stream: "stdout".into(), line: "line2".into(),
        });
        state.process_message(HookOutputMessage::StepStarted { step: "shell".into() });
        state.process_message(HookOutputMessage::OutputLine {
            step: "shell".into(), stream: "stdout".into(), line: "line3".into(),
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
            step: "run".into(), stream: "stdout".into(), line: "one".into(),
        });
        state.auto_scroll(20); // plenty of room
        assert_eq!(state.scroll_offset, 0);
    }
}
