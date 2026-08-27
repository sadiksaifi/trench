use std::time::Duration;

use crate::operation::{OperationEvent, OperationFailure, OperationKind, OperationStage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageView {
    pub stage: OperationStage,
    pub duration: Option<Duration>,
    pub success: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalStatus {
    Running,
    Succeeded,
    Failed {
        stage: OperationStage,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKey {
    Enter,
    Escape,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalEffect {
    Cancel,
    ReturnToForm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationModal {
    operation: OperationKind,
    stages: Vec<StageView>,
    current_stage: Option<OperationStage>,
    elapsed: Duration,
    spinner_tick: u64,
    mutation_started: bool,
    hook_lines: Vec<String>,
    hook_scroll: usize,
    warnings: Vec<String>,
    status: ModalStatus,
}

impl OperationModal {
    pub fn new(operation: OperationKind) -> Self {
        Self {
            operation,
            stages: Vec::new(),
            current_stage: None,
            elapsed: Duration::ZERO,
            spinner_tick: 0,
            mutation_started: operation == OperationKind::Sync,
            hook_lines: Vec::new(),
            hook_scroll: 0,
            warnings: Vec::new(),
            status: ModalStatus::Running,
        }
    }

    pub fn apply(&mut self, event: OperationEvent) {
        match event {
            OperationEvent::Started { operation } => {
                self.operation = operation;
            }
            OperationEvent::StageStarted { stage } => {
                self.current_stage = Some(stage);
                self.stages.push(StageView {
                    stage,
                    duration: None,
                    success: None,
                });
            }
            OperationEvent::MutationStarted => self.mutation_started = true,
            OperationEvent::Output { line, .. } => {
                if self.hook_scroll > 0 {
                    self.hook_scroll = self.hook_scroll.saturating_add(1);
                }
                self.hook_lines.push(line);
            }
            OperationEvent::StageFinished {
                stage,
                duration,
                success,
            } => {
                if let Some(view) = self
                    .stages
                    .iter_mut()
                    .rev()
                    .find(|view| view.stage == stage)
                {
                    view.duration = Some(duration);
                    view.success = Some(success);
                }
                if self.current_stage == Some(stage) {
                    self.current_stage = None;
                }
            }
            OperationEvent::Warning { message, .. } => self.warnings.push(message),
            OperationEvent::Finished { duration, .. } => {
                self.elapsed = duration;
                self.current_stage = None;
            }
        }
    }

    pub fn tick(&mut self, elapsed: Duration) {
        if self.status == ModalStatus::Running {
            self.elapsed = elapsed;
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }
    }

    pub fn succeed(&mut self) {
        self.current_stage = None;
        self.status = ModalStatus::Succeeded;
    }

    pub fn fail(&mut self, failure: &OperationFailure) {
        self.current_stage = Some(failure.stage);
        self.status = ModalStatus::Failed {
            stage: failure.stage,
            message: failure.message.clone(),
        };
    }

    pub fn handle_key(&mut self, key: ModalKey) -> Option<ModalEffect> {
        match key {
            ModalKey::Escape if self.status == ModalStatus::Running && !self.mutation_started => {
                Some(ModalEffect::Cancel)
            }
            ModalKey::Enter if matches!(self.status, ModalStatus::Failed { .. }) => {
                Some(ModalEffect::ReturnToForm)
            }
            ModalKey::Up => {
                self.hook_scroll = self
                    .hook_scroll
                    .saturating_add(1)
                    .min(self.hook_lines.len().saturating_sub(1));
                None
            }
            ModalKey::Down => {
                self.hook_scroll = self.hook_scroll.saturating_sub(1);
                None
            }
            ModalKey::Enter | ModalKey::Escape => None,
        }
    }

    pub fn operation(&self) -> OperationKind {
        self.operation
    }

    pub fn stages(&self) -> &[StageView] {
        &self.stages
    }

    pub fn current_stage(&self) -> Option<OperationStage> {
        self.current_stage
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub fn spinner_tick(&self) -> u64 {
        self.spinner_tick
    }

    pub fn spinner_visible(&self) -> bool {
        self.status == ModalStatus::Running && self.current_stage.is_some()
    }

    pub fn mutation_started(&self) -> bool {
        self.mutation_started
    }

    pub fn visible_hook_lines(&self, height: usize) -> Vec<&str> {
        let end = self.hook_lines.len().saturating_sub(self.hook_scroll);
        let start = end.saturating_sub(height);
        self.hook_lines[start..end]
            .iter()
            .map(String::as_str)
            .collect()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn status(&self) -> &ModalStatus {
        &self.status
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        hooks::{
            types::{HookStep, OutputStream},
            HookEvent,
        },
        operation::{
            ErrorClass, MutationState, OperationEvent, OperationFailure, OperationKind,
            OperationStage,
        },
    };

    #[test]
    fn modal_tracks_stages_elapsed_hooks_and_the_mutation_cancellation_boundary() {
        let mut modal = OperationModal::new(OperationKind::Create);
        modal.apply(OperationEvent::Started {
            operation: OperationKind::Create,
        });
        modal.apply(OperationEvent::StageStarted {
            stage: OperationStage::Revalidate,
        });
        modal.tick(Duration::from_millis(1_250));

        assert_eq!(modal.current_stage(), Some(OperationStage::Revalidate));
        assert_eq!(modal.elapsed(), Duration::from_millis(1_250));
        assert!(modal.spinner_visible());
        assert_eq!(
            modal.handle_key(ModalKey::Escape),
            Some(ModalEffect::Cancel)
        );

        modal.apply(OperationEvent::MutationStarted);
        modal.apply(OperationEvent::Output {
            hook: HookEvent::PreCreate,
            step: HookStep::Run,
            stream: OutputStream::Stdout,
            line: "setting up".to_string(),
        });
        modal.apply(OperationEvent::Output {
            hook: HookEvent::PreCreate,
            step: HookStep::Shell,
            stream: OutputStream::Stderr,
            line: "checking tools".to_string(),
        });

        assert!(modal.mutation_started());
        assert_eq!(modal.handle_key(ModalKey::Escape), None);
        assert_eq!(modal.visible_hook_lines(1), ["checking tools"]);
        modal.handle_key(ModalKey::Up);
        assert_eq!(modal.visible_hook_lines(1), ["setting up"]);

        modal.fail(&OperationFailure {
            stage: OperationStage::PostHook,
            mutation_state: MutationState::RolledBack,
            class: ErrorClass::Hook,
            message: "post-create hook failed".to_string(),
            retained_quarantine: None,
        });
        assert_eq!(
            modal.status(),
            &ModalStatus::Failed {
                stage: OperationStage::PostHook,
                message: "post-create hook failed".to_string(),
            }
        );
        assert_eq!(
            modal.handle_key(ModalKey::Enter),
            Some(ModalEffect::ReturnToForm)
        );
    }

    #[test]
    fn remove_modal_tracks_all_shared_stages_and_locks_cancellation_at_mutation() {
        let mut modal = OperationModal::new(OperationKind::Remove);
        for stage in [
            OperationStage::Revalidate,
            OperationStage::PreHook,
            OperationStage::RemoveWorktree,
            OperationStage::Prune,
            OperationStage::DeleteBranch,
            OperationStage::PostHook,
        ] {
            modal.apply(OperationEvent::StageStarted { stage });
            modal.apply(OperationEvent::StageFinished {
                stage,
                duration: Duration::from_millis(10),
                success: true,
            });
        }
        modal.tick(Duration::from_millis(750));
        assert_eq!(modal.elapsed(), Duration::from_millis(750));
        assert_eq!(modal.stages().len(), 6);
        assert_eq!(
            modal.handle_key(ModalKey::Escape),
            Some(ModalEffect::Cancel)
        );
        modal.apply(OperationEvent::MutationStarted);
        assert_eq!(modal.handle_key(ModalKey::Escape), None);
    }

    #[test]
    fn sync_modal_is_non_cancellable_from_its_first_frame() {
        let mut modal = OperationModal::new(OperationKind::Sync);

        assert!(modal.mutation_started());
        assert_eq!(modal.handle_key(ModalKey::Escape), None);
    }
}
