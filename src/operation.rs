use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::ser::{Serialize, SerializeStruct, Serializer};

use crate::config::HooksConfig;
use crate::create_plan::{CreateAction, CreatePlan, CreatePlanner, HookPolicy};
use crate::{git, logging};

#[derive(Debug)]
pub enum OperationRequest {
    Create(CreateRequest),
    Sync(SyncRequest),
    Remove(RemoveRequest),
}

#[derive(Debug)]
pub struct RemoveRequest {
    pub plan: crate::cli::commands::remove::stateless::RemovalPlan,
    pub hooks: Option<HooksConfig>,
}

#[derive(Debug, Clone)]
pub struct SyncRequest {
    pub plan: crate::cli::commands::sync::stateless::SyncPlan,
    pub hooks: Option<HooksConfig>,
}

#[derive(Debug, Clone)]
pub struct CreateRequest {
    pub plan: CreatePlan,
    pub repo_path: PathBuf,
    pub worktree_root: PathBuf,
    pub hooks: Option<HooksConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Create,
    Sync,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStage {
    Fetch,
    Revalidate,
    PreHook,
    CreateWorktree,
    Sync,
    RemoveWorktree,
    Prune,
    DeleteBranch,
    PostHook,
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationState {
    NotStarted,
    RolledBack,
    Applied,
    PartiallyApplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Cancelled,
    PreconditionsChanged,
    Git,
    Hook,
    HookTimeout,
    Cleanup,
    Io,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationEvent {
    Started {
        operation: OperationKind,
    },
    StageStarted {
        stage: OperationStage,
    },
    MutationStarted,
    Output {
        hook: crate::hooks::HookEvent,
        step: crate::hooks::types::HookStep,
        stream: crate::hooks::types::OutputStream,
        line: String,
    },
    StageFinished {
        stage: OperationStage,
        duration: Duration,
        success: bool,
    },
    Warning {
        stage: OperationStage,
        message: String,
    },
    Finished {
        mutation_state: MutationState,
        duration: Duration,
    },
}

pub trait Emitter: Send + Sync {
    fn emit(&self, event: OperationEvent);
}

#[derive(Debug, Default)]
pub struct NoopEmitter;

impl Emitter for NoopEmitter {
    fn emit(&self, _event: OperationEvent) {}
}

#[derive(Debug, Default)]
pub struct TerminalEmitter;

impl Emitter for TerminalEmitter {
    fn emit(&self, event: OperationEvent) {
        if let OperationEvent::Output { line, .. } = event {
            eprintln!("{line}");
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecordingEmitter {
    events: Arc<Mutex<Vec<OperationEvent>>>,
}

impl RecordingEmitter {
    pub fn events(&self) -> Vec<OperationEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl Emitter for RecordingEmitter {
    fn emit(&self, event: OperationEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChannelEmitter {
    sender: mpsc::Sender<OperationEvent>,
}

impl ChannelEmitter {
    pub fn new(sender: mpsc::Sender<OperationEvent>) -> Self {
        Self { sender }
    }
}

impl Emitter for ChannelEmitter {
    fn emit(&self, event: OperationEvent) {
        let _ = self.sender.send(event);
    }
}

pub trait CancellationCheck: Send + Sync {
    fn is_cancelled(&self) -> bool;

    fn try_begin_mutation(&self) -> bool {
        !self.is_cancelled()
    }
}

#[derive(Debug, Default)]
pub struct NeverCancelled;

impl CancellationCheck for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

const CANCELLATION_OPEN: u8 = 0;
const CANCELLATION_REQUESTED: u8 = 1;
const MUTATION_STARTED: u8 = 2;

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicU8>);

impl CancellationToken {
    pub fn cancel(&self) -> bool {
        self.0
            .compare_exchange(
                CANCELLATION_OPEN,
                CANCELLATION_REQUESTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

impl CancellationCheck for CancellationToken {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == CANCELLATION_REQUESTED
    }

    fn try_begin_mutation(&self) -> bool {
        self.0
            .compare_exchange(
                CANCELLATION_OPEN,
                MUTATION_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationOutcome {
    Create(CreateOutcome),
    Sync(crate::cli::commands::sync::stateless::SyncOutcome),
    Remove(crate::cli::commands::remove::stateless::RemovalOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOutcome {
    pub plan: CreatePlan,
    pub mutation_state: MutationState,
}

impl Serialize for CreateOutcome {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut output = serializer.serialize_struct("CreateOutcome", 9)?;
        output.serialize_field("dry_run", &false)?;
        output.serialize_field("action", self.plan.action.name())?;
        output.serialize_field("branch", &self.plan.branch)?;
        output.serialize_field("worktree", &self.plan.worktree)?;
        output.serialize_field("path", &self.plan.path)?;
        output.serialize_field("base", &self.plan.base)?;
        output.serialize_field("tracking", &self.plan.tracking)?;
        output.serialize_field("hook_policy", &self.plan.hook_policy)?;
        output.serialize_field("mutation_state", &self.mutation_state)?;
        output.end()
    }
}

#[derive(Debug, thiserror::Error, serde::Serialize)]
#[error("{message}")]
pub struct OperationFailure {
    pub stage: OperationStage,
    pub mutation_state: MutationState,
    pub class: ErrorClass,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained_quarantine: Option<PathBuf>,
}

pub async fn execute(
    request: OperationRequest,
    emitter: &dyn Emitter,
) -> Result<OperationOutcome, OperationFailure> {
    execute_cancellable(request, emitter, &NeverCancelled).await
}

pub async fn execute_cancellable(
    request: OperationRequest,
    emitter: &dyn Emitter,
    cancellation: &dyn CancellationCheck,
) -> Result<OperationOutcome, OperationFailure> {
    match request {
        OperationRequest::Create(request) => execute_create(request, emitter, cancellation).await,
        OperationRequest::Sync(request) => execute_sync(request, emitter, cancellation).await,
        OperationRequest::Remove(request) => execute_remove(request, emitter, cancellation).await,
    }
}

async fn execute_sync(
    request: SyncRequest,
    emitter: &dyn Emitter,
    cancellation: &dyn CancellationCheck,
) -> Result<OperationOutcome, OperationFailure> {
    use crate::cli::commands::sync::stateless as sync;

    emitter.emit(OperationEvent::Started {
        operation: OperationKind::Sync,
    });
    if !cancellation.try_begin_mutation() {
        return Err(OperationFailure {
            stage: OperationStage::Revalidate,
            mutation_state: MutationState::NotStarted,
            class: ErrorClass::Cancelled,
            message: "operation cancelled before sync started".to_string(),
            retained_quarantine: None,
        });
    }
    emitter.emit(OperationEvent::MutationStarted);
    let adapter = SyncEmitterAdapter { emitter };
    match sync::execute(request.plan, request.hooks.as_ref(), &adapter).await {
        Ok(outcome) => {
            emitter.emit(OperationEvent::Finished {
                mutation_state: map_sync_mutation(outcome.mutation_state),
                duration: outcome.elapsed,
            });
            Ok(OperationOutcome::Sync(outcome))
        }
        Err(failure) => {
            emitter.emit(OperationEvent::Finished {
                mutation_state: map_sync_mutation(failure.mutation_state),
                duration: failure.elapsed,
            });
            Err(OperationFailure {
                stage: map_sync_stage(failure.stage),
                mutation_state: map_sync_mutation(failure.mutation_state),
                class: map_sync_error(failure.class),
                message: failure.message,
                retained_quarantine: None,
            })
        }
    }
}

struct SyncEmitterAdapter<'a> {
    emitter: &'a dyn Emitter,
}

impl crate::cli::commands::sync::stateless::SyncEmitter for SyncEmitterAdapter<'_> {
    fn emit(&self, event: crate::cli::commands::sync::stateless::SyncEvent) {
        use crate::cli::commands::sync::stateless::SyncEvent;

        let event = match event {
            SyncEvent::StageStarted(stage) => OperationEvent::StageStarted {
                stage: map_sync_stage(stage),
            },
            SyncEvent::HookOutput {
                hook,
                step,
                stream,
                line,
            } => OperationEvent::Output {
                hook,
                step,
                stream,
                line,
            },
            SyncEvent::StageFinished {
                stage,
                success,
                elapsed,
            } => OperationEvent::StageFinished {
                stage: map_sync_stage(stage),
                duration: elapsed,
                success,
            },
            SyncEvent::Warning { stage, message } => OperationEvent::Warning {
                stage: map_sync_stage(stage),
                message,
            },
        };
        self.emitter.emit(event);
    }
}

fn map_sync_stage(stage: crate::cli::commands::sync::stateless::SyncStage) -> OperationStage {
    use crate::cli::commands::sync::stateless::SyncStage;
    match stage {
        SyncStage::Fetch => OperationStage::Fetch,
        SyncStage::Validate => OperationStage::Revalidate,
        SyncStage::PreHook => OperationStage::PreHook,
        SyncStage::Sync => OperationStage::Sync,
        SyncStage::Rollback => OperationStage::Rollback,
        SyncStage::PostHook => OperationStage::PostHook,
    }
}

fn map_sync_mutation(state: crate::cli::commands::sync::stateless::MutationState) -> MutationState {
    use crate::cli::commands::sync::stateless::MutationState as SyncMutationState;
    match state {
        SyncMutationState::NotStarted => MutationState::NotStarted,
        SyncMutationState::RolledBack => MutationState::RolledBack,
        SyncMutationState::Applied => MutationState::Applied,
        SyncMutationState::PartiallyApplied => MutationState::PartiallyApplied,
    }
}

fn map_sync_error(class: crate::cli::commands::sync::stateless::SyncErrorClass) -> ErrorClass {
    use crate::cli::commands::sync::stateless::SyncErrorClass;
    match class {
        SyncErrorClass::InvalidTarget
        | SyncErrorClass::InvalidBase
        | SyncErrorClass::Dirty
        | SyncErrorClass::Detached
        | SyncErrorClass::OperationInProgress
        | SyncErrorClass::PreconditionsChanged => ErrorClass::PreconditionsChanged,
        SyncErrorClass::Conflict => ErrorClass::Git,
        SyncErrorClass::Git => ErrorClass::Git,
        SyncErrorClass::Hook => ErrorClass::Hook,
        SyncErrorClass::HookTimeout => ErrorClass::HookTimeout,
        SyncErrorClass::Rollback => ErrorClass::Cleanup,
    }
}

async fn execute_remove(
    request: RemoveRequest,
    emitter: &dyn Emitter,
    cancellation: &dyn CancellationCheck,
) -> Result<OperationOutcome, OperationFailure> {
    use crate::cli::commands::remove::stateless as remove;

    if cancellation.is_cancelled() {
        return Err(OperationFailure {
            stage: OperationStage::Revalidate,
            mutation_state: MutationState::NotStarted,
            class: ErrorClass::Cancelled,
            message: "operation cancelled before mutation".to_string(),
            retained_quarantine: None,
        });
    }
    let adapter = RemoveEmitterAdapter { emitter };
    let guard = RemoveMutationGuard {
        cancellation,
        emitter,
    };
    remove::execute_with_guard(request.plan, request.hooks.as_ref(), &adapter, &guard)
        .await
        .map(OperationOutcome::Remove)
        .map_err(|failure| OperationFailure {
            stage: map_remove_stage(failure.stage),
            mutation_state: map_remove_mutation(failure.mutation_state),
            class: match failure.class {
                remove::RemovalErrorClass::Cancelled => ErrorClass::Cancelled,
                remove::RemovalErrorClass::PreconditionsChanged => ErrorClass::PreconditionsChanged,
                remove::RemovalErrorClass::Git => ErrorClass::Git,
                remove::RemovalErrorClass::Hook => ErrorClass::Hook,
                remove::RemovalErrorClass::HookTimeout => ErrorClass::HookTimeout,
                remove::RemovalErrorClass::Io => ErrorClass::Io,
            },
            message: failure.message,
            retained_quarantine: failure.retained_quarantine,
        })
}

struct RemoveMutationGuard<'a> {
    cancellation: &'a dyn CancellationCheck,
    emitter: &'a dyn Emitter,
}

impl crate::cli::commands::remove::stateless::RemovalMutationGuard for RemoveMutationGuard<'_> {
    fn try_begin_mutation(&self) -> bool {
        let accepted = self.cancellation.try_begin_mutation();
        if accepted {
            self.emitter.emit(OperationEvent::MutationStarted);
        }
        accepted
    }
}

struct RemoveEmitterAdapter<'a> {
    emitter: &'a dyn Emitter,
}

impl crate::cli::commands::remove::stateless::RemovalEventSink for RemoveEmitterAdapter<'_> {
    fn emit(&self, event: crate::cli::commands::remove::stateless::RemovalEvent) {
        use crate::cli::commands::remove::stateless::RemovalEvent;

        let event = match event {
            RemovalEvent::Started => OperationEvent::Started {
                operation: OperationKind::Remove,
            },
            RemovalEvent::StageStarted { stage } => OperationEvent::StageStarted {
                stage: map_remove_stage(stage),
            },
            RemovalEvent::Output {
                hook,
                step,
                stream,
                line,
            } => OperationEvent::Output {
                hook,
                step,
                stream,
                line,
            },
            RemovalEvent::StageFinished {
                stage,
                duration,
                success,
            } => OperationEvent::StageFinished {
                stage: map_remove_stage(stage),
                duration,
                success,
            },
            RemovalEvent::Warning { stage, message } => OperationEvent::Warning {
                stage: map_remove_stage(stage),
                message,
            },
            RemovalEvent::Finished {
                mutation_state,
                duration,
            } => OperationEvent::Finished {
                mutation_state: map_remove_mutation(mutation_state),
                duration,
            },
        };
        self.emitter.emit(event);
    }
}

fn map_remove_stage(
    stage: crate::cli::commands::remove::stateless::RemovalStage,
) -> OperationStage {
    use crate::cli::commands::remove::stateless::RemovalStage;
    match stage {
        RemovalStage::Revalidate => OperationStage::Revalidate,
        RemovalStage::PreRemove => OperationStage::PreHook,
        RemovalStage::RemoveWorktree => OperationStage::RemoveWorktree,
        RemovalStage::Prune => OperationStage::Prune,
        RemovalStage::DeleteBranch => OperationStage::DeleteBranch,
        RemovalStage::PostRemove => OperationStage::PostHook,
    }
}

fn map_remove_mutation(
    state: crate::cli::commands::remove::stateless::RemovalMutationState,
) -> MutationState {
    use crate::cli::commands::remove::stateless::RemovalMutationState;
    match state {
        RemovalMutationState::NotStarted => MutationState::NotStarted,
        RemovalMutationState::Applied => MutationState::Applied,
        RemovalMutationState::PartiallyApplied => MutationState::PartiallyApplied,
    }
}

async fn execute_create(
    request: CreateRequest,
    emitter: &dyn Emitter,
    cancellation: &dyn CancellationCheck,
) -> Result<OperationOutcome, OperationFailure> {
    execute_create_with_boundary(request, emitter, cancellation, &ProductionCreateBoundary).await
}

trait CreateBoundary: Send + Sync {
    fn after_final_verify(&self) {}
}

struct ProductionCreateBoundary;

impl CreateBoundary for ProductionCreateBoundary {}

async fn execute_create_with_boundary(
    request: CreateRequest,
    emitter: &dyn Emitter,
    cancellation: &dyn CancellationCheck,
    boundary: &dyn CreateBoundary,
) -> Result<OperationOutcome, OperationFailure> {
    let operation_started = Instant::now();
    emitter.emit(OperationEvent::Started {
        operation: OperationKind::Create,
    });

    let stage_started = Instant::now();
    emitter.emit(OperationEvent::StageStarted {
        stage: OperationStage::Revalidate,
    });
    let live_plan = match revalidate(&request) {
        Ok(plan) => plan,
        Err(message) => {
            finish_stage(emitter, OperationStage::Revalidate, stage_started, false);
            return Err(fail(
                emitter,
                operation_started,
                OperationStage::Revalidate,
                MutationState::NotStarted,
                ErrorClass::PreconditionsChanged,
                message,
            ));
        }
    };
    if live_plan != request.plan {
        finish_stage(emitter, OperationStage::Revalidate, stage_started, false);
        return Err(fail(
            emitter,
            operation_started,
            OperationStage::Revalidate,
            MutationState::NotStarted,
            ErrorClass::PreconditionsChanged,
            "create plan no longer matches live Git state".to_string(),
        ));
    }
    finish_stage(emitter, OperationStage::Revalidate, stage_started, true);

    if let CreateAction::Navigate(_) = &request.plan.action {
        let outcome = CreateOutcome {
            plan: request.plan,
            mutation_state: MutationState::NotStarted,
        };
        finish_operation(emitter, operation_started, MutationState::NotStarted);
        return Ok(OperationOutcome::Create(outcome));
    }

    if cancellation.is_cancelled() {
        return Err(fail(
            emitter,
            operation_started,
            OperationStage::Revalidate,
            MutationState::NotStarted,
            ErrorClass::Cancelled,
            "operation cancelled before mutation".to_string(),
        ));
    }
    let hook_context = create_hook_context(&request);
    if request.plan.hook_policy == HookPolicy::Run {
        if let Some(pre_create) = request
            .hooks
            .as_ref()
            .and_then(|hooks| hooks.pre_create.as_ref())
        {
            let stage_started = Instant::now();
            emitter.emit(OperationEvent::StageStarted {
                stage: OperationStage::PreHook,
            });
            let hook_emitter = OperationHookEmitter {
                operation_emitter: emitter,
                hook: crate::hooks::HookEvent::PreCreate,
            };
            if let Err(error) = crate::hooks::runner::execute_hook(
                &crate::hooks::HookEvent::PreCreate,
                pre_create,
                &hook_context,
                &request.repo_path,
                &request.repo_path,
                &hook_emitter,
            )
            .await
            {
                finish_stage(emitter, OperationStage::PreHook, stage_started, false);
                let class = classify_hook_error(&error);
                return Err(fail(
                    emitter,
                    operation_started,
                    OperationStage::PreHook,
                    MutationState::RolledBack,
                    class,
                    error.to_string(),
                ));
            }
            finish_stage(emitter, OperationStage::PreHook, stage_started, true);
        }
    }

    // Hooks and concurrent actors may change refs or filesystem components.
    // Revalidate the frozen plan at the last boundary before any Git or path
    // mutation, including its hidden source commit OID.
    let stage_started = Instant::now();
    emitter.emit(OperationEvent::StageStarted {
        stage: OperationStage::Revalidate,
    });
    let live_plan = match revalidate(&request) {
        Ok(plan) => plan,
        Err(message) => {
            finish_stage(emitter, OperationStage::Revalidate, stage_started, false);
            return Err(fail(
                emitter,
                operation_started,
                OperationStage::Revalidate,
                MutationState::RolledBack,
                ErrorClass::PreconditionsChanged,
                message,
            ));
        }
    };
    if live_plan != request.plan {
        finish_stage(emitter, OperationStage::Revalidate, stage_started, false);
        return Err(fail(
            emitter,
            operation_started,
            OperationStage::Revalidate,
            MutationState::RolledBack,
            ErrorClass::PreconditionsChanged,
            "create plan changed before Git mutation".to_string(),
        ));
    }
    finish_stage(emitter, OperationStage::Revalidate, stage_started, true);

    let stage_started = Instant::now();
    emitter.emit(OperationEvent::StageStarted {
        stage: OperationStage::CreateWorktree,
    });
    let prepared_parent = match prepare_parent_directory(&request.plan.path, &request.worktree_root)
    {
        Ok(prepared) => prepared,
        Err(error) => {
            finish_stage(
                emitter,
                OperationStage::CreateWorktree,
                stage_started,
                false,
            );
            let rollback_started = Instant::now();
            emitter.emit(OperationEvent::StageStarted {
                stage: OperationStage::Rollback,
            });
            match cleanup_empty_directories(&error.created) {
                Ok(()) => {
                    finish_stage(emitter, OperationStage::Rollback, rollback_started, true);
                    return Err(fail(
                        emitter,
                        operation_started,
                        OperationStage::CreateWorktree,
                        MutationState::RolledBack,
                        error.class,
                        format!("failed to create worktree parent: {}", error.source),
                    ));
                }
                Err(cleanup_error) => {
                    finish_stage(emitter, OperationStage::Rollback, rollback_started, false);
                    return Err(fail(
                        emitter,
                        operation_started,
                        OperationStage::Rollback,
                        MutationState::PartiallyApplied,
                        ErrorClass::Cleanup,
                        format!("failed to create worktree parent and clean up: {cleanup_error}"),
                    ));
                }
            }
        }
    };

    // Parent creation can race with refs and filesystem actors. Confirm the
    // complete frozen plan once more, then prove the path still resolves to
    // the exact directory handle prepared without following new symlinks.
    let live_plan = revalidate(&request);
    let path_verification = prepared_parent.verify(&request.plan.path);
    let plan_matches = live_plan.as_ref().is_ok_and(|plan| plan == &request.plan);
    if !plan_matches || path_verification.is_err() {
        finish_stage(
            emitter,
            OperationStage::CreateWorktree,
            stage_started,
            false,
        );
        let message = live_plan
            .err()
            .or_else(|| path_verification.err().map(|error| error.to_string()))
            .unwrap_or_else(|| "create plan changed during path preparation".to_string());
        return Err(rollback_failure(
            &request.repo_path,
            emitter,
            operation_started,
            &prepared_parent.created,
            None,
            FailureCause {
                stage: OperationStage::CreateWorktree,
                class: ErrorClass::PreconditionsChanged,
                message,
            },
        ));
    }
    boundary.after_final_verify();

    if !cancellation.try_begin_mutation() {
        finish_stage(
            emitter,
            OperationStage::CreateWorktree,
            stage_started,
            false,
        );
        return Err(rollback_failure(
            &request.repo_path,
            emitter,
            operation_started,
            &prepared_parent.created,
            None,
            FailureCause {
                stage: OperationStage::CreateWorktree,
                class: ErrorClass::Cancelled,
                message: "operation cancelled before Git mutation".to_string(),
            },
        ));
    }
    emitter.emit(OperationEvent::MutationStarted);

    let target = git::create::CreateTarget {
        planned_path: &request.plan.path,
        #[cfg(unix)]
        parent_directory: Arc::clone(&prepared_parent.directory),
    };

    let source_oid = request
        .plan
        .source_oid
        .expect("mutating create plans always freeze a source commit");
    let create_result = match &request.plan.action {
        CreateAction::NewBranch(base) => git::create::add_new_branch(
            &request.repo_path,
            &request.plan.worktree,
            &request.plan.branch,
            base,
            source_oid,
            target,
        ),
        CreateAction::ExistingLocal => git::create::add_existing_local(
            &request.repo_path,
            &request.plan.worktree,
            &request.plan.branch,
            source_oid,
            target,
        ),
        CreateAction::TrackRemote(upstream) => git::create::add_tracking_branch(
            &request.repo_path,
            &request.plan.worktree,
            &request.plan.branch,
            upstream,
            source_oid,
            target,
        ),
        CreateAction::Navigate(_) => unreachable!("navigate returned before mutation"),
    };

    let receipt = match create_result {
        Ok(receipt) => receipt,
        Err(error) => {
            finish_stage(
                emitter,
                OperationStage::CreateWorktree,
                stage_started,
                false,
            );
            let class = if matches!(error, git::GitError::PreconditionsChanged) {
                ErrorClass::PreconditionsChanged
            } else {
                ErrorClass::Git
            };
            return Err(rollback_failure(
                &request.repo_path,
                emitter,
                operation_started,
                &prepared_parent.created,
                None,
                FailureCause {
                    stage: OperationStage::CreateWorktree,
                    class,
                    message: error.to_string(),
                },
            ));
        }
    };
    let expected_path = prepared_parent.expected_path(&request.plan.path);
    if receipt.recorded_path.as_ref() != Some(&expected_path) {
        finish_stage(
            emitter,
            OperationStage::CreateWorktree,
            stage_started,
            false,
        );
        return Err(rollback_failure(
            &request.repo_path,
            emitter,
            operation_started,
            &prepared_parent.created,
            Some(&receipt),
            FailureCause {
                stage: OperationStage::CreateWorktree,
                class: ErrorClass::PreconditionsChanged,
                message: "Git recorded a different worktree destination than planned".to_string(),
            },
        ));
    }
    finish_stage(emitter, OperationStage::CreateWorktree, stage_started, true);

    if request.plan.hook_policy == HookPolicy::Run {
        if let Some(post_create) = request
            .hooks
            .as_ref()
            .and_then(|hooks| hooks.post_create.as_ref())
        {
            let stage_started = Instant::now();
            emitter.emit(OperationEvent::StageStarted {
                stage: OperationStage::PostHook,
            });
            let hook_emitter = OperationHookEmitter {
                operation_emitter: emitter,
                hook: crate::hooks::HookEvent::PostCreate,
            };
            if let Err(error) = crate::hooks::runner::execute_hook(
                &crate::hooks::HookEvent::PostCreate,
                post_create,
                &hook_context,
                &request.repo_path,
                &request.plan.path,
                &hook_emitter,
            )
            .await
            {
                finish_stage(emitter, OperationStage::PostHook, stage_started, false);
                let class = classify_hook_error(&error);
                return Err(rollback_failure(
                    &request.repo_path,
                    emitter,
                    operation_started,
                    &prepared_parent.created,
                    Some(&receipt),
                    FailureCause {
                        stage: OperationStage::PostHook,
                        class,
                        message: error.to_string(),
                    },
                ));
            }
            finish_stage(emitter, OperationStage::PostHook, stage_started, true);
        }
    }

    let outcome = CreateOutcome {
        plan: request.plan,
        mutation_state: MutationState::Applied,
    };
    finish_operation(emitter, operation_started, MutationState::Applied);
    Ok(OperationOutcome::Create(outcome))
}

fn revalidate(request: &CreateRequest) -> Result<CreatePlan, String> {
    let planner = CreatePlanner::discover(
        &request.repo_path,
        &request.worktree_root,
        None,
        request.plan.hook_policy,
    )
    .map_err(|error| error.to_string())?;
    let from = match &request.plan.action {
        CreateAction::NewBranch(_) => request.plan.base.as_deref(),
        _ => None,
    };
    planner
        .plan(&request.plan.branch, from)
        .map_err(|error| error.to_string())
}

#[derive(Debug)]
struct ParentCreationFailure {
    source: std::io::Error,
    created: Vec<CreatedDirectory>,
    class: ErrorClass,
}

#[derive(Debug, Clone)]
struct CreatedDirectory {
    path: PathBuf,
    #[cfg(unix)]
    parent_directory: Arc<std::fs::File>,
    #[cfg(unix)]
    leaf: std::ffi::OsString,
    #[cfg(unix)]
    identity: (u64, u64),
}

struct PreparedParent {
    parent: PathBuf,
    created: Vec<CreatedDirectory>,
    #[cfg(unix)]
    directory: Arc<std::fs::File>,
    #[cfg(unix)]
    identity: (u64, u64),
    canonical_parent: PathBuf,
}

impl PreparedParent {
    fn verify(&self, target: &Path) -> std::io::Result<()> {
        if std::fs::symlink_metadata(target).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "planned worktree path appeared during execution",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let live = std::fs::File::open(&self.parent)?.metadata()?;
            if (live.dev(), live.ino()) != self.identity {
                return Err(std::io::Error::other(
                    "planned worktree parent changed during execution",
                ));
            }
            let held = self.directory.metadata()?;
            if (held.dev(), held.ino()) != self.identity {
                return Err(std::io::Error::other(
                    "prepared worktree parent identity changed",
                ));
            }
        }
        #[cfg(not(unix))]
        if self.parent.canonicalize()? != self.parent {
            return Err(std::io::Error::other(
                "planned worktree parent changed during execution",
            ));
        }
        Ok(())
    }

    fn expected_path(&self, target: &Path) -> PathBuf {
        self.canonical_parent
            .join(target.file_name().expect("planned path has a leaf"))
    }
}

#[cfg(unix)]
fn prepare_parent_directory(
    target: &Path,
    worktree_root: &Path,
) -> Result<PreparedParent, ParentCreationFailure> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    let Some(parent) = target.parent() else {
        return Err(ParentCreationFailure {
            source: std::io::Error::other("planned worktree has no parent"),
            created: Vec::new(),
            class: ErrorClass::PreconditionsChanged,
        });
    };
    if !parent.is_absolute() || !worktree_root.is_absolute() || !target.starts_with(worktree_root) {
        return Err(ParentCreationFailure {
            source: std::io::Error::other(
                "planned worktree path must be absolute and contained by its root",
            ),
            created: Vec::new(),
            class: ErrorClass::PreconditionsChanged,
        });
    }

    // Resolve only the pre-root prefix so platform aliases such as macOS /var
    // remain usable. Every configured-root and repository component is then
    // opened relative to a held directory FD with O_NOFOLLOW.
    let mut lexical_anchor = worktree_root
        .parent()
        .ok_or_else(|| ParentCreationFailure {
            source: std::io::Error::other("worktree root has no parent"),
            created: Vec::new(),
            class: ErrorClass::PreconditionsChanged,
        })?;
    while !lexical_anchor.exists() {
        lexical_anchor = lexical_anchor
            .parent()
            .ok_or_else(|| ParentCreationFailure {
                source: std::io::Error::other("no existing worktree root anchor"),
                created: Vec::new(),
                class: ErrorClass::PreconditionsChanged,
            })?;
    }
    let canonical_anchor =
        lexical_anchor
            .canonicalize()
            .map_err(|source| ParentCreationFailure {
                source,
                created: Vec::new(),
                class: ErrorClass::PreconditionsChanged,
            })?;

    // Walk the canonical anchor from the filesystem root so O_NOFOLLOW still
    // protects every component against changes after canonicalization.
    let root = CString::new("/").expect("the root path has no NUL byte");
    let root_fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if root_fd < 0 {
        return Err(ParentCreationFailure {
            source: std::io::Error::last_os_error(),
            created: Vec::new(),
            class: ErrorClass::Io,
        });
    }
    let mut directory = Arc::new(unsafe { std::fs::File::from_raw_fd(root_fd) });
    let mut current_path = PathBuf::from("/");
    let mut created = Vec::new();
    for component in canonical_anchor.components() {
        let name = match component {
            std::path::Component::RootDir => continue,
            std::path::Component::Normal(name) => name,
            _ => {
                return Err(ParentCreationFailure {
                    source: std::io::Error::other("invalid worktree parent component"),
                    created,
                    class: ErrorClass::PreconditionsChanged,
                });
            }
        };
        let name_c = CString::new(name.as_bytes()).map_err(|_| ParentCreationFailure {
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "worktree path contains a NUL byte",
            ),
            created: created.clone(),
            class: ErrorClass::PreconditionsChanged,
        })?;
        current_path.push(name);
        let next_fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if next_fd < 0 {
            return Err(ParentCreationFailure {
                source: std::io::Error::last_os_error(),
                created,
                class: ErrorClass::PreconditionsChanged,
            });
        }
        directory = Arc::new(unsafe { std::fs::File::from_raw_fd(next_fd) });
    }
    let anchor_metadata =
        std::fs::metadata(lexical_anchor).map_err(|source| ParentCreationFailure {
            source,
            created: created.clone(),
            class: ErrorClass::PreconditionsChanged,
        })?;
    let held_anchor_metadata = directory
        .metadata()
        .map_err(|source| ParentCreationFailure {
            source,
            created: created.clone(),
            class: ErrorClass::Io,
        })?;
    if (anchor_metadata.dev(), anchor_metadata.ino())
        != (held_anchor_metadata.dev(), held_anchor_metadata.ino())
    {
        return Err(ParentCreationFailure {
            source: std::io::Error::other("worktree root anchor changed during execution"),
            created,
            class: ErrorClass::PreconditionsChanged,
        });
    }

    for component in parent
        .strip_prefix(lexical_anchor)
        .expect("worktree parent is below its root anchor")
        .components()
    {
        let std::path::Component::Normal(name) = component else {
            return Err(ParentCreationFailure {
                source: std::io::Error::other("invalid worktree parent component"),
                created,
                class: ErrorClass::PreconditionsChanged,
            });
        };
        let name_c = CString::new(name.as_bytes()).map_err(|_| ParentCreationFailure {
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "worktree path contains a NUL byte",
            ),
            created: created.clone(),
            class: ErrorClass::PreconditionsChanged,
        })?;
        current_path.push(name);
        let mut next_fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        let created_here = if next_fd < 0 {
            let open_error = std::io::Error::last_os_error();
            if open_error.kind() != std::io::ErrorKind::NotFound {
                return Err(ParentCreationFailure {
                    source: open_error,
                    created,
                    class: ErrorClass::PreconditionsChanged,
                });
            }
            let mkdir_status =
                unsafe { libc::mkdirat(directory.as_raw_fd(), name_c.as_ptr(), 0o755) };
            let created_here = mkdir_status == 0;
            if !created_here {
                let create_error = std::io::Error::last_os_error();
                if create_error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(ParentCreationFailure {
                        source: create_error,
                        created,
                        class: ErrorClass::Io,
                    });
                }
            }
            next_fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name_c.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if next_fd < 0 {
                return Err(ParentCreationFailure {
                    source: std::io::Error::last_os_error(),
                    created,
                    class: ErrorClass::PreconditionsChanged,
                });
            }
            created_here
        } else {
            false
        };
        let next_directory = Arc::new(unsafe { std::fs::File::from_raw_fd(next_fd) });
        if created_here {
            let metadata = next_directory
                .metadata()
                .map_err(|source| ParentCreationFailure {
                    source,
                    created: created.clone(),
                    class: ErrorClass::Io,
                })?;
            created.push(CreatedDirectory {
                path: current_path.clone(),
                parent_directory: Arc::clone(&directory),
                leaf: name.to_os_string(),
                identity: (metadata.dev(), metadata.ino()),
            });
        }
        directory = next_directory;
    }
    let metadata = directory
        .metadata()
        .map_err(|source| ParentCreationFailure {
            source,
            created: created.clone(),
            class: ErrorClass::Io,
        })?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|source| ParentCreationFailure {
            source,
            created: created.clone(),
            class: ErrorClass::PreconditionsChanged,
        })?;
    let canonical_metadata =
        std::fs::metadata(&canonical_parent).map_err(|source| ParentCreationFailure {
            source,
            created: created.clone(),
            class: ErrorClass::PreconditionsChanged,
        })?;
    if (canonical_metadata.dev(), canonical_metadata.ino()) != (metadata.dev(), metadata.ino()) {
        return Err(ParentCreationFailure {
            source: std::io::Error::other("prepared worktree parent changed during execution"),
            created,
            class: ErrorClass::PreconditionsChanged,
        });
    }
    Ok(PreparedParent {
        parent: parent.to_path_buf(),
        created,
        identity: (metadata.dev(), metadata.ino()),
        directory,
        canonical_parent,
    })
}

#[cfg(not(unix))]
fn prepare_parent_directory(
    target: &Path,
    _worktree_root: &Path,
) -> Result<PreparedParent, ParentCreationFailure> {
    let created_paths =
        create_parent_directories_with(target, |directory| std::fs::create_dir(directory))
            .map_err(|error| ParentCreationFailure {
                source: error.source,
                created: error
                    .created
                    .into_iter()
                    .map(|path| CreatedDirectory { path })
                    .collect(),
                class: ErrorClass::Io,
            })?;
    let created = created_paths
        .into_iter()
        .map(|path| CreatedDirectory { path })
        .collect::<Vec<_>>();
    let parent = target.parent().ok_or_else(|| ParentCreationFailure {
        source: std::io::Error::other("planned worktree has no parent"),
        created: created.clone(),
        class: ErrorClass::PreconditionsChanged,
    })?;
    Ok(PreparedParent {
        parent: parent.to_path_buf(),
        created,
        canonical_parent: parent.to_path_buf(),
    })
}

#[cfg(any(test, not(unix)))]
#[derive(Debug)]
struct PathParentCreationFailure {
    source: std::io::Error,
    created: Vec<PathBuf>,
}

#[cfg(any(test, not(unix)))]
fn create_parent_directories_with(
    target: &Path,
    mut create: impl FnMut(&Path) -> std::io::Result<()>,
) -> Result<Vec<PathBuf>, PathParentCreationFailure> {
    let Some(parent) = target.parent() else {
        return Ok(Vec::new());
    };
    let mut missing = Vec::new();
    let mut cursor = parent;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        let Some(next) = cursor.parent() else {
            break;
        };
        cursor = next;
    }
    let mut created = Vec::new();
    for directory in missing.iter().rev() {
        if let Err(source) = create(directory) {
            return Err(PathParentCreationFailure { source, created });
        }
        created.push(directory.clone());
    }
    Ok(created)
}

#[cfg(unix)]
fn cleanup_empty_directories(directories: &[CreatedDirectory]) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    for directory in directories.iter().rev() {
        let leaf = std::ffi::CString::new(directory.leaf.as_os_str().as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "created directory name contains a NUL byte",
            )
        })?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let status = unsafe {
            libc::fstatat(
                directory.parent_directory.as_raw_fd(),
                leaf.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                continue;
            }
            return Err(error);
        }
        let stat = unsafe { stat.assume_init() };
        #[allow(clippy::unnecessary_cast)]
        let live_identity = (stat.st_dev as u64, stat.st_ino as u64);
        if live_identity != directory.identity {
            return Err(std::io::Error::other(format!(
                "created directory identity changed before cleanup: {}",
                directory.path.display()
            )));
        }
        let status = unsafe {
            libc::unlinkat(
                directory.parent_directory.as_raw_fd(),
                leaf.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        };
        if status != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn cleanup_empty_directories(directories: &[CreatedDirectory]) -> std::io::Result<()> {
    for directory in directories.iter().rev() {
        match std::fs::remove_dir(&directory.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

struct FailureCause {
    stage: OperationStage,
    class: ErrorClass,
    message: String,
}

fn rollback_failure(
    repo_path: &Path,
    emitter: &dyn Emitter,
    operation_started: Instant,
    created_parents: &[CreatedDirectory],
    receipt: Option<&git::create::CreateReceipt>,
    failure: FailureCause,
) -> OperationFailure {
    let rollback_started = Instant::now();
    emitter.emit(OperationEvent::StageStarted {
        stage: OperationStage::Rollback,
    });
    let git_rollback = receipt.map_or(Ok(()), |receipt| {
        git::create::rollback_created_worktree(repo_path, receipt)
    });
    let parent_cleanup = cleanup_empty_directories(created_parents);
    match (git_rollback, parent_cleanup) {
        (Ok(()), Ok(())) => {
            finish_stage(emitter, OperationStage::Rollback, rollback_started, true);
            fail(
                emitter,
                operation_started,
                failure.stage,
                MutationState::RolledBack,
                failure.class,
                failure.message,
            )
        }
        (git_result, parent_result) => {
            finish_stage(emitter, OperationStage::Rollback, rollback_started, false);
            let cleanup_error = git_result
                .err()
                .map(|error| error.to_string())
                .or_else(|| parent_result.err().map(|error| error.to_string()))
                .unwrap_or_else(|| "unknown cleanup failure".to_string());
            fail(
                emitter,
                operation_started,
                OperationStage::Rollback,
                MutationState::PartiallyApplied,
                ErrorClass::Cleanup,
                format!("operation failed and cleanup failed: {cleanup_error}"),
            )
        }
    }
}

fn create_hook_context(request: &CreateRequest) -> crate::hooks::HookEnvContext {
    crate::hooks::HookEnvContext {
        worktree_path: request.plan.path.to_string_lossy().into_owned(),
        worktree_name: request.plan.worktree.clone(),
        branch: request.plan.branch.clone(),
        repo_name: request
            .repo_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repository".to_string()),
        repo_path: request.repo_path.to_string_lossy().into_owned(),
        base_branch: request
            .plan
            .base
            .as_deref()
            .or(request.plan.tracking.as_deref())
            .unwrap_or("")
            .to_string(),
    }
}

fn classify_hook_error(error: &anyhow::Error) -> ErrorClass {
    if error
        .chain()
        .any(|cause| cause.is::<crate::hooks::runner::HookTimeoutError>())
    {
        ErrorClass::HookTimeout
    } else {
        ErrorClass::Hook
    }
}

struct OperationHookEmitter<'a> {
    operation_emitter: &'a dyn Emitter,
    hook: crate::hooks::HookEvent,
}

impl crate::hooks::types::HookEmitter for OperationHookEmitter<'_> {
    fn emit(&self, event: crate::hooks::types::HookStreamEvent) {
        if let crate::hooks::types::HookStreamEvent::Output { step, stream, line } = event {
            self.operation_emitter.emit(OperationEvent::Output {
                hook: self.hook,
                step,
                stream,
                line,
            });
        }
    }
}

fn finish_stage(emitter: &dyn Emitter, stage: OperationStage, started: Instant, success: bool) {
    let duration = started.elapsed();
    emitter.emit(OperationEvent::StageFinished {
        stage,
        duration,
        success,
    });
    logging::record(logging::DiagnosticEvent::debug(
        logging::Operation::Create,
        diagnostic_stage(stage),
        duration,
    ));
}

fn finish_operation(emitter: &dyn Emitter, started: Instant, state: MutationState) {
    let duration = started.elapsed();
    emitter.emit(OperationEvent::Finished {
        mutation_state: state,
        duration,
    });
    logging::record(logging::DiagnosticEvent::debug(
        logging::Operation::Create,
        logging::Stage::Complete,
        duration,
    ));
}

fn fail(
    emitter: &dyn Emitter,
    started: Instant,
    stage: OperationStage,
    state: MutationState,
    class: ErrorClass,
    message: String,
) -> OperationFailure {
    let duration = started.elapsed();
    emitter.emit(OperationEvent::Finished {
        mutation_state: state,
        duration,
    });
    logging::record(logging::DiagnosticEvent::error(
        logging::Operation::Create,
        diagnostic_stage(stage),
        duration,
        diagnostic_error(class),
    ));
    OperationFailure {
        stage,
        mutation_state: state,
        class,
        message,
        retained_quarantine: None,
    }
}

fn diagnostic_stage(stage: OperationStage) -> logging::Stage {
    match stage {
        OperationStage::Revalidate => logging::Stage::Validate,
        OperationStage::PreHook | OperationStage::PostHook => logging::Stage::Hook,
        OperationStage::Fetch
        | OperationStage::CreateWorktree
        | OperationStage::Sync
        | OperationStage::RemoveWorktree
        | OperationStage::Prune
        | OperationStage::DeleteBranch
        | OperationStage::Rollback => logging::Stage::Git,
    }
}

fn diagnostic_error(class: ErrorClass) -> logging::DiagnosticError {
    match class {
        ErrorClass::Cancelled | ErrorClass::PreconditionsChanged => {
            logging::DiagnosticError::InvalidInput
        }
        ErrorClass::Git | ErrorClass::Cleanup => logging::DiagnosticError::Git,
        ErrorClass::Hook | ErrorClass::HookTimeout => logging::DiagnosticError::Hook,
        ErrorClass::Io => logging::DiagnosticError::Io,
        ErrorClass::Internal => logging::DiagnosticError::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create_plan::HookPolicy;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn sync_adapter_preserves_stage_and_hook_output_events() {
        use crate::cli::commands::sync::stateless::{SyncEmitter, SyncEvent, SyncStage};
        use crate::hooks::{
            types::{HookStep, OutputStream},
            HookEvent,
        };

        let emitter = RecordingEmitter::default();
        let adapter = SyncEmitterAdapter { emitter: &emitter };
        adapter.emit(SyncEvent::StageStarted(SyncStage::Sync));
        adapter.emit(SyncEvent::HookOutput {
            hook: HookEvent::PreSync,
            step: HookStep::Run,
            stream: OutputStream::Stdout,
            line: "preparing".to_string(),
        });
        adapter.emit(SyncEvent::StageFinished {
            stage: SyncStage::Sync,
            success: true,
            elapsed: Duration::from_millis(25),
        });

        assert_eq!(
            emitter.events(),
            [
                OperationEvent::StageStarted {
                    stage: OperationStage::Sync,
                },
                OperationEvent::Output {
                    hook: HookEvent::PreSync,
                    step: HookStep::Run,
                    stream: OutputStream::Stdout,
                    line: "preparing".to_string(),
                },
                OperationEvent::StageFinished {
                    stage: OperationStage::Sync,
                    duration: Duration::from_millis(25),
                    success: true,
                },
            ]
        );
    }

    struct CreateCollisionAtMutation {
        path: PathBuf,
        revalidations: AtomicUsize,
        events: RecordingEmitter,
    }

    struct RefChangeAfterPreHook {
        repo_path: PathBuf,
        reference: String,
        replacement: git2::Oid,
        events: RecordingEmitter,
    }

    impl Emitter for RefChangeAfterPreHook {
        fn emit(&self, event: OperationEvent) {
            if matches!(
                event,
                OperationEvent::StageFinished {
                    stage: OperationStage::PreHook,
                    success: true,
                    ..
                }
            ) {
                let repo = git2::Repository::open(&self.repo_path).unwrap();
                repo.reference(&self.reference, self.replacement, true, "test race")
                    .unwrap();
            }
            self.events.emit(event);
        }
    }

    struct BranchRaceAtGitBoundary {
        repo_path: PathBuf,
        branch: String,
        revalidations: AtomicUsize,
        events: RecordingEmitter,
    }

    struct ActorWorktreeAtCreateBoundary {
        repo_path: PathBuf,
        target: PathBuf,
        worktree: String,
        fired: AtomicBool,
        events: RecordingEmitter,
    }

    struct CancelAtFinalMutationBoundary {
        cancellation: CancellationToken,
    }

    impl CreateBoundary for CancelAtFinalMutationBoundary {
        fn after_final_verify(&self) {
            assert!(self.cancellation.cancel());
        }
    }

    #[cfg(unix)]
    struct ParentSwapAfterFinalVerify {
        parent: PathBuf,
        held_parent: PathBuf,
        outside: PathBuf,
    }

    #[cfg(unix)]
    impl CreateBoundary for ParentSwapAfterFinalVerify {
        fn after_final_verify(&self) {
            std::fs::rename(&self.parent, &self.held_parent).unwrap();
            std::os::unix::fs::symlink(&self.outside, &self.parent).unwrap();
        }
    }

    impl Emitter for ActorWorktreeAtCreateBoundary {
        fn emit(&self, event: OperationEvent) {
            if matches!(
                event,
                OperationEvent::StageStarted {
                    stage: OperationStage::CreateWorktree
                }
            ) && !self.fired.swap(true, Ordering::SeqCst)
            {
                std::fs::create_dir_all(self.target.parent().unwrap()).unwrap();
                let repo = git2::Repository::open(&self.repo_path).unwrap();
                let head = repo.head().unwrap().peel_to_commit().unwrap();
                let actor = repo.branch("actor/owned", &head, false).unwrap();
                let mut options = git2::WorktreeAddOptions::new();
                options.reference(Some(actor.get()));
                repo.worktree(&self.worktree, &self.target, Some(&options))
                    .unwrap();
            }
            self.events.emit(event);
        }
    }

    impl Emitter for BranchRaceAtGitBoundary {
        fn emit(&self, event: OperationEvent) {
            if matches!(
                event,
                OperationEvent::StageFinished {
                    stage: OperationStage::Revalidate,
                    success: true,
                    ..
                }
            ) && self.revalidations.fetch_add(1, Ordering::SeqCst) == 1
            {
                let repo = git2::Repository::open(&self.repo_path).unwrap();
                let head = repo.head().unwrap().peel_to_commit().unwrap();
                repo.branch(&self.branch, &head, false).unwrap();
            }
            self.events.emit(event);
        }
    }

    #[cfg(unix)]
    struct SymlinkRaceAfterPreHook {
        link: PathBuf,
        outside: PathBuf,
        events: RecordingEmitter,
    }

    #[cfg(unix)]
    struct SymlinkRaceAtCreateBoundary {
        link: PathBuf,
        outside: PathBuf,
        fired: AtomicBool,
        events: RecordingEmitter,
    }

    #[cfg(unix)]
    impl Emitter for SymlinkRaceAtCreateBoundary {
        fn emit(&self, event: OperationEvent) {
            if matches!(
                event,
                OperationEvent::StageStarted {
                    stage: OperationStage::CreateWorktree
                }
            ) && !self.fired.swap(true, Ordering::SeqCst)
            {
                std::fs::create_dir_all(self.link.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(&self.outside, &self.link).unwrap();
            }
            self.events.emit(event);
        }
    }

    #[cfg(unix)]
    impl Emitter for SymlinkRaceAfterPreHook {
        fn emit(&self, event: OperationEvent) {
            if matches!(
                event,
                OperationEvent::StageFinished {
                    stage: OperationStage::PreHook,
                    success: true,
                    ..
                }
            ) {
                std::fs::create_dir_all(self.link.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(&self.outside, &self.link).unwrap();
            }
            self.events.emit(event);
        }
    }

    struct BranchRaceAfterPreHook {
        repo_path: PathBuf,
        branch: String,
        events: RecordingEmitter,
    }

    impl Emitter for BranchRaceAfterPreHook {
        fn emit(&self, event: OperationEvent) {
            if matches!(
                event,
                OperationEvent::StageFinished {
                    stage: OperationStage::PreHook,
                    success: true,
                    ..
                }
            ) {
                let repo = git2::Repository::open(&self.repo_path).unwrap();
                let head = repo.head().unwrap().peel_to_commit().unwrap();
                repo.branch(&self.branch, &head, false).unwrap();
            }
            self.events.emit(event);
        }
    }

    impl Emitter for CreateCollisionAtMutation {
        fn emit(&self, event: OperationEvent) {
            if matches!(
                event,
                OperationEvent::StageFinished {
                    stage: OperationStage::Revalidate,
                    success: true,
                    ..
                }
            ) && self.revalidations.fetch_add(1, Ordering::SeqCst) == 1
            {
                std::fs::create_dir_all(&self.path).unwrap();
                std::fs::write(self.path.join("foreign-file"), "do not remove").unwrap();
            }
            self.events.emit(event);
        }
    }

    fn init_repo(path: &Path) -> git2::Repository {
        let repo = git2::Repository::init(path).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
            .unwrap();
        drop(tree);
        repo
    }

    #[tokio::test]
    async fn changed_ref_precondition_aborts_before_mutation() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        let repo = init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/race", None)
            .unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature/race", &head, false).unwrap();
        let emitter = RecordingEmitter::default();

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root: worktree_root.clone(),
                hooks: None,
            }),
            &emitter,
        )
        .await
        .unwrap_err();

        assert_eq!(error.stage, OperationStage::Revalidate);
        assert_eq!(error.mutation_state, MutationState::NotStarted);
        assert_eq!(error.class, ErrorClass::PreconditionsChanged);
        assert!(!worktree_root.exists());
        assert!(!emitter.events().contains(&OperationEvent::MutationStarted));
    }

    #[tokio::test]
    async fn cancellation_is_checked_immediately_before_the_pre_hook_boundary() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/cancelled", None)
            .unwrap();
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let emitter = RecordingEmitter::default();

        let error = execute_cancellable(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root: worktree_root.clone(),
                hooks: Some(HooksConfig {
                    pre_create: Some(crate::config::HookDef {
                        shell: Some("exit 99".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }),
            &emitter,
            &cancellation,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::Cancelled);
        assert_eq!(error.mutation_state, MutationState::NotStarted);
        assert!(!worktree_root.exists());
        assert!(!emitter.events().contains(&OperationEvent::MutationStarted));
        assert!(!emitter.events().iter().any(|event| matches!(
            event,
            OperationEvent::StageStarted {
                stage: OperationStage::PreHook
            }
        )));
    }

    #[tokio::test]
    async fn cancellation_wins_atomically_at_the_final_git_mutation_boundary() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        let repo = init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/final-cancel", None)
            .unwrap();
        let cancellation = CancellationToken::default();
        let boundary = CancelAtFinalMutationBoundary {
            cancellation: cancellation.clone(),
        };
        let emitter = RecordingEmitter::default();

        let error = execute_create_with_boundary(
            CreateRequest {
                plan: plan.clone(),
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: None,
            },
            &emitter,
            &cancellation,
            &boundary,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::Cancelled);
        assert_eq!(error.mutation_state, MutationState::RolledBack);
        assert!(!emitter.events().contains(&OperationEvent::MutationStarted));
        assert!(repo
            .find_branch(&plan.branch, git2::BranchType::Local)
            .is_err());
        assert!(repo.find_worktree(&plan.worktree).is_err());
        assert!(!plan.path.exists());
    }

    #[tokio::test]
    async fn collision_without_a_receipt_is_preserved_without_git_rollback() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/collision", None)
            .unwrap();
        let collision_path = plan.path.clone();
        let emitter = CreateCollisionAtMutation {
            path: collision_path.clone(),
            revalidations: AtomicUsize::new(0),
            events: RecordingEmitter::default(),
        };

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: None,
            }),
            &emitter,
        )
        .await
        .unwrap_err();

        assert_eq!(error.stage, OperationStage::CreateWorktree);
        assert_eq!(error.mutation_state, MutationState::RolledBack);
        assert_eq!(error.class, ErrorClass::PreconditionsChanged);
        assert_eq!(
            std::fs::read_to_string(collision_path.join("foreign-file")).unwrap(),
            "do not remove"
        );
    }

    #[tokio::test]
    async fn post_create_cleanup_failure_is_structured_and_preserves_foreign_files() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/cleanup-failure", None)
            .unwrap();
        let worktree_path = plan.path.clone();
        let foreign_path = worktree_path.parent().unwrap().join("foreign-file");

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: Some(HooksConfig {
                    post_create: Some(crate::config::HookDef {
                        shell: Some("touch ../foreign-file; exit 1".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }),
            &RecordingEmitter::default(),
        )
        .await
        .unwrap_err();

        assert_eq!(error.stage, OperationStage::Rollback);
        assert_eq!(error.mutation_state, MutationState::PartiallyApplied);
        assert_eq!(error.class, ErrorClass::Cleanup);
        assert!(
            !worktree_path.exists(),
            "owned worktree must be rolled back"
        );
        assert!(foreign_path.exists(), "foreign files must never be removed");
    }

    #[test]
    fn parent_creation_failure_removes_only_directories_created_before_the_error() {
        let outside = tempfile::tempdir().unwrap();
        let root = outside.path().join("root");
        let target = root.join("repository/worktree");
        let mut calls = 0;

        let error = create_parent_directories_with(&target, |directory| {
            calls += 1;
            if calls == 2 {
                return Err(std::io::Error::other("injected parent creation failure"));
            }
            std::fs::create_dir(directory)
        })
        .unwrap_err();

        assert_eq!(error.source.kind(), std::io::ErrorKind::Other);
        assert_eq!(error.created.as_slice(), std::slice::from_ref(&root));
        for directory in error.created.iter().rev() {
            std::fs::remove_dir(directory).unwrap();
        }
        assert!(!root.exists(), "partially-created parents must be cleaned");
        assert!(outside.path().is_dir(), "pre-existing parent must remain");
    }

    #[tokio::test]
    async fn base_oid_changed_after_pre_hook_aborts_before_worktree_creation() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        let repo = init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/base-race", None)
            .unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree = repo
            .find_tree(repo.index().unwrap().write_tree().unwrap())
            .unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let replacement = repo
            .commit(
                None,
                &signature,
                &signature,
                "replacement",
                &tree,
                &[&parent],
            )
            .unwrap();
        let emitter = RefChangeAfterPreHook {
            repo_path: repo_dir.path().to_path_buf(),
            reference: "refs/heads/main".into(),
            replacement,
            events: RecordingEmitter::default(),
        };

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root: worktree_root.clone(),
                hooks: Some(HooksConfig {
                    pre_create: Some(crate::config::HookDef {
                        run: Some(vec!["true".into()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }),
            &emitter,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::PreconditionsChanged);
        assert!(!worktree_root.exists());
        assert!(repo
            .find_branch("feature/base-race", git2::BranchType::Local)
            .is_err());
    }

    #[tokio::test]
    async fn branch_created_after_pre_hook_is_never_deleted_as_rollback_ownership() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        let repo = init_repo(repo_dir.path());
        let branch = "feature/branch-race";
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan(branch, None)
            .unwrap();
        let emitter = BranchRaceAfterPreHook {
            repo_path: repo_dir.path().to_path_buf(),
            branch: branch.into(),
            events: RecordingEmitter::default(),
        };

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: Some(HooksConfig {
                    pre_create: Some(crate::config::HookDef {
                        run: Some(vec!["true".into()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }),
            &emitter,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::PreconditionsChanged);
        assert!(repo.find_branch(branch, git2::BranchType::Local).is_ok());
    }

    #[tokio::test]
    async fn branch_racing_before_parent_preparation_is_not_owned_or_deleted() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        let repo = init_repo(repo_dir.path());
        let branch = "feature/git-boundary-race";
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan(branch, None)
            .unwrap();
        let emitter = BranchRaceAtGitBoundary {
            repo_path: repo_dir.path().to_path_buf(),
            branch: branch.into(),
            revalidations: AtomicUsize::new(0),
            events: RecordingEmitter::default(),
        };

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: None,
            }),
            &emitter,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::PreconditionsChanged);
        assert!(repo.find_branch(branch, git2::BranchType::Local).is_ok());
    }

    #[tokio::test]
    async fn remote_oid_changed_after_pre_hook_aborts_before_tracking_branch_creation() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        let repo = init_repo(repo_dir.path());
        repo.remote("origin", "unused-test-remote").unwrap();
        let original = repo.head().unwrap().target().unwrap();
        repo.reference("refs/remotes/origin/topic", original, false, "test remote")
            .unwrap();
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("origin/topic", None)
            .unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree = repo
            .find_tree(repo.index().unwrap().write_tree().unwrap())
            .unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let replacement = repo
            .commit(
                None,
                &signature,
                &signature,
                "replacement",
                &tree,
                &[&parent],
            )
            .unwrap();
        let emitter = RefChangeAfterPreHook {
            repo_path: repo_dir.path().to_path_buf(),
            reference: "refs/remotes/origin/topic".into(),
            replacement,
            events: RecordingEmitter::default(),
        };

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: Some(HooksConfig {
                    pre_create: Some(crate::config::HookDef {
                        run: Some(vec!["true".into()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }),
            &emitter,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::PreconditionsChanged);
        assert!(repo.find_branch("topic", git2::BranchType::Local).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_inserted_after_pre_hook_is_rejected_before_path_mutation() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let escape = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/symlink-race", None)
            .unwrap();
        let repository_name = repo_dir.path().file_name().unwrap();
        let emitter = SymlinkRaceAfterPreHook {
            link: worktree_root.join(repository_name),
            outside: escape.path().to_path_buf(),
            events: RecordingEmitter::default(),
        };

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: Some(HooksConfig {
                    pre_create: Some(crate::config::HookDef {
                        run: Some(vec!["true".into()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }),
            &emitter,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::PreconditionsChanged);
        assert!(!escape.path().join("feature-symlink-race").exists());
    }

    #[tokio::test]
    async fn create_failure_without_receipt_never_removes_an_actor_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        let repo = init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/actor-race", None)
            .unwrap();
        let emitter = ActorWorktreeAtCreateBoundary {
            repo_path: repo_dir.path().to_path_buf(),
            target: plan.path.clone(),
            worktree: plan.worktree.clone(),
            fired: AtomicBool::new(false),
            events: RecordingEmitter::default(),
        };

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan: plan.clone(),
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: None,
            }),
            &emitter,
        )
        .await
        .unwrap_err();

        assert_eq!(error.mutation_state, MutationState::RolledBack);
        assert!(plan.path.is_dir(), "actor worktree was removed");
        let actor = repo
            .find_branch("actor/owned", git2::BranchType::Local)
            .unwrap();
        assert!(actor.is_head() || actor.get().target().is_some());
        assert!(repo.find_worktree(&plan.worktree).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_inserted_after_final_validation_cannot_escape_parent_preparation() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let escape = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/late-symlink-race", None)
            .unwrap();
        let repository_name = repo_dir.path().file_name().unwrap();
        let emitter = SymlinkRaceAtCreateBoundary {
            link: worktree_root.join(repository_name),
            outside: escape.path().to_path_buf(),
            fired: AtomicBool::new(false),
            events: RecordingEmitter::default(),
        };

        let error = execute(
            OperationRequest::Create(CreateRequest {
                plan,
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: None,
            }),
            &emitter,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::PreconditionsChanged);
        assert!(!escape.path().join("feature-late-symlink-race").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parent_swap_after_final_verify_cannot_redirect_git_mutation() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let escape = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("worktrees");
        let repository_parent = worktree_root.join(repo_dir.path().file_name().unwrap());
        std::fs::create_dir_all(&repository_parent).unwrap();
        std::fs::write(escape.path().join("foreign-file"), "preserve").unwrap();
        let repo = init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/verify-swap", None)
            .unwrap();
        let held_parent = outside.path().join("held-repository-parent");
        let boundary = ParentSwapAfterFinalVerify {
            parent: repository_parent.clone(),
            held_parent: held_parent.clone(),
            outside: escape.path().to_path_buf(),
        };
        let process_cwd = std::env::current_dir().unwrap();

        let error = execute_create_with_boundary(
            CreateRequest {
                plan: plan.clone(),
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root,
                hooks: None,
            },
            &RecordingEmitter::default(),
            &NeverCancelled,
            &boundary,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::PreconditionsChanged);
        assert_eq!(error.mutation_state, MutationState::RolledBack);
        assert_eq!(std::env::current_dir().unwrap(), process_cwd);
        assert!(std::fs::symlink_metadata(&repository_parent)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(escape.path().join("foreign-file")).unwrap(),
            "preserve"
        );
        assert!(!escape.path().join(&plan.worktree).exists());
        assert!(!held_parent.join(&plan.worktree).exists());
        assert!(repo
            .find_branch(&plan.branch, git2::BranchType::Local)
            .is_err());
        assert!(repo.find_worktree(&plan.worktree).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn swapped_created_ancestor_cleanup_never_removes_a_foreign_empty_directory() {
        let repo_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let escape = tempfile::tempdir().unwrap();
        let worktree_root = outside.path().join("missing/worktrees");
        let repository_name = repo_dir.path().file_name().unwrap();
        let foreign_empty = escape.path().join(repository_name);
        std::fs::create_dir(&foreign_empty).unwrap();
        std::fs::write(escape.path().join("foreign-file"), "preserve").unwrap();
        let repo = init_repo(repo_dir.path());
        let plan = CreatePlanner::discover(repo_dir.path(), &worktree_root, None, HookPolicy::Run)
            .unwrap()
            .plan("feature/ancestor-swap", None)
            .unwrap();
        let held_root = outside.path().join("held-worktree-root");
        let boundary = ParentSwapAfterFinalVerify {
            parent: worktree_root.clone(),
            held_parent: held_root.clone(),
            outside: escape.path().to_path_buf(),
        };

        let error = execute_create_with_boundary(
            CreateRequest {
                plan: plan.clone(),
                repo_path: repo_dir.path().to_path_buf(),
                worktree_root: worktree_root.clone(),
                hooks: None,
            },
            &RecordingEmitter::default(),
            &NeverCancelled,
            &boundary,
        )
        .await
        .unwrap_err();

        assert_eq!(error.class, ErrorClass::Cleanup);
        assert_eq!(error.mutation_state, MutationState::PartiallyApplied);
        assert!(
            foreign_empty.is_dir(),
            "foreign empty directory was removed"
        );
        assert_eq!(
            std::fs::read_to_string(escape.path().join("foreign-file")).unwrap(),
            "preserve"
        );
        assert!(!held_root
            .join(repository_name)
            .join(&plan.worktree)
            .exists());
        assert!(repo
            .find_branch(&plan.branch, git2::BranchType::Local)
            .is_err());
        assert!(repo.find_worktree(&plan.worktree).is_err());
    }

    #[test]
    fn remove_emitter_maps_typed_stages_and_partial_state() {
        use crate::cli::commands::remove::stateless::{
            RemovalEvent, RemovalEventSink, RemovalMutationState, RemovalStage,
        };

        let emitter = RecordingEmitter::default();
        let adapter = RemoveEmitterAdapter { emitter: &emitter };
        adapter.emit(RemovalEvent::StageStarted {
            stage: RemovalStage::Prune,
        });
        adapter.emit(RemovalEvent::Finished {
            mutation_state: RemovalMutationState::PartiallyApplied,
            duration: Duration::from_millis(1),
        });

        assert_eq!(
            emitter.events(),
            vec![
                OperationEvent::StageStarted {
                    stage: OperationStage::Prune,
                },
                OperationEvent::Finished {
                    mutation_state: MutationState::PartiallyApplied,
                    duration: Duration::from_millis(1),
                },
            ]
        );
    }

    #[test]
    fn operation_failure_serializes_retained_quarantine() {
        let failure = OperationFailure {
            stage: OperationStage::RemoveWorktree,
            mutation_state: MutationState::PartiallyApplied,
            class: ErrorClass::Io,
            message: "recovery required".to_string(),
            retained_quarantine: Some(PathBuf::from("/tmp/retained-worktree")),
        };

        let value = serde_json::to_value(failure).unwrap();
        assert_eq!(value["retained_quarantine"], "/tmp/retained-worktree");
        assert_eq!(value["mutation_state"], "partially_applied");
    }
}
