use std::time::Duration;

use std::sync::{mpsc, Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookStep {
    Copy,
    Run,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookStreamEvent {
    StepStarted {
        step: HookStep,
    },
    Output {
        step: HookStep,
        stream: OutputStream,
        line: String,
    },
    StepFinished {
        step: HookStep,
        success: bool,
        duration: Duration,
    },
}

pub trait HookEmitter: Send + Sync {
    fn emit(&self, event: HookStreamEvent);
}

#[derive(Debug, Default)]
pub struct NoopHookEmitter;

impl HookEmitter for NoopHookEmitter {
    fn emit(&self, _event: HookStreamEvent) {}
}

#[derive(Debug, Clone, Default)]
pub struct RecordingHookEmitter {
    events: Arc<Mutex<Vec<HookStreamEvent>>>,
}

impl RecordingHookEmitter {
    pub fn events(&self) -> Vec<HookStreamEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl HookEmitter for RecordingHookEmitter {
    fn emit(&self, event: HookStreamEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChannelHookEmitter {
    sender: mpsc::Sender<HookStreamEvent>,
}

impl ChannelHookEmitter {
    pub fn new(sender: mpsc::Sender<HookStreamEvent>) -> Self {
        Self { sender }
    }
}

impl HookEmitter for ChannelHookEmitter {
    fn emit(&self, event: HookStreamEvent) {
        let _ = self.sender.send(event);
    }
}

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

/// Temporary adapter for legacy CLI/TUI callers while they migrate to the
/// shared operation event stream.
pub struct LegacyHookEmitter<'a> {
    sender: Option<&'a mpsc::Sender<HookOutputMessage>>,
}

impl<'a> LegacyHookEmitter<'a> {
    pub fn new(sender: Option<&'a mpsc::Sender<HookOutputMessage>>) -> Self {
        Self { sender }
    }
}

impl HookEmitter for LegacyHookEmitter<'_> {
    fn emit(&self, event: HookStreamEvent) {
        let message = match event {
            HookStreamEvent::StepStarted { step } => HookOutputMessage::StepStarted {
                step: step.as_str().to_string(),
            },
            HookStreamEvent::Output { step, stream, line } => HookOutputMessage::OutputLine {
                step: step.as_str().to_string(),
                stream: stream.as_str().to_string(),
                line,
            },
            HookStreamEvent::StepFinished {
                step,
                success,
                duration,
            } => HookOutputMessage::StepCompleted {
                step: step.as_str().to_string(),
                success,
                duration,
            },
        };
        if let Some(sender) = self.sender {
            let _ = sender.send(message);
        } else if let HookOutputMessage::OutputLine { stream, line, .. } = message {
            if stream == "stderr" {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
    }
}

impl HookStep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Run => "run",
            Self::Shell => "shell",
        }
    }
}

impl OutputStream {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
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
