use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::HooksConfig;
use crate::git;
use crate::ref_catalog::{DefaultBaseError, RefCatalog, RefCatalogError};
use crate::worktree_catalog::{CatalogError, WorktreeCatalog};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStrategy {
    Rebase,
    Merge,
}

impl std::fmt::Display for SyncStrategy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Rebase => "rebase",
            Self::Merge => "merge",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPolicy {
    Run,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationState {
    NotStarted,
    RolledBack,
    Applied,
    PartiallyApplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStage {
    Fetch,
    Validate,
    PreHook,
    Sync,
    Rollback,
    PostHook,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncPlan {
    pub target: String,
    pub branch: String,
    pub path: PathBuf,
    pub base: String,
    pub strategy: SyncStrategy,
    pub hook_policy: HookPolicy,
    pub before: AheadBehind,
    #[serde(skip)]
    repo_path: PathBuf,
    #[serde(skip)]
    head_oid: git2::Oid,
    #[serde(skip)]
    base_oid: git2::Oid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncPreview {
    pub dry_run: bool,
    #[serde(flatten)]
    pub plan: SyncPlan,
}

impl std::fmt::Display for SyncPreview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "Dry run — no changes will be made")?;
        writeln!(formatter, "  Worktree: {}", self.plan.target)?;
        writeln!(formatter, "  Branch:   {}", self.plan.branch)?;
        writeln!(formatter, "  Path:     {}", self.plan.path.display())?;
        writeln!(formatter, "  Base:     {}", self.plan.base)?;
        writeln!(formatter, "  Strategy: {}", self.plan.strategy)?;
        write!(
            formatter,
            "  Hooks:    {}",
            match self.plan.hook_policy {
                HookPolicy::Run => "enabled",
                HookPolicy::Skip => "skipped",
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncOutcome {
    pub target: String,
    pub branch: String,
    pub path: PathBuf,
    pub base: String,
    pub strategy: SyncStrategy,
    pub before: AheadBehind,
    pub after: AheadBehind,
    pub mutation_state: MutationState,
    pub elapsed: Duration,
}

impl std::fmt::Display for SyncOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            formatter,
            "Synced '{}' via {} onto {}",
            self.target, self.strategy, self.base
        )?;
        writeln!(
            formatter,
            "  before: ahead={}, behind={}",
            self.before.ahead, self.before.behind
        )?;
        write!(
            formatter,
            "  after:  ahead={}, behind={}",
            self.after.ahead, self.after.behind
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEvent {
    StageStarted(SyncStage),
    HookOutput {
        hook: crate::hooks::HookEvent,
        step: crate::hooks::types::HookStep,
        stream: crate::hooks::types::OutputStream,
        line: String,
    },
    StageFinished {
        stage: SyncStage,
        success: bool,
        elapsed: Duration,
    },
    Warning {
        stage: SyncStage,
        message: String,
    },
}

pub trait SyncEmitter: Send + Sync {
    fn emit(&self, event: SyncEvent);
}

#[derive(Debug)]
pub struct NoopSyncEmitter;

impl SyncEmitter for NoopSyncEmitter {
    fn emit(&self, _event: SyncEvent) {}
}

#[derive(Debug, Clone, Default)]
pub struct RecordingSyncEmitter(Arc<Mutex<Vec<SyncEvent>>>);

impl RecordingSyncEmitter {
    pub fn events(&self) -> Vec<SyncEvent> {
        self.0
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl SyncEmitter for RecordingSyncEmitter {
    fn emit(&self, event: SyncEvent) {
        if let Ok(mut events) = self.0.lock() {
            events.push(event);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncErrorClass {
    InvalidTarget,
    InvalidBase,
    Dirty,
    Detached,
    OperationInProgress,
    PreconditionsChanged,
    Conflict,
    Git,
    Hook,
    HookTimeout,
    Rollback,
}

#[derive(Debug, thiserror::Error, Serialize)]
#[error("{message}")]
pub struct SyncFailure {
    pub stage: SyncStage,
    pub mutation_state: MutationState,
    pub class: SyncErrorClass,
    pub message: String,
    pub elapsed: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncPlanError {
    #[error(transparent)]
    Discovery(#[from] git::GitError),
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Refs(#[from] RefCatalogError),
    #[error(transparent)]
    DefaultBase(#[from] DefaultBaseError),
    #[error("explicit base not found: {base}")]
    ExplicitBaseNotFound { base: String },
    #[error("worktree '{target}' is detached and cannot be synced")]
    Detached { target: String },
    #[error("worktree '{target}' has uncommitted changes; commit or stash them before syncing")]
    Dirty { target: String },
    #[error("worktree '{target}' already has a Git operation in progress")]
    OperationInProgress { target: String },
    #[error(transparent)]
    Git(#[from] git2::Error),
}

impl SyncPlanError {
    pub fn class(&self) -> SyncErrorClass {
        match self {
            Self::Catalog(CatalogError::NotFound { .. } | CatalogError::Ambiguous { .. }) => {
                SyncErrorClass::InvalidTarget
            }
            Self::DefaultBase(_) | Self::ExplicitBaseNotFound { .. } => SyncErrorClass::InvalidBase,
            Self::Detached { .. } => SyncErrorClass::Detached,
            Self::Dirty { .. } => SyncErrorClass::Dirty,
            Self::OperationInProgress { .. } => SyncErrorClass::OperationInProgress,
            Self::Discovery(_)
            | Self::Catalog(CatalogError::Git(_))
            | Self::Refs(_)
            | Self::Git(_) => SyncErrorClass::Git,
        }
    }

    pub fn into_failure(self, elapsed: Duration) -> SyncFailure {
        SyncFailure {
            stage: SyncStage::Validate,
            mutation_state: MutationState::NotStarted,
            class: self.class(),
            message: self.to_string(),
            elapsed,
        }
    }
}

pub struct SyncPlanner {
    cwd: PathBuf,
    configured_base: Option<String>,
}

impl SyncPlanner {
    pub fn discover(cwd: &Path, configured_base: Option<&str>) -> Result<Self, SyncPlanError> {
        // Discovery is intentionally local-only. It never fetches.
        WorktreeCatalog::discover(cwd)?;
        RefCatalog::discover(cwd)?;
        Ok(Self {
            cwd: cwd.to_path_buf(),
            configured_base: configured_base.map(ToOwned::to_owned),
        })
    }

    pub fn plan(
        &self,
        selector: &str,
        explicit_base: Option<&str>,
        strategy: SyncStrategy,
        hook_policy: HookPolicy,
    ) -> Result<SyncPlan, SyncPlanError> {
        let catalog = WorktreeCatalog::discover(&self.cwd)?;
        let target = catalog.resolve(selector)?;
        if target.detached || target.branch.is_none() {
            return Err(SyncPlanError::Detached {
                target: target.worktree.clone(),
            });
        }
        let repo = git2::Repository::open(&target.path)?;
        if repo.state() != git2::RepositoryState::Clean {
            return Err(SyncPlanError::OperationInProgress {
                target: target.worktree.clone(),
            });
        }
        if repo
            .statuses(Some(
                git2::StatusOptions::new()
                    .include_untracked(true)
                    .recurse_untracked_dirs(true),
            ))?
            .iter()
            .next()
            .is_some()
        {
            return Err(SyncPlanError::Dirty {
                target: target.worktree.clone(),
            });
        }
        let refs = RefCatalog::discover(&self.cwd)?;
        let base = match explicit_base {
            Some(base) => {
                refs.resolve(base)
                    .ok_or_else(|| SyncPlanError::ExplicitBaseNotFound {
                        base: base.to_string(),
                    })?
            }
            None => refs.default_base(self.configured_base.as_deref())?,
        };
        let base_object = repo.revparse_single(&base)?;
        let base_oid = base_object.peel_to_commit()?.id();
        let branch = target.branch.clone().unwrap();
        let head_oid = repo.head()?.peel_to_commit()?.id();
        let (ahead, behind) = repo.graph_ahead_behind(head_oid, base_oid)?;
        let repo_path = git::discover_repo(&target.path)?.path;
        Ok(SyncPlan {
            target: target.worktree.clone(),
            branch,
            path: target.path.clone(),
            base,
            strategy,
            hook_policy,
            before: AheadBehind { ahead, behind },
            repo_path,
            head_oid,
            base_oid,
        })
    }
}

/// Build the final plan for a real CLI invocation. This is the only sync
/// entry point that may access the network, and it attempts `origin` once at
/// most. Call [`SyncPlanner::plan`] directly for dry-run and TUI snapshots.
pub fn plan_after_best_effort_origin_fetch(
    cwd: &Path,
    configured_base: Option<&str>,
    selector: &str,
    explicit_base: Option<&str>,
    strategy: SyncStrategy,
    hook_policy: HookPolicy,
    emitter: &dyn SyncEmitter,
) -> Result<SyncPlan, SyncPlanError> {
    let refs = RefCatalog::discover(cwd)?;
    if refs.has_origin {
        let started = Instant::now();
        emitter.emit(SyncEvent::StageStarted(SyncStage::Fetch));
        let repo_path = git::discover_repo(cwd)?.path;
        let result = RefCatalog::fetch_origin(&repo_path);
        emitter.emit(SyncEvent::StageFinished {
            stage: SyncStage::Fetch,
            success: result.is_ok(),
            elapsed: started.elapsed(),
        });
        if result.is_err() {
            emitter.emit(SyncEvent::Warning {
                stage: SyncStage::Fetch,
                message: "origin fetch failed; using local refs".to_string(),
            });
        }
    }
    SyncPlanner::discover(cwd, configured_base)?.plan(
        selector,
        explicit_base,
        strategy,
        hook_policy,
    )
}

pub fn preview(plan: SyncPlan) -> SyncPreview {
    SyncPreview {
        dry_run: true,
        plan,
    }
}

pub async fn execute(
    plan: SyncPlan,
    hooks: Option<&HooksConfig>,
    emitter: &dyn SyncEmitter,
) -> Result<SyncOutcome, SyncFailure> {
    let started = Instant::now();
    stage(emitter, SyncStage::Validate, || revalidate(&plan)).map_err(|message| {
        failure(
            started,
            SyncStage::Validate,
            MutationState::NotStarted,
            SyncErrorClass::PreconditionsChanged,
            message,
        )
    })?;

    let hook_context = hook_context(&plan);
    if plan.hook_policy == HookPolicy::Run {
        if let Some(hook) = hooks.and_then(|hooks| hooks.pre_sync.as_ref()) {
            run_hook(
                crate::hooks::HookEvent::PreSync,
                hook,
                &hook_context,
                &plan,
                emitter,
                SyncStage::PreHook,
            )
            .await
            .map_err(|(class, message)| {
                failure(
                    started,
                    SyncStage::PreHook,
                    MutationState::NotStarted,
                    class,
                    message,
                )
            })?;
        }
    }

    // Hooks are allowed to invoke arbitrary Git commands. Freeze the same
    // plan again at the last boundary before mutation so a hook cannot move
    // the target or base ref underneath the displayed operation.
    stage(emitter, SyncStage::Validate, || revalidate(&plan)).map_err(|message| {
        failure(
            started,
            SyncStage::Validate,
            MutationState::NotStarted,
            SyncErrorClass::PreconditionsChanged,
            message,
        )
    })?;

    let transaction = git::sync::TransactionPlan {
        worktree_path: plan.path.clone(),
        branch_ref: format!("refs/heads/{}", plan.branch),
        expected_head: plan.head_oid,
        base_oid: plan.base_oid,
        strategy: match plan.strategy {
            SyncStrategy::Rebase => git::sync::Strategy::Rebase,
            SyncStrategy::Merge => git::sync::Strategy::Merge,
        },
    };
    let sync_started = Instant::now();
    emitter.emit(SyncEvent::StageStarted(SyncStage::Sync));
    let new_head = match git::sync::execute(&transaction) {
        Ok(head) => {
            emitter.emit(SyncEvent::StageFinished {
                stage: SyncStage::Sync,
                success: true,
                elapsed: sync_started.elapsed(),
            });
            head
        }
        Err(error) => {
            emitter.emit(SyncEvent::StageFinished {
                stage: SyncStage::Sync,
                success: false,
                elapsed: sync_started.elapsed(),
            });
            let (class, mutation_state) = match error {
                git::sync::SyncGitError::Conflict => {
                    (SyncErrorClass::Conflict, MutationState::NotStarted)
                }
                git::sync::SyncGitError::Rollback(_) => {
                    (SyncErrorClass::Rollback, MutationState::PartiallyApplied)
                }
                git::sync::SyncGitError::PreconditionsChanged => (
                    SyncErrorClass::PreconditionsChanged,
                    MutationState::NotStarted,
                ),
                git::sync::SyncGitError::Git(_) => (SyncErrorClass::Git, MutationState::NotStarted),
            };
            return Err(failure(
                started,
                SyncStage::Sync,
                mutation_state,
                class,
                error.to_string(),
            ));
        }
    };
    let repo = git2::Repository::open(&plan.path).map_err(|error| {
        failure(
            started,
            SyncStage::Sync,
            MutationState::Applied,
            SyncErrorClass::Git,
            error.to_string(),
        )
    })?;
    let (ahead, behind) = repo
        .graph_ahead_behind(new_head, plan.base_oid)
        .map_err(|error| {
            failure(
                started,
                SyncStage::Sync,
                MutationState::Applied,
                SyncErrorClass::Git,
                error.to_string(),
            )
        })?;
    if plan.hook_policy == HookPolicy::Run {
        if let Some(hook) = hooks.and_then(|hooks| hooks.post_sync.as_ref()) {
            run_hook(
                crate::hooks::HookEvent::PostSync,
                hook,
                &hook_context,
                &plan,
                emitter,
                SyncStage::PostHook,
            )
            .await
            .map_err(|(class, message)| {
                failure(
                    started,
                    SyncStage::PostHook,
                    MutationState::Applied,
                    class,
                    message,
                )
            })?;
        }
    }
    Ok(SyncOutcome {
        target: plan.target,
        branch: plan.branch,
        path: plan.path,
        base: plan.base,
        strategy: plan.strategy,
        before: plan.before,
        after: AheadBehind { ahead, behind },
        mutation_state: MutationState::Applied,
        elapsed: started.elapsed(),
    })
}

fn revalidate(plan: &SyncPlan) -> Result<(), String> {
    let planner =
        SyncPlanner::discover(&plan.repo_path, None).map_err(|error| error.to_string())?;
    let live = planner
        .plan(
            &plan.path.to_string_lossy(),
            Some(&plan.base),
            plan.strategy,
            plan.hook_policy,
        )
        .map_err(|error| error.to_string())?;
    if live == *plan {
        Ok(())
    } else {
        Err("sync plan no longer matches live Git state".to_string())
    }
}

fn stage<T>(
    emitter: &dyn SyncEmitter,
    stage: SyncStage,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let started = Instant::now();
    emitter.emit(SyncEvent::StageStarted(stage));
    let result = action();
    emitter.emit(SyncEvent::StageFinished {
        stage,
        success: result.is_ok(),
        elapsed: started.elapsed(),
    });
    result
}

fn failure(
    started: Instant,
    stage: SyncStage,
    mutation_state: MutationState,
    class: SyncErrorClass,
    message: String,
) -> SyncFailure {
    SyncFailure {
        stage,
        mutation_state,
        class,
        message,
        elapsed: started.elapsed(),
    }
}

fn hook_context(plan: &SyncPlan) -> crate::hooks::HookEnvContext {
    crate::hooks::HookEnvContext {
        worktree_path: plan.path.to_string_lossy().into_owned(),
        worktree_name: plan.target.clone(),
        branch: plan.branch.clone(),
        repo_name: plan
            .repo_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repository".to_string()),
        repo_path: plan.repo_path.to_string_lossy().into_owned(),
        base_branch: plan.base.clone(),
    }
}

async fn run_hook(
    event: crate::hooks::HookEvent,
    hook: &crate::config::HookDef,
    context: &crate::hooks::HookEnvContext,
    plan: &SyncPlan,
    emitter: &dyn SyncEmitter,
    stage_name: SyncStage,
) -> Result<(), (SyncErrorClass, String)> {
    let started = Instant::now();
    emitter.emit(SyncEvent::StageStarted(stage_name));
    let adapter = HookEmitterAdapter {
        emitter,
        hook: event,
    };
    let result = crate::hooks::runner::execute_hook(
        &event,
        hook,
        context,
        &plan.repo_path,
        &plan.path,
        &adapter,
    )
    .await;
    emitter.emit(SyncEvent::StageFinished {
        stage: stage_name,
        success: result.is_ok(),
        elapsed: started.elapsed(),
    });
    result.map(|_| ()).map_err(|error| {
        let class = if error.chain().any(|cause| {
            cause
                .downcast_ref::<crate::hooks::runner::HookTimeoutError>()
                .is_some()
        }) {
            SyncErrorClass::HookTimeout
        } else {
            SyncErrorClass::Hook
        };
        (class, error.to_string())
    })
}

struct HookEmitterAdapter<'a> {
    emitter: &'a dyn SyncEmitter,
    hook: crate::hooks::HookEvent,
}

impl crate::hooks::types::HookEmitter for HookEmitterAdapter<'_> {
    fn emit(&self, event: crate::hooks::types::HookStreamEvent) {
        if let crate::hooks::types::HookStreamEvent::Output { step, stream, line } = event {
            self.emitter.emit(SyncEvent::HookOutput {
                hook: self.hook,
                step,
                stream,
                line,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        execute, plan_after_best_effort_origin_fetch, preview, HookPolicy, MutationState,
        NoopSyncEmitter, RecordingSyncEmitter, SyncErrorClass, SyncEvent, SyncPlanner, SyncStage,
        SyncStrategy,
    };

    fn commit_file(repo: &git2::Repository, path: &str, contents: &str, message: &str) {
        std::fs::write(repo.workdir().unwrap().join(path), contents).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents = parent.iter().collect::<Vec<_>>();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .unwrap();
    }

    fn divergent_worktree() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let feature = root.path().join("feature-topic");
        std::fs::create_dir(&main).unwrap();
        let repo = git2::Repository::init(&main).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Test").unwrap();
            config.set_str("user.email", "test@example.com").unwrap();
        }
        commit_file(&repo, "common", "initial\n", "initial");
        commit_file(&repo, ".gitignore", "ignored-state\n", "ignore local state");
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let branch = repo.branch("feature/topic", &head, false).unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(branch.get()));
        repo.worktree("feature-topic", &feature, Some(&options))
            .unwrap();
        drop(branch);
        drop(head);
        commit_file(&repo, "main-only", "main\n", "advance main");
        let feature_repo = git2::Repository::open(&feature).unwrap();
        commit_file(&feature_repo, "feature-only", "feature\n", "feature work");
        (root, feature)
    }

    fn conflicting_worktree() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let feature = root.path().join("feature-conflict");
        std::fs::create_dir(&main).unwrap();
        let repo = git2::Repository::init(&main).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Test").unwrap();
            config.set_str("user.email", "test@example.com").unwrap();
        }
        commit_file(&repo, "shared", "initial\n", "initial");
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let branch = repo.branch("feature/conflict", &head, false).unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(branch.get()));
        repo.worktree("feature-conflict", &feature, Some(&options))
            .unwrap();
        drop(branch);
        drop(head);
        commit_file(&repo, "shared", "main\n", "main conflict");
        let feature_repo = git2::Repository::open(&feature).unwrap();
        commit_file(&feature_repo, "shared", "feature\n", "feature conflict");
        (root, feature)
    }

    fn worktree_with_patch_already_on_base() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let feature = root.path().join("feature-applied");
        std::fs::create_dir(&main).unwrap();
        let repo = git2::Repository::init(&main).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Test").unwrap();
            config.set_str("user.email", "test@example.com").unwrap();
        }
        commit_file(&repo, "common", "initial\n", "initial");
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let branch = repo.branch("feature/applied", &head, false).unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(branch.get()));
        repo.worktree("feature-applied", &feature, Some(&options))
            .unwrap();
        drop(branch);
        drop(head);
        commit_file(&repo, "shared-change", "same patch\n", "main patch");
        let feature_repo = git2::Repository::open(&feature).unwrap();
        commit_file(
            &feature_repo,
            "shared-change",
            "same patch\n",
            "feature patch",
        );
        (root, feature)
    }

    async fn assert_conflict_is_atomic(strategy: SyncStrategy) {
        let (_root, feature) = conflicting_worktree();
        let repo = git2::Repository::open(&feature).unwrap();
        let head_before = repo.head().unwrap().target().unwrap();
        let index_before = repo
            .index()
            .unwrap()
            .get_path(Path::new("shared"), 0)
            .unwrap()
            .id;
        let contents_before = std::fs::read(feature.join("shared")).unwrap();
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan("feature/conflict", Some("main"), strategy, HookPolicy::Skip)
            .unwrap();

        let failure = execute(plan, None, &NoopSyncEmitter).await.unwrap_err();

        let repo = git2::Repository::open(&feature).unwrap();
        assert_eq!(failure.class, SyncErrorClass::Conflict);
        assert_eq!(failure.mutation_state, MutationState::NotStarted);
        assert_eq!(repo.head().unwrap().target(), Some(head_before));
        assert_eq!(
            repo.index()
                .unwrap()
                .get_path(Path::new("shared"), 0)
                .unwrap()
                .id,
            index_before
        );
        assert_eq!(
            std::fs::read(feature.join("shared")).unwrap(),
            contents_before
        );
        assert!(repo.statuses(None).unwrap().is_empty());
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        assert!(!repo.path().join("MERGE_HEAD").exists());
        assert!(!repo.path().join("rebase-merge").exists());
        assert!(!repo.path().join("rebase-apply").exists());
    }

    #[tokio::test]
    async fn rebases_one_live_resolved_worktree_onto_an_explicit_base() {
        let (_root, feature) = divergent_worktree();
        let planner = SyncPlanner::discover(&feature, None).unwrap();
        let plan = planner
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap();

        let outcome = execute(plan, None, &NoopSyncEmitter).await.unwrap();

        assert_eq!(outcome.target, "feature-topic");
        assert_eq!(outcome.base, "main");
        assert_eq!(outcome.after.behind, 0);
        assert!(feature.join("main-only").is_file());
        assert!(feature.join("feature-only").is_file());
    }

    #[tokio::test]
    async fn rebase_skips_a_patch_already_applied_to_the_base() {
        let (_root, feature) = worktree_with_patch_already_on_base();
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/applied",
                Some("main"),
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap();

        let outcome = execute(plan, None, &NoopSyncEmitter).await.unwrap();

        assert_eq!(outcome.after.behind, 0);
        let repo = git2::Repository::open(&feature).unwrap();
        assert_eq!(
            repo.head().unwrap().target(),
            repo.find_reference("refs/heads/main").unwrap().target()
        );
    }

    #[tokio::test]
    async fn merges_one_live_resolved_worktree_with_an_explicit_base() {
        let (_root, feature) = divergent_worktree();
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                &feature.to_string_lossy(),
                Some("main"),
                SyncStrategy::Merge,
                HookPolicy::Skip,
            )
            .unwrap();

        let outcome = execute(plan, None, &NoopSyncEmitter).await.unwrap();

        assert_eq!(outcome.after.behind, 0);
        let repo = git2::Repository::open(&feature).unwrap();
        assert_eq!(
            repo.head()
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .parent_count(),
            2
        );
        assert!(feature.join("main-only").is_file());
        assert!(feature.join("feature-only").is_file());
    }

    #[tokio::test]
    async fn rebase_conflict_restores_exact_prestate_without_residue() {
        assert_conflict_is_atomic(SyncStrategy::Rebase).await;
    }

    #[tokio::test]
    async fn in_memory_conflict_does_not_report_a_live_rollback_stage() {
        let (_root, feature) = conflicting_worktree();
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/conflict",
                Some("main"),
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap();
        let emitter = RecordingSyncEmitter::default();

        let failure = execute(plan, None, &emitter).await.unwrap_err();

        assert_eq!(failure.class, SyncErrorClass::Conflict);
        assert_eq!(failure.mutation_state, MutationState::NotStarted);
        assert!(emitter.events().iter().all(|event| !matches!(
            event,
            SyncEvent::StageStarted(SyncStage::Rollback)
                | SyncEvent::StageFinished {
                    stage: SyncStage::Rollback,
                    ..
                }
        )));
    }

    #[tokio::test]
    async fn merge_conflict_restores_exact_prestate_without_residue() {
        assert_conflict_is_atomic(SyncStrategy::Merge).await;
    }

    #[tokio::test]
    async fn checkout_failure_never_overwrites_ignored_user_state() {
        let (root, feature) = divergent_worktree();
        let main = git2::Repository::open(root.path().join("main")).unwrap();
        commit_file(&main, "ignored-state", "from-main\n", "track ignored path");
        std::fs::write(feature.join("ignored-state"), "preserve-user-state\n").unwrap();
        let repo = git2::Repository::open(&feature).unwrap();
        let head_before = repo.head().unwrap().target().unwrap();
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Merge,
                HookPolicy::Skip,
            )
            .unwrap();

        let failure = execute(plan, None, &NoopSyncEmitter).await.unwrap_err();

        let repo = git2::Repository::open(&feature).unwrap();
        assert_eq!(failure.mutation_state, MutationState::RolledBack);
        assert_eq!(repo.head().unwrap().target(), Some(head_before));
        assert_eq!(
            std::fs::read_to_string(feature.join("ignored-state")).unwrap(),
            "preserve-user-state\n"
        );
    }

    #[test]
    fn planner_rejects_dirty_worktrees_without_stashing() {
        let (_root, feature) = divergent_worktree();
        std::fs::write(feature.join("untracked"), "preserve\n").unwrap();

        let error = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap_err();

        assert!(matches!(error, super::SyncPlanError::Dirty { .. }));
        assert_eq!(
            std::fs::read_to_string(feature.join("untracked")).unwrap(),
            "preserve\n"
        );
    }

    #[test]
    fn planner_rejects_detached_worktrees() {
        let (_root, feature) = divergent_worktree();
        let repo = git2::Repository::open(&feature).unwrap();
        let head = repo.head().unwrap().target().unwrap();
        repo.set_head_detached(head).unwrap();

        let error = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                &feature.to_string_lossy(),
                Some("main"),
                SyncStrategy::Merge,
                HookPolicy::Skip,
            )
            .unwrap_err();

        assert!(matches!(error, super::SyncPlanError::Detached { .. }));
    }

    #[test]
    fn planner_rejects_an_existing_git_operation() {
        let (_root, feature) = divergent_worktree();
        let repo = git2::Repository::open(&feature).unwrap();
        let head = repo.head().unwrap().target().unwrap();
        std::fs::write(repo.path().join("MERGE_HEAD"), format!("{head}\n")).unwrap();

        let error = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Merge,
                HookPolicy::Skip,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            super::SyncPlanError::OperationInProgress { .. }
        ));
    }

    #[test]
    fn default_base_falls_back_to_main_worktree_branch_without_origin() {
        let (_root, feature) = divergent_worktree();

        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                None,
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap();

        assert_eq!(plan.base, "main");
    }

    #[test]
    fn explicit_and_configured_missing_bases_are_rejected() {
        let (_root, feature) = divergent_worktree();
        let explicit = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("missing"),
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap_err();
        let configured = SyncPlanner::discover(&feature, Some("missing"))
            .unwrap()
            .plan(
                "feature/topic",
                None,
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap_err();

        assert!(matches!(
            explicit,
            super::SyncPlanError::ExplicitBaseNotFound { .. }
        ));
        assert!(matches!(
            configured,
            super::SyncPlanError::DefaultBase(
                crate::ref_catalog::DefaultBaseError::ConfiguredNotFound { .. }
            )
        ));
    }

    #[test]
    fn default_base_prefers_origin_head_then_configured_ref() {
        let (_root, feature) = divergent_worktree();
        let repo = git2::Repository::open(&feature).unwrap();
        let main = repo
            .find_reference("refs/heads/main")
            .unwrap()
            .target()
            .unwrap();
        repo.remote("origin", "https://invalid.example/trench.git")
            .unwrap();
        repo.reference("refs/remotes/origin/main", main, false, "test")
            .unwrap();
        repo.reference_symbolic(
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
            false,
            "test",
        )
        .unwrap();

        let detected = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                None,
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap();
        let configured = SyncPlanner::discover(&feature, Some("main"))
            .unwrap()
            .plan(
                "feature/topic",
                None,
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap();

        assert_eq!(detected.base, "origin/main");
        assert_eq!(configured.base, "main");
    }

    #[test]
    fn real_cli_planning_warns_and_uses_local_refs_after_one_failed_fetch() {
        let (root, feature) = divergent_worktree();
        let repo = git2::Repository::open(&feature).unwrap();
        repo.remote("origin", root.path().join("missing").to_str().unwrap())
            .unwrap();
        let emitter = RecordingSyncEmitter::default();

        let plan = plan_after_best_effort_origin_fetch(
            &feature,
            None,
            "feature/topic",
            Some("main"),
            SyncStrategy::Merge,
            HookPolicy::Skip,
            &emitter,
        )
        .unwrap();

        assert_eq!(plan.base, "main");
        let events = emitter.events();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SyncEvent::StageStarted(SyncStage::Fetch)))
                .count(),
            1
        );
        assert!(events.contains(&SyncEvent::Warning {
            stage: SyncStage::Fetch,
            message: "origin fetch failed; using local refs".to_string(),
        }));
        assert!(events.iter().all(|event| !matches!(
            event,
            SyncEvent::Warning { message, .. } if message.contains("missing")
        )));
    }

    #[test]
    fn real_cli_planning_silently_skips_fetch_without_origin() {
        let (_root, feature) = divergent_worktree();
        let emitter = RecordingSyncEmitter::default();

        plan_after_best_effort_origin_fetch(
            &feature,
            None,
            "feature/topic",
            None,
            SyncStrategy::Rebase,
            HookPolicy::Skip,
            &emitter,
        )
        .unwrap();

        assert!(emitter
            .events()
            .iter()
            .all(|event| !matches!(event, SyncEvent::StageStarted(SyncStage::Fetch))));
    }

    #[tokio::test]
    async fn pre_sync_failure_stops_before_git_mutation() {
        let (_root, feature) = divergent_worktree();
        let repo = git2::Repository::open(&feature).unwrap();
        let head_before = repo.head().unwrap().target().unwrap();
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Rebase,
                HookPolicy::Run,
            )
            .unwrap();
        let hooks = crate::config::HooksConfig {
            pre_sync: Some(crate::config::HookDef {
                run: Some(vec!["exit 9".to_string()]),
                ..crate::config::HookDef::default()
            }),
            ..crate::config::HooksConfig::default()
        };

        let failure = execute(plan, Some(&hooks), &NoopSyncEmitter)
            .await
            .unwrap_err();

        let repo = git2::Repository::open(&feature).unwrap();
        assert_eq!(failure.stage, SyncStage::PreHook);
        assert_eq!(failure.class, SyncErrorClass::Hook);
        assert_eq!(failure.mutation_state, MutationState::NotStarted);
        assert_eq!(repo.head().unwrap().target(), Some(head_before));
        assert!(!feature.join("main-only").exists());
    }

    #[tokio::test]
    async fn pre_sync_ref_change_is_revalidated_before_git_mutation() {
        let (_root, feature) = divergent_worktree();
        let repo = git2::Repository::open(&feature).unwrap();
        let head_before = repo.head().unwrap().target().unwrap();
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Rebase,
                HookPolicy::Run,
            )
            .unwrap();
        let hooks = crate::config::HooksConfig {
            pre_sync: Some(crate::config::HookDef {
                run: Some(vec![
                    "git -C \"$TRENCH_REPO_PATH\" commit --allow-empty -m hook-moved-base"
                        .to_string(),
                ]),
                ..crate::config::HookDef::default()
            }),
            ..crate::config::HooksConfig::default()
        };

        let failure = execute(plan, Some(&hooks), &NoopSyncEmitter)
            .await
            .unwrap_err();

        let repo = git2::Repository::open(&feature).unwrap();
        assert_eq!(failure.stage, SyncStage::Validate);
        assert_eq!(failure.class, SyncErrorClass::PreconditionsChanged);
        assert_eq!(failure.mutation_state, MutationState::NotStarted);
        assert_eq!(repo.head().unwrap().target(), Some(head_before));
        assert!(!feature.join("main-only").exists());
    }

    #[tokio::test]
    async fn post_sync_failure_reports_applied_without_rolling_back_git() {
        let (_root, feature) = divergent_worktree();
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Merge,
                HookPolicy::Run,
            )
            .unwrap();
        let hooks = crate::config::HooksConfig {
            post_sync: Some(crate::config::HookDef {
                run: Some(vec!["exit 7".to_string()]),
                ..crate::config::HookDef::default()
            }),
            ..crate::config::HooksConfig::default()
        };

        let failure = execute(plan, Some(&hooks), &NoopSyncEmitter)
            .await
            .unwrap_err();

        let repo = git2::Repository::open(&feature).unwrap();
        let main = repo
            .find_reference("refs/heads/main")
            .unwrap()
            .target()
            .unwrap();
        let head = repo.head().unwrap().target().unwrap();
        assert_eq!(failure.stage, SyncStage::PostHook);
        assert_eq!(failure.mutation_state, MutationState::Applied);
        assert_eq!(repo.graph_ahead_behind(head, main).unwrap().1, 0);
        assert!(feature.join("main-only").is_file());
    }

    #[tokio::test]
    async fn no_hooks_policy_bypasses_configured_hooks() {
        let (_root, feature) = divergent_worktree();
        let marker = feature.join("hook-marker");
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Rebase,
                HookPolicy::Skip,
            )
            .unwrap();
        let command = format!("printf ran > {}", marker.display());
        let hook = crate::config::HookDef {
            run: Some(vec![command]),
            ..crate::config::HookDef::default()
        };
        let hooks = crate::config::HooksConfig {
            pre_sync: Some(hook.clone()),
            post_sync: Some(hook),
            ..crate::config::HooksConfig::default()
        };

        execute(plan, Some(&hooks), &NoopSyncEmitter).await.unwrap();

        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn hook_output_is_streamed_only_to_the_current_emitter() {
        let (_root, feature) = divergent_worktree();
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Rebase,
                HookPolicy::Run,
            )
            .unwrap();
        let hooks = crate::config::HooksConfig {
            pre_sync: Some(crate::config::HookDef {
                run: Some(vec!["printf pre-stream".to_string()]),
                ..crate::config::HookDef::default()
            }),
            post_sync: Some(crate::config::HookDef {
                run: Some(vec!["printf post-stream".to_string()]),
                ..crate::config::HookDef::default()
            }),
            ..crate::config::HooksConfig::default()
        };
        let emitter = RecordingSyncEmitter::default();

        execute(plan, Some(&hooks), &emitter).await.unwrap();

        let output = emitter
            .events()
            .into_iter()
            .filter_map(|event| match event {
                SyncEvent::HookOutput { hook, line, .. } => Some((hook, line)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            output,
            [
                (crate::hooks::HookEvent::PreSync, "pre-stream".to_string()),
                (crate::hooks::HookEvent::PostSync, "post-stream".to_string()),
            ]
        );
    }

    #[test]
    fn preview_validates_but_never_mutates_or_runs_hooks() {
        let (_root, feature) = divergent_worktree();
        let repo = git2::Repository::open(&feature).unwrap();
        repo.remote("origin", "https://invalid.example/secret.git")
            .unwrap();
        let head_before = repo.head().unwrap().target().unwrap();
        let fetch_head = repo.path().join("FETCH_HEAD");
        let plan = SyncPlanner::discover(&feature, None)
            .unwrap()
            .plan(
                "feature/topic",
                Some("main"),
                SyncStrategy::Rebase,
                HookPolicy::Run,
            )
            .unwrap();

        let value = serde_json::to_value(preview(plan)).unwrap();

        assert_eq!(value["dry_run"], true);
        assert_eq!(value["target"], "feature-topic");
        assert_eq!(repo.head().unwrap().target(), Some(head_before));
        assert!(!fetch_head.exists());
        assert!(!feature.join("hook-marker").exists());
    }
}
