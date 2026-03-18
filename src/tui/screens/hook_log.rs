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
}
