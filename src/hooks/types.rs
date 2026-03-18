use std::time::Duration;

/// A message sent from the hook runner for live streaming of hook execution.
///
/// This type lives in the hooks module (not TUI) so that the hook runner
/// does not depend on UI-layer types.
#[derive(Debug, Clone)]
pub enum HookOutputMessage {
    /// A new hook step (copy/run/shell) has started.
    StepStarted { step: String },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_output_message_is_debug_and_clone() {
        let msg = HookOutputMessage::StepStarted {
            step: "run".to_string(),
        };
        let debug = format!("{msg:?}");
        assert!(debug.contains("StepStarted"));
        let cloned = msg.clone();
        match cloned {
            HookOutputMessage::StepStarted { step } => assert_eq!(step, "run"),
            _ => panic!("expected StepStarted"),
        }
    }
}
