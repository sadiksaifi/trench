use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::ser::{Serialize, SerializeStruct, Serializer};

use crate::config::HooksConfig;
use crate::create_plan::{CreateAction, CreatePlan, CreatePlanner, HookPolicy};
use crate::{git, logging};

#[derive(Debug, Clone)]
pub enum OperationRequest {
    Create(CreateRequest),
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStage {
    Revalidate,
    PreHook,
    CreateWorktree,
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
}

#[derive(Debug, Default)]
pub struct NeverCancelled;

impl CancellationCheck for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

impl CancellationCheck for CancellationToken {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationOutcome {
    Create(CreateOutcome),
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
    }
}

async fn execute_create(
    request: CreateRequest,
    emitter: &dyn Emitter,
    cancellation: &dyn CancellationCheck,
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
    emitter.emit(OperationEvent::MutationStarted);

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
                return Err(fail(
                    emitter,
                    operation_started,
                    OperationStage::PreHook,
                    MutationState::RolledBack,
                    ErrorClass::Hook,
                    error.to_string(),
                ));
            }
            finish_stage(emitter, OperationStage::PreHook, stage_started, true);
        }
    }

    let stage_started = Instant::now();
    emitter.emit(OperationEvent::StageStarted {
        stage: OperationStage::CreateWorktree,
    });
    let created_parents = create_parent_directories(&request.plan.path).map_err(|error| {
        fail(
            emitter,
            operation_started,
            OperationStage::CreateWorktree,
            MutationState::NotStarted,
            ErrorClass::Io,
            format!("failed to create worktree parent: {error}"),
        )
    })?;

    let create_result = match &request.plan.action {
        CreateAction::NewBranch(base) => git::create::add_new_branch(
            &request.repo_path,
            &request.plan.worktree,
            &request.plan.branch,
            base,
            &request.plan.path,
        ),
        CreateAction::ExistingLocal => git::create::add_existing_local(
            &request.repo_path,
            &request.plan.worktree,
            &request.plan.branch,
            &request.plan.path,
        ),
        CreateAction::TrackRemote(upstream) => git::create::add_tracking_branch(
            &request.repo_path,
            &request.plan.worktree,
            &request.plan.branch,
            upstream,
            &request.plan.path,
        ),
        CreateAction::Navigate(_) => unreachable!("navigate returned before mutation"),
    };

    if let Err(error) = create_result {
        finish_stage(
            emitter,
            OperationStage::CreateWorktree,
            stage_started,
            false,
        );
        return Err(rollback_failure(
            &request,
            emitter,
            operation_started,
            &created_parents,
            OperationStage::CreateWorktree,
            ErrorClass::Git,
            error.to_string(),
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
                return Err(rollback_failure(
                    &request,
                    emitter,
                    operation_started,
                    &created_parents,
                    OperationStage::PostHook,
                    ErrorClass::Hook,
                    error.to_string(),
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

fn create_parent_directories(target: &Path) -> std::io::Result<Vec<PathBuf>> {
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
    for directory in missing.iter().rev() {
        std::fs::create_dir(directory)?;
    }
    Ok(missing)
}

fn cleanup_empty_directories(directories: &[PathBuf]) {
    for directory in directories {
        let _ = std::fs::remove_dir(directory);
    }
}

fn created_branch(plan: &CreatePlan) -> Option<&str> {
    match plan.action {
        CreateAction::NewBranch(_) | CreateAction::TrackRemote(_) => Some(&plan.branch),
        CreateAction::ExistingLocal | CreateAction::Navigate(_) => None,
    }
}

fn rollback_failure(
    request: &CreateRequest,
    emitter: &dyn Emitter,
    operation_started: Instant,
    created_parents: &[PathBuf],
    failed_stage: OperationStage,
    failed_class: ErrorClass,
    failure_message: String,
) -> OperationFailure {
    let rollback_started = Instant::now();
    emitter.emit(OperationEvent::StageStarted {
        stage: OperationStage::Rollback,
    });
    let rollback = git::create::rollback_created_worktree(
        &request.repo_path,
        &request.plan.path,
        created_branch(&request.plan),
    );
    cleanup_empty_directories(created_parents);
    match rollback {
        Ok(()) => {
            finish_stage(emitter, OperationStage::Rollback, rollback_started, true);
            fail(
                emitter,
                operation_started,
                failed_stage,
                MutationState::RolledBack,
                failed_class,
                failure_message,
            )
        }
        Err(cleanup_error) => {
            finish_stage(emitter, OperationStage::Rollback, rollback_started, false);
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
    }
}

fn diagnostic_stage(stage: OperationStage) -> logging::Stage {
    match stage {
        OperationStage::Revalidate => logging::Stage::Validate,
        OperationStage::PreHook | OperationStage::PostHook => logging::Stage::Hook,
        OperationStage::CreateWorktree | OperationStage::Rollback => logging::Stage::Git,
    }
}

fn diagnostic_error(class: ErrorClass) -> logging::DiagnosticError {
    match class {
        ErrorClass::Cancelled | ErrorClass::PreconditionsChanged => {
            logging::DiagnosticError::InvalidInput
        }
        ErrorClass::Git | ErrorClass::Cleanup => logging::DiagnosticError::Git,
        ErrorClass::Hook => logging::DiagnosticError::Hook,
        ErrorClass::Io => logging::DiagnosticError::Io,
        ErrorClass::Internal => logging::DiagnosticError::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create_plan::HookPolicy;

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
}
