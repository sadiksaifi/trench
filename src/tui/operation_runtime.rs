use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::{
    operation::{
        self, CancellationToken, Emitter, OperationEvent, OperationKind, OperationOutcome,
        OperationRequest, OperationStage,
    },
    tui::operation_modal::{ModalEffect, ModalKey, OperationModal},
};

pub trait RuntimeClock {
    fn now(&self) -> Duration;
}

#[derive(Debug, Clone)]
pub struct SystemRuntimeClock {
    epoch: Instant,
}

impl Default for SystemRuntimeClock {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl RuntimeClock for SystemRuntimeClock {
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }
}

pub enum OperationMessage {
    Event(OperationEvent),
    Completed(Result<OperationOutcome, operation::OperationFailure>),
}

pub trait OperationLauncher {
    fn launch(
        &mut self,
        request: OperationRequest,
        sender: mpsc::Sender<OperationMessage>,
        cancellation: CancellationToken,
    );
}

#[derive(Debug, Default)]
pub struct ThreadOperationLauncher;

struct RuntimeEmitter {
    sender: mpsc::Sender<OperationMessage>,
}

impl Emitter for RuntimeEmitter {
    fn emit(&self, event: OperationEvent) {
        let _ = self.sender.send(OperationMessage::Event(event));
    }
}

impl OperationLauncher for ThreadOperationLauncher {
    fn launch(
        &mut self,
        request: OperationRequest,
        sender: mpsc::Sender<OperationMessage>,
        cancellation: CancellationToken,
    ) {
        std::thread::spawn(move || {
            let emitter = RuntimeEmitter {
                sender: sender.clone(),
            };
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| operation::OperationFailure {
                    stage: OperationStage::Revalidate,
                    mutation_state: operation::MutationState::NotStarted,
                    class: operation::ErrorClass::Internal,
                    message: format!("could not start operation runtime: {error}"),
                })
                .and_then(|runtime| {
                    runtime.block_on(operation::execute_cancellable(
                        request,
                        &emitter,
                        &cancellation,
                    ))
                });
            let _ = sender.send(OperationMessage::Completed(result));
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationRuntimeEffect {
    Succeeded(OperationOutcome),
    Failed {
        stage: OperationStage,
        message: String,
    },
}

pub struct OperationRuntime<L, C> {
    launcher: L,
    clock: C,
    sender: mpsc::Sender<OperationMessage>,
    receiver: mpsc::Receiver<OperationMessage>,
    modal: Option<OperationModal>,
    cancellation: Option<CancellationToken>,
    started_at: Duration,
}

impl<L, C> OperationRuntime<L, C>
where
    L: OperationLauncher,
    C: RuntimeClock,
{
    pub fn new(launcher: L, clock: C) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            launcher,
            clock,
            sender,
            receiver,
            modal: None,
            cancellation: None,
            started_at: Duration::ZERO,
        }
    }

    pub fn start(&mut self, request: OperationRequest) {
        while self.receiver.try_recv().is_ok() {}
        let operation = match &request {
            OperationRequest::Create(_) => OperationKind::Create,
        };
        let cancellation = CancellationToken::default();
        self.started_at = self.clock.now();
        self.modal = Some(OperationModal::new(operation));
        self.cancellation = Some(cancellation.clone());
        self.launcher
            .launch(request, self.sender.clone(), cancellation);
    }

    pub fn tick(&mut self) -> Vec<OperationRuntimeEffect> {
        if let Some(modal) = self.modal.as_mut() {
            modal.tick(self.clock.now().saturating_sub(self.started_at));
        }
        let mut effects = Vec::new();
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                OperationMessage::Event(event) => {
                    if let Some(modal) = self.modal.as_mut() {
                        modal.apply(event);
                    }
                }
                OperationMessage::Completed(Ok(outcome)) => {
                    if let Some(modal) = self.modal.as_mut() {
                        modal.succeed();
                    }
                    self.cancellation = None;
                    effects.push(OperationRuntimeEffect::Succeeded(outcome));
                }
                OperationMessage::Completed(Err(failure)) => {
                    if let Some(modal) = self.modal.as_mut() {
                        modal.fail(&failure);
                    }
                    self.cancellation = None;
                    effects.push(OperationRuntimeEffect::Failed {
                        stage: failure.stage,
                        message: failure.message,
                    });
                }
            }
        }
        effects
    }

    pub fn cancel(&mut self) -> bool {
        let accepted = self
            .modal
            .as_mut()
            .is_some_and(|modal| modal.handle_key(ModalKey::Escape) == Some(ModalEffect::Cancel));
        if accepted {
            if let Some(cancellation) = self.cancellation.as_ref() {
                cancellation.cancel();
            }
        }
        accepted
    }

    pub fn modal(&self) -> Option<&OperationModal> {
        self.modal.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        path::PathBuf,
        rc::Rc,
        sync::mpsc,
        time::Duration,
    };

    use super::*;
    use crate::{
        create_plan::{CreateAction, CreatePlan, CreatePrecondition, HookPolicy},
        operation::{
            CancellationCheck, CreateOutcome, CreateRequest, MutationState, OperationEvent,
            OperationKind, OperationOutcome, OperationRequest, OperationStage,
        },
    };

    #[derive(Clone, Default)]
    struct FakeClock(Rc<Cell<Duration>>);

    impl RuntimeClock for FakeClock {
        fn now(&self) -> Duration {
            self.0.get()
        }
    }

    #[derive(Clone, Default)]
    struct FakeLauncher {
        sender: Rc<RefCell<Option<mpsc::Sender<OperationMessage>>>>,
        cancellation: Rc<RefCell<Option<crate::operation::CancellationToken>>>,
    }

    impl OperationLauncher for FakeLauncher {
        fn launch(
            &mut self,
            _request: OperationRequest,
            sender: mpsc::Sender<OperationMessage>,
            cancellation: crate::operation::CancellationToken,
        ) {
            self.sender.replace(Some(sender));
            self.cancellation.replace(Some(cancellation));
        }
    }

    fn request() -> OperationRequest {
        OperationRequest::Create(CreateRequest {
            plan: CreatePlan {
                dry_run: true,
                action: CreateAction::NewBranch("main".to_string()),
                branch: "feature/auth".to_string(),
                worktree: "feature-auth".to_string(),
                path: PathBuf::from("/worktrees/trench/feature-auth"),
                base: Some("main".to_string()),
                tracking: None,
                hook_policy: HookPolicy::Run,
                preconditions: vec![CreatePrecondition::ValidRef],
                source_oid: Some(git2::Oid::zero()),
            },
            repo_path: PathBuf::from("/repo/trench"),
            worktree_root: PathBuf::from("/worktrees"),
            hooks: None,
        })
    }

    #[test]
    fn adapter_forwards_events_ticks_elapsed_and_delivers_typed_success() {
        let clock = FakeClock::default();
        let launcher = FakeLauncher::default();
        let channel = launcher.sender.clone();
        let mut runtime = OperationRuntime::new(launcher, clock.clone());

        runtime.start(request());
        let sender = channel.borrow().clone().unwrap();
        sender
            .send(OperationMessage::Event(OperationEvent::Started {
                operation: OperationKind::Create,
            }))
            .unwrap();
        sender
            .send(OperationMessage::Event(OperationEvent::StageStarted {
                stage: OperationStage::CreateWorktree,
            }))
            .unwrap();
        clock.0.set(Duration::from_millis(1_250));

        assert!(runtime.tick().is_empty());
        assert_eq!(
            runtime.modal().unwrap().elapsed(),
            Duration::from_millis(1_250)
        );
        assert_eq!(
            runtime.modal().unwrap().current_stage(),
            Some(OperationStage::CreateWorktree)
        );

        let plan = match request() {
            OperationRequest::Create(request) => request.plan,
        };
        sender
            .send(OperationMessage::Completed(Ok(OperationOutcome::Create(
                CreateOutcome {
                    plan: plan.clone(),
                    mutation_state: MutationState::Applied,
                },
            ))))
            .unwrap();
        assert_eq!(
            runtime.tick(),
            [OperationRuntimeEffect::Succeeded(OperationOutcome::Create(
                CreateOutcome {
                    plan,
                    mutation_state: MutationState::Applied,
                }
            ))]
        );
        assert_eq!(
            runtime.modal().unwrap().status(),
            &crate::tui::operation_modal::ModalStatus::Succeeded
        );
    }

    #[test]
    fn adapter_cancels_only_when_the_modal_accepts_escape() {
        let clock = FakeClock::default();
        let launcher = FakeLauncher::default();
        let cancellation = launcher.cancellation.clone();
        let channel = launcher.sender.clone();
        let mut runtime = OperationRuntime::new(launcher, clock);
        runtime.start(request());

        assert!(runtime.cancel());
        assert!(cancellation
            .borrow()
            .as_ref()
            .is_some_and(CancellationCheck::is_cancelled));

        runtime.start(request());
        channel
            .borrow()
            .as_ref()
            .unwrap()
            .send(OperationMessage::Event(OperationEvent::MutationStarted))
            .unwrap();
        runtime.tick();
        assert!(!runtime.cancel());
    }
}
