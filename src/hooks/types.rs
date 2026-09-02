use std::time::Duration;

#[cfg(test)]
use std::sync::{Arc, Mutex};

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

#[cfg(test)]
#[derive(Debug, Default)]
pub struct NoopHookEmitter;

#[cfg(test)]
impl HookEmitter for NoopHookEmitter {
    fn emit(&self, _event: HookStreamEvent) {}
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct RecordingHookEmitter {
    events: Arc<Mutex<Vec<HookStreamEvent>>>,
}

#[cfg(test)]
impl RecordingHookEmitter {
    pub fn events(&self) -> Vec<HookStreamEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl HookEmitter for RecordingHookEmitter {
    fn emit(&self, event: HookStreamEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}
