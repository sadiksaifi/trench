use std::fmt;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::HooksConfig;
use crate::hooks::{HookEnvContext, HookEvent};
use crate::worktree_catalog::{CatalogError, WorktreeCatalog, WorktreeIdentity};
use crate::{git, hooks};

/// The independent user choices accepted by worktree removal.
///
/// Each field maps to one CLI flag. In particular, confirmation, dirty
/// worktree removal, and unmerged branch deletion are never conflated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemoveOptions {
    pub yes: bool,
    pub force_worktree: bool,
    pub delete_branch: bool,
    pub force_branch: bool,
    pub no_hooks: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalHookPolicy {
    Run,
    Skip,
}

/// Proof that confirmation can be collected from an interactive terminal.
///
/// The private field prevents non-interactive callers from claiming terminal
/// access. Callers should detect this before prompting, then pass the token to
/// [`RemovalAssessment::confirm_interactively`].
#[derive(Debug)]
pub struct InteractiveTerminal {
    _private: (),
}

impl InteractiveTerminal {
    pub fn detect() -> Option<Self> {
        (std::io::stdin().is_terminal() && std::io::stderr().is_terminal())
            .then_some(Self { _private: () })
    }

    #[cfg(test)]
    fn for_test() -> Self {
        Self { _private: () }
    }
}

/// A non-cloneable receipt for a successful interactive prompt.
///
/// The receipt captures the complete assessment, so it cannot authorize a
/// different target or stale safety facts.
#[derive(Debug)]
pub struct InteractiveConfirmationReceipt {
    expected: RemovalAssessment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalConfirmation {
    Flag,
    Interactive,
    DryRun,
}

impl fmt::Display for RemovalConfirmation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Flag => "flag",
            Self::Interactive => "interactive",
            Self::DryRun => "dry_run",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    canonical_path: PathBuf,
}

impl DirectoryIdentity {
    fn read(path: &Path) -> Result<Self, RemovalAssessmentError> {
        let metadata = std::fs::symlink_metadata(path).map_err(|source| {
            RemovalAssessmentError::TargetUnavailable {
                path: path.to_path_buf(),
                source,
            }
        })?;
        if !metadata.file_type().is_dir() {
            return Err(RemovalAssessmentError::TargetIsNotDirectory {
                path: path.to_path_buf(),
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                canonical_path: path.canonicalize().map_err(|source| {
                    RemovalAssessmentError::TargetUnavailable {
                        path: path.to_path_buf(),
                        source,
                    }
                })?,
            })
        }
    }
}

/// Live, read-only removal facts derived from the shared worktree catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovalAssessment {
    worktree: String,
    branch: Option<String>,
    path: PathBuf,
    is_main: bool,
    detached: bool,
    dirty: bool,
    base: Option<String>,
    merged: Option<bool>,
    #[serde(skip)]
    repo_path: PathBuf,
    #[serde(skip)]
    head_oid: git2::Oid,
    #[serde(skip)]
    branch_oid: Option<git2::Oid>,
    #[serde(skip)]
    base_ref: Option<String>,
    #[serde(skip)]
    base_oid: Option<git2::Oid>,
    #[serde(skip)]
    admin_dir: PathBuf,
    #[serde(skip)]
    directory_identity: DirectoryIdentity,
}

#[derive(Debug, thiserror::Error)]
pub enum RemovalAssessmentError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Git(#[from] git::GitError),
    #[error("worktree has no readable HEAD: {path}")]
    MissingHead { path: PathBuf },
    #[error("worktree target is unavailable: {path}: {source}")]
    TargetUnavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("worktree target is not a directory: {path}")]
    TargetIsNotDirectory { path: PathBuf },
    #[error("worktree repository identity is unavailable: {path}: {source}")]
    RepositoryIdentity {
        path: PathBuf,
        #[source]
        source: git2::Error,
    },
    #[error("local branch '{branch}' no longer points at the worktree HEAD")]
    BranchHeadChanged { branch: String },
}

impl RemovalAssessment {
    pub fn worktree(&self) -> &str {
        &self.worktree
    }

    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_main(&self) -> bool {
        self.is_main
    }

    pub fn detached(&self) -> bool {
        self.detached
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    pub fn merged(&self) -> Option<bool> {
        self.merged
    }

    /// Resolve a target from live Git state without fetching, writing state, or
    /// creating directories.
    pub fn discover(
        cwd: &Path,
        selector: &str,
        configured_base: Option<&str>,
    ) -> Result<Self, RemovalAssessmentError> {
        let repo_path = git::discover_repo(cwd)?.path;
        Self::discover_in_repo(&repo_path, selector, configured_base)
    }

    fn discover_in_repo(
        repo_path: &Path,
        selector: &str,
        configured_base: Option<&str>,
    ) -> Result<Self, RemovalAssessmentError> {
        let catalog = WorktreeCatalog::discover(repo_path)?.with_base(configured_base);
        let identity = catalog.resolve(selector)?.clone();
        let status = catalog.status(&identity.path)?;
        Self::from_live(
            repo_path,
            &identity,
            status.base,
            status.staged,
            status.modified,
            status.untracked,
            status.conflicted,
        )
    }

    fn from_live(
        repo_path: &Path,
        identity: &WorktreeIdentity,
        base: Option<String>,
        staged: u32,
        modified: u32,
        untracked: u32,
        conflicted: u32,
    ) -> Result<Self, RemovalAssessmentError> {
        let head_oid = identity
            .head
            .as_deref()
            .and_then(|head| git2::Oid::from_str(head).ok())
            .ok_or_else(|| RemovalAssessmentError::MissingHead {
                path: identity.path.clone(),
            })?;
        let repo = git2::Repository::open(&identity.path).map_err(|source| {
            RemovalAssessmentError::RepositoryIdentity {
                path: identity.path.clone(),
                source,
            }
        })?;
        let admin_dir = repo.path().canonicalize().map_err(|source| {
            RemovalAssessmentError::TargetUnavailable {
                path: repo.path().to_path_buf(),
                source,
            }
        })?;
        let branch_oid = identity
            .branch
            .as_deref()
            .map(|branch| local_branch_oid(&repo, branch))
            .transpose()?;
        if identity.branch.is_some() && branch_oid != Some(head_oid) {
            return Err(RemovalAssessmentError::BranchHeadChanged {
                branch: identity.branch.clone().expect("checked above"),
            });
        }
        let resolved_base = base
            .as_deref()
            .map(|base| resolve_base(&repo, base))
            .transpose()?
            .flatten();
        let (base_ref, base_oid) = resolved_base
            .map(|resolved| (Some(resolved.reference), Some(resolved.oid)))
            .unwrap_or((None, None));
        let merged = match (branch_oid, base_oid) {
            (Some(branch), Some(base)) => Some(
                branch == base
                    || repo
                        .graph_descendant_of(base, branch)
                        .map_err(git::GitError::Git)?,
            ),
            _ => None,
        };

        Ok(Self {
            worktree: identity.worktree.clone(),
            branch: identity.branch.clone(),
            path: identity.path.clone(),
            is_main: identity.is_main,
            detached: identity.detached,
            dirty: staged > 0 || modified > 0 || untracked > 0 || conflicted > 0,
            base,
            merged,
            repo_path: repo_path.to_path_buf(),
            head_oid,
            branch_oid,
            base_ref,
            base_oid,
            admin_dir,
            directory_identity: DirectoryIdentity::read(&identity.path)?,
        })
    }

    /// Run an interactive prompt and issue a receipt only when it is accepted.
    pub fn confirm_interactively<F>(
        &self,
        _terminal: InteractiveTerminal,
        prompt: F,
    ) -> std::io::Result<Option<InteractiveConfirmationReceipt>>
    where
        F: FnOnce() -> std::io::Result<bool>,
    {
        prompt().map(|confirmed| {
            confirmed.then(|| InteractiveConfirmationReceipt {
                expected: self.clone(),
            })
        })
    }

    /// Convert live facts and explicit user choices into an immutable plan.
    ///
    /// This entry point authorizes live removal only through `--yes`. Use
    /// [`Self::authorize_confirmed`] after an accepted interactive prompt.
    pub fn authorize(
        self,
        options: RemoveOptions,
    ) -> Result<RemovalPlan, RemovalAuthorizationError> {
        self.authorize_inner(options, None)
    }

    pub fn authorize_confirmed(
        self,
        options: RemoveOptions,
        receipt: InteractiveConfirmationReceipt,
    ) -> Result<RemovalPlan, RemovalAuthorizationError> {
        self.authorize_inner(options, Some(receipt))
    }

    fn authorize_inner(
        self,
        options: RemoveOptions,
        receipt: Option<InteractiveConfirmationReceipt>,
    ) -> Result<RemovalPlan, RemovalAuthorizationError> {
        if self.is_main {
            return Err(RemovalAuthorizationError::MainWorktree);
        }
        if options.force_branch && !options.delete_branch {
            return Err(RemovalAuthorizationError::ForceBranchRequiresDeleteBranch);
        }
        if options.delete_branch && self.detached {
            return Err(RemovalAuthorizationError::DetachedHasNoLocalBranch);
        }
        let confirmation = match (options.dry_run, options.yes, receipt) {
            (true, _, None) => RemovalConfirmation::DryRun,
            (false, true, None) => RemovalConfirmation::Flag,
            (false, false, Some(receipt)) if receipt.expected == self => {
                RemovalConfirmation::Interactive
            }
            (false, false, Some(_)) => {
                return Err(RemovalAuthorizationError::ConfirmationReceiptMismatch);
            }
            (_, _, Some(_)) => {
                return Err(RemovalAuthorizationError::ConfirmationReceiptUnexpected);
            }
            (false, false, None) => {
                return Err(RemovalAuthorizationError::ConfirmationRequired);
            }
        };
        if self.dirty && !options.force_worktree {
            return Err(RemovalAuthorizationError::DirtyWorktree);
        }
        if options.delete_branch {
            match self.merged {
                Some(false) if !options.force_branch => {
                    return Err(RemovalAuthorizationError::UnmergedBranch {
                        branch: self.branch.clone().expect("attached target has a branch"),
                    });
                }
                None => return Err(RemovalAuthorizationError::MergeStatusUnavailable),
                _ => {}
            }
        }

        Ok(RemovalPlan {
            dry_run: options.dry_run,
            worktree: self.worktree.clone(),
            branch: self.branch.clone(),
            path: self.path.clone(),
            detached: self.detached,
            dirty: self.dirty,
            merged: self.merged,
            yes: options.yes,
            confirmation,
            force_worktree: options.force_worktree,
            force_worktree_applied: self.dirty && options.force_worktree,
            delete_branch: options.delete_branch,
            force_branch: options.force_branch,
            force_branch_applied: options.delete_branch
                && self.merged == Some(false)
                && options.force_branch,
            no_hooks: options.no_hooks,
            hook_policy: if options.no_hooks {
                RemovalHookPolicy::Skip
            } else {
                RemovalHookPolicy::Run
            },
            expected: self,
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RemovalAuthorizationError {
    #[error("the main worktree cannot be removed")]
    MainWorktree,
    #[error("removal requires confirmation; pass --yes in non-interactive use")]
    ConfirmationRequired,
    #[error("interactive confirmation was recorded for different removal facts")]
    ConfirmationReceiptMismatch,
    #[error("interactive confirmation is not applicable with --yes or --dry-run")]
    ConfirmationReceiptUnexpected,
    #[error("worktree is dirty; pass --force-worktree to remove it")]
    DirtyWorktree,
    #[error("--force-branch requires --delete-branch")]
    ForceBranchRequiresDeleteBranch,
    #[error("detached worktrees have no local branch to delete")]
    DetachedHasNoLocalBranch,
    #[error("branch '{branch}' is not merged; pass --force-branch with --delete-branch")]
    UnmergedBranch { branch: String },
    #[error("branch merge status is unavailable; local branch deletion was not authorized")]
    MergeStatusUnavailable,
}

/// A complete, stable preview of the exact removal that was authorized.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RemovalPlan {
    dry_run: bool,
    worktree: String,
    branch: Option<String>,
    path: PathBuf,
    detached: bool,
    dirty: bool,
    merged: Option<bool>,
    yes: bool,
    confirmation: RemovalConfirmation,
    force_worktree: bool,
    force_worktree_applied: bool,
    delete_branch: bool,
    force_branch: bool,
    force_branch_applied: bool,
    no_hooks: bool,
    hook_policy: RemovalHookPolicy,
    #[serde(skip)]
    expected: RemovalAssessment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalStage {
    Revalidate,
    PreRemove,
    RemoveWorktree,
    Prune,
    DeleteBranch,
    PostRemove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalMutationState {
    NotStarted,
    Applied,
    PartiallyApplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalErrorClass {
    PreconditionsChanged,
    Git,
    Hook,
    HookTimeout,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalEvent {
    Started,
    StageStarted {
        stage: RemovalStage,
    },
    Output {
        hook: HookEvent,
        step: hooks::types::HookStep,
        stream: hooks::types::OutputStream,
        line: String,
    },
    StageFinished {
        stage: RemovalStage,
        duration: Duration,
        success: bool,
    },
    Warning {
        stage: RemovalStage,
        message: String,
    },
    Finished {
        mutation_state: RemovalMutationState,
        duration: Duration,
    },
}

pub trait RemovalEventSink: Send + Sync {
    fn emit(&self, event: RemovalEvent);
}

#[derive(Debug, Default)]
pub struct NoopRemovalEventSink;

impl RemovalEventSink for NoopRemovalEventSink {
    fn emit(&self, _event: RemovalEvent) {}
}

#[derive(Debug, Clone, Default)]
pub struct RecordingRemovalEventSink {
    events: Arc<Mutex<Vec<RemovalEvent>>>,
}

impl RecordingRemovalEventSink {
    pub fn events(&self) -> Vec<RemovalEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl RemovalEventSink for RecordingRemovalEventSink {
    fn emit(&self, event: RemovalEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalHooksStatus {
    None,
    Planned,
    Ran,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovalOutcome {
    pub dry_run: bool,
    pub worktree: String,
    pub branch: Option<String>,
    pub path: PathBuf,
    pub detached: bool,
    pub dirty: bool,
    pub merged: Option<bool>,
    pub yes: bool,
    pub confirmation: RemovalConfirmation,
    pub force_worktree: bool,
    pub force_worktree_applied: bool,
    pub delete_branch: bool,
    pub branch_deleted: bool,
    pub force_branch: bool,
    pub force_branch_applied: bool,
    pub no_hooks: bool,
    pub hooks: RemovalHooksStatus,
    pub mutation_state: RemovalMutationState,
    pub warning: Option<String>,
}

impl RemovalOutcome {
    fn from_plan(
        plan: &RemovalPlan,
        hooks: RemovalHooksStatus,
        mutation_state: RemovalMutationState,
        branch_deleted: bool,
        warning: Option<String>,
    ) -> Self {
        Self {
            dry_run: plan.dry_run,
            worktree: plan.worktree.clone(),
            branch: plan.branch.clone(),
            path: plan.path.clone(),
            detached: plan.detached,
            dirty: plan.dirty,
            merged: plan.merged,
            yes: plan.yes,
            confirmation: plan.confirmation,
            force_worktree: plan.force_worktree,
            force_worktree_applied: plan.force_worktree_applied,
            delete_branch: plan.delete_branch,
            branch_deleted,
            force_branch: plan.force_branch,
            force_branch_applied: plan.force_branch_applied,
            no_hooks: plan.no_hooks,
            hooks,
            mutation_state,
            warning,
        }
    }
}

impl fmt::Display for RemovalPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_human_outcome(formatter, "Would remove", self, false, None)
    }
}

impl fmt::Display for RemovalOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let action = if self.dry_run {
            "Would remove"
        } else {
            "Removed"
        };
        write!(
            formatter,
            "{action} worktree '{}' at {}",
            self.worktree,
            self.path.display()
        )?;
        match (&self.branch, self.delete_branch, self.branch_deleted) {
            (Some(branch), true, true) => write!(formatter, "; deleted local branch '{branch}'")?,
            (Some(branch), true, false) => {
                write!(formatter, "; local branch '{branch}' selected for deletion")?
            }
            (Some(branch), false, _) => write!(formatter, "; kept local branch '{branch}'")?,
            (None, _, _) => write!(formatter, "; detached HEAD (no local branch)")?,
        }
        write!(
            formatter,
            "; --yes={}; confirmation={}; --force-worktree={} (applied={}); --delete-branch={}; --force-branch={} (applied={}); --no-hooks={}",
            self.yes,
            self.confirmation,
            self.force_worktree,
            self.force_worktree_applied,
            self.delete_branch,
            self.force_branch,
            self.force_branch_applied,
            self.no_hooks,
        )?;
        if let Some(warning) = &self.warning {
            write!(formatter, "; warning: {warning}")?;
        }
        Ok(())
    }
}

fn write_human_outcome(
    formatter: &mut fmt::Formatter<'_>,
    action: &str,
    plan: &RemovalPlan,
    branch_deleted: bool,
    warning: Option<&str>,
) -> fmt::Result {
    write!(
        formatter,
        "{action} worktree '{}' at {}",
        plan.worktree,
        plan.path.display()
    )?;
    match (&plan.branch, plan.delete_branch, branch_deleted) {
        (Some(branch), true, true) => write!(formatter, "; deleted local branch '{branch}'")?,
        (Some(branch), true, false) => write!(formatter, "; would delete local branch '{branch}'")?,
        (Some(branch), false, _) => write!(formatter, "; would keep local branch '{branch}'")?,
        (None, _, _) => write!(formatter, "; detached HEAD (no local branch)")?,
    }
    write!(
        formatter,
        "; --yes={}; confirmation={}; --force-worktree={} (applied={}); --delete-branch={}; --force-branch={} (applied={}); --no-hooks={}",
        plan.yes,
        plan.confirmation,
        plan.force_worktree,
        plan.force_worktree_applied,
        plan.delete_branch,
        plan.force_branch,
        plan.force_branch_applied,
        plan.no_hooks,
    )?;
    if let Some(warning) = warning {
        write!(formatter, "; warning: {warning}")?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error, Serialize)]
#[error("{message}")]
pub struct RemovalFailure {
    pub stage: RemovalStage,
    pub mutation_state: RemovalMutationState,
    pub class: RemovalErrorClass,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained_quarantine: Option<PathBuf>,
}

/// Execute an authorized plan through Git worktree semantics.
///
/// A dry-run plan returns without revalidation, hooks, Git mutation, logging,
/// database access, directory creation, or network access.
pub async fn execute(
    plan: RemovalPlan,
    hooks_config: Option<&HooksConfig>,
    sink: &dyn RemovalEventSink,
) -> Result<RemovalOutcome, RemovalFailure> {
    if plan.dry_run {
        let hooks = dry_run_hook_status(&plan, hooks_config);
        return Ok(RemovalOutcome::from_plan(
            &plan,
            hooks,
            RemovalMutationState::NotStarted,
            false,
            None,
        ));
    }

    let operation_started = Instant::now();
    sink.emit(RemovalEvent::Started);
    let stage_started = start_stage(sink, RemovalStage::Revalidate);
    if let Err(error) = revalidate(&plan) {
        finish_stage(sink, RemovalStage::Revalidate, stage_started, false);
        return Err(fail(
            sink,
            operation_started,
            RemovalStage::Revalidate,
            RemovalMutationState::NotStarted,
            RemovalErrorClass::PreconditionsChanged,
            error,
        ));
    }
    finish_stage(sink, RemovalStage::Revalidate, stage_started, true);

    let hook_context = hook_context(&plan);
    let hooks_status = hook_status(&plan, hooks_config);
    let stage_started = start_stage(sink, RemovalStage::PreRemove);
    if plan.hook_policy == RemovalHookPolicy::Run {
        if let Some(pre_remove) = hooks_config.and_then(|hooks| hooks.pre_remove.as_ref()) {
            let hook_sink = HookEventAdapter {
                sink,
                hook: HookEvent::PreRemove,
            };
            if let Err(error) = hooks::runner::execute_hook(
                &HookEvent::PreRemove,
                pre_remove,
                &hook_context,
                &plan.expected.repo_path,
                &plan.path,
                &hook_sink,
            )
            .await
            {
                finish_stage(sink, RemovalStage::PreRemove, stage_started, false);
                return Err(fail(
                    sink,
                    operation_started,
                    RemovalStage::PreRemove,
                    RemovalMutationState::NotStarted,
                    classify_hook_error(&error),
                    error.to_string(),
                ));
            }
        }
    }
    finish_stage(sink, RemovalStage::PreRemove, stage_started, true);

    // This is the final mutation boundary. It catches hook or concurrent
    // changes and the Git helper proves the directory/repository identity
    // again immediately before descriptor-bound removal.
    if let Err(error) = revalidate(&plan) {
        return Err(fail(
            sink,
            operation_started,
            RemovalStage::Revalidate,
            RemovalMutationState::NotStarted,
            RemovalErrorClass::PreconditionsChanged,
            error,
        ));
    }

    let stage_started = start_stage(sink, RemovalStage::RemoveWorktree);
    let pending = match prepare_exact_worktree_removal(&plan.expected, plan.force_worktree_applied)
    {
        Ok(pending) => pending,
        Err(error) => {
            finish_stage(sink, RemovalStage::RemoveWorktree, stage_started, false);
            let mutation_state = if error.partially_applied {
                RemovalMutationState::PartiallyApplied
            } else {
                RemovalMutationState::NotStarted
            };
            return Err(fail_with_quarantine(
                sink,
                operation_started,
                RemovalStage::RemoveWorktree,
                mutation_state,
                classify_git_error(&error.error),
                error.error.to_string(),
                error.retained_quarantine,
            ));
        }
    };
    finish_stage(sink, RemovalStage::RemoveWorktree, stage_started, true);

    let stage_started = start_stage(sink, RemovalStage::Prune);
    if let Err(error) = pending.retire(&plan.expected, &ProductionRemovalBoundary) {
        finish_stage(sink, RemovalStage::Prune, stage_started, false);
        return Err(fail_with_quarantine(
            sink,
            operation_started,
            RemovalStage::Prune,
            RemovalMutationState::PartiallyApplied,
            classify_git_error(&error.error),
            error.error.to_string(),
            error.retained_quarantine,
        ));
    }
    finish_stage(sink, RemovalStage::Prune, stage_started, true);

    let mut branch_deleted = false;
    if plan.delete_branch {
        let stage_started = start_stage(sink, RemovalStage::DeleteBranch);
        if let Err(error) = delete_exact_branch(&plan.expected, plan.force_branch_applied) {
            finish_stage(sink, RemovalStage::DeleteBranch, stage_started, false);
            return Err(fail(
                sink,
                operation_started,
                RemovalStage::DeleteBranch,
                RemovalMutationState::PartiallyApplied,
                classify_git_error(&error),
                error.to_string(),
            ));
        }
        branch_deleted = true;
        finish_stage(sink, RemovalStage::DeleteBranch, stage_started, true);
    }

    let mut warning = None;
    let stage_started = start_stage(sink, RemovalStage::PostRemove);
    if plan.hook_policy == RemovalHookPolicy::Run {
        if let Some(post_remove) = hooks_config.and_then(|hooks| hooks.post_remove.as_ref()) {
            let hook_sink = HookEventAdapter {
                sink,
                hook: HookEvent::PostRemove,
            };
            if let Err(error) = hooks::runner::execute_hook(
                &HookEvent::PostRemove,
                post_remove,
                &hook_context,
                &plan.expected.repo_path,
                &plan.expected.repo_path,
                &hook_sink,
            )
            .await
            {
                let message = format!("post_remove hook failed: {error}");
                warning = Some(message.clone());
                sink.emit(RemovalEvent::Warning {
                    stage: RemovalStage::PostRemove,
                    message,
                });
                finish_stage(sink, RemovalStage::PostRemove, stage_started, false);
                finish_operation(sink, operation_started, RemovalMutationState::Applied);
                return Ok(RemovalOutcome::from_plan(
                    &plan,
                    hooks_status,
                    RemovalMutationState::Applied,
                    branch_deleted,
                    warning,
                ));
            }
        }
    }
    finish_stage(sink, RemovalStage::PostRemove, stage_started, true);

    finish_operation(sink, operation_started, RemovalMutationState::Applied);
    Ok(RemovalOutcome::from_plan(
        &plan,
        hooks_status,
        RemovalMutationState::Applied,
        branch_deleted,
        warning,
    ))
}

fn revalidate(plan: &RemovalPlan) -> Result<(), String> {
    let live = RemovalAssessment::discover_in_repo(
        &plan.expected.repo_path,
        plan.path.to_string_lossy().as_ref(),
        plan.expected.base.as_deref(),
    )
    .map_err(|error| error.to_string())?;
    if live == plan.expected {
        Ok(())
    } else {
        Err("removal plan no longer matches live Git state".to_string())
    }
}

fn hook_status(plan: &RemovalPlan, config: Option<&HooksConfig>) -> RemovalHooksStatus {
    let configured = config
        .map(|hooks| hooks.pre_remove.is_some() || hooks.post_remove.is_some())
        .unwrap_or(false);
    match (plan.hook_policy, configured) {
        (RemovalHookPolicy::Skip, true) => RemovalHooksStatus::Skipped,
        (RemovalHookPolicy::Run, true) => RemovalHooksStatus::Ran,
        _ => RemovalHooksStatus::None,
    }
}

fn dry_run_hook_status(plan: &RemovalPlan, config: Option<&HooksConfig>) -> RemovalHooksStatus {
    let configured = config
        .map(|hooks| hooks.pre_remove.is_some() || hooks.post_remove.is_some())
        .unwrap_or(false);
    match (plan.hook_policy, configured) {
        (RemovalHookPolicy::Skip, true) => RemovalHooksStatus::Skipped,
        (RemovalHookPolicy::Run, true) => RemovalHooksStatus::Planned,
        _ => RemovalHooksStatus::None,
    }
}

fn hook_context(plan: &RemovalPlan) -> HookEnvContext {
    HookEnvContext {
        worktree_path: plan.path.to_string_lossy().into_owned(),
        worktree_name: plan.worktree.clone(),
        branch: plan.branch.clone().unwrap_or_default(),
        repo_name: plan
            .expected
            .repo_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repository".to_string()),
        repo_path: plan.expected.repo_path.to_string_lossy().into_owned(),
        base_branch: plan.expected.base.clone().unwrap_or_default(),
    }
}

struct HookEventAdapter<'a> {
    sink: &'a dyn RemovalEventSink,
    hook: HookEvent,
}

impl hooks::types::HookEmitter for HookEventAdapter<'_> {
    fn emit(&self, event: hooks::types::HookStreamEvent) {
        if let hooks::types::HookStreamEvent::Output { step, stream, line } = event {
            self.sink.emit(RemovalEvent::Output {
                hook: self.hook,
                step,
                stream,
                line,
            });
        }
    }
}

fn classify_hook_error(error: &anyhow::Error) -> RemovalErrorClass {
    if error
        .chain()
        .any(|cause| cause.is::<hooks::runner::HookTimeoutError>())
    {
        RemovalErrorClass::HookTimeout
    } else {
        RemovalErrorClass::Hook
    }
}

fn classify_git_error(error: &git::GitError) -> RemovalErrorClass {
    match error {
        git::GitError::PreconditionsChanged => RemovalErrorClass::PreconditionsChanged,
        git::GitError::Io(_) => RemovalErrorClass::Io,
        _ => RemovalErrorClass::Git,
    }
}

fn start_stage(sink: &dyn RemovalEventSink, stage: RemovalStage) -> Instant {
    sink.emit(RemovalEvent::StageStarted { stage });
    Instant::now()
}

fn finish_stage(sink: &dyn RemovalEventSink, stage: RemovalStage, started: Instant, success: bool) {
    sink.emit(RemovalEvent::StageFinished {
        stage,
        duration: started.elapsed(),
        success,
    });
}

fn finish_operation(
    sink: &dyn RemovalEventSink,
    started: Instant,
    mutation_state: RemovalMutationState,
) {
    sink.emit(RemovalEvent::Finished {
        mutation_state,
        duration: started.elapsed(),
    });
}

fn fail(
    sink: &dyn RemovalEventSink,
    started: Instant,
    stage: RemovalStage,
    mutation_state: RemovalMutationState,
    class: RemovalErrorClass,
    message: String,
) -> RemovalFailure {
    finish_operation(sink, started, mutation_state);
    RemovalFailure {
        stage,
        mutation_state,
        class,
        message,
        retained_quarantine: None,
    }
}

fn fail_with_quarantine(
    sink: &dyn RemovalEventSink,
    started: Instant,
    stage: RemovalStage,
    mutation_state: RemovalMutationState,
    class: RemovalErrorClass,
    message: String,
    retained_quarantine: Option<PathBuf>,
) -> RemovalFailure {
    finish_operation(sink, started, mutation_state);
    RemovalFailure {
        stage,
        mutation_state,
        class,
        message,
        retained_quarantine,
    }
}

#[derive(Debug)]
struct ResolvedBase {
    reference: String,
    oid: git2::Oid,
}

fn resolve_base(
    repo: &git2::Repository,
    base: &str,
) -> Result<Option<ResolvedBase>, git::GitError> {
    let candidates = if base.starts_with("refs/") {
        vec![base.to_string()]
    } else if base.starts_with("origin/") {
        vec![format!("refs/remotes/{base}")]
    } else {
        vec![
            format!("refs/heads/{base}"),
            format!("refs/remotes/origin/{base}"),
        ]
    };
    for reference in candidates {
        match repo.revparse_single(&reference) {
            Ok(object) => {
                return Ok(Some(ResolvedBase {
                    reference,
                    oid: object.peel_to_commit()?.id(),
                }));
            }
            Err(error) if error.code() == git2::ErrorCode::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(None)
}

fn local_branch_oid(repo: &git2::Repository, branch: &str) -> Result<git2::Oid, git::GitError> {
    repo.find_branch(branch, git2::BranchType::Local)
        .map_err(|error| {
            if error.code() == git2::ErrorCode::NotFound {
                git::GitError::LocalBranchNotFound {
                    branch: branch.to_string(),
                }
            } else {
                error.into()
            }
        })?
        .get()
        .peel_to_commit()
        .map(|commit| commit.id())
        .map_err(Into::into)
}

fn common_git_dir(repo_path: &Path) -> Result<PathBuf, git::GitError> {
    let repo = git2::Repository::open(repo_path)?;
    let admin = repo.path();
    if admin
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "worktrees")
    {
        admin
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or(git::GitError::PreconditionsChanged)
    } else {
        Ok(admin.to_path_buf())
    }
}

#[cfg(unix)]
struct ExactTarget {
    parent: std::fs::File,
    directory: std::fs::File,
    parent_path: PathBuf,
    leaf: std::ffi::CString,
}

#[cfg(unix)]
struct QuarantinedTarget {
    name: std::ffi::CString,
    path: PathBuf,
}

#[derive(Debug)]
struct ExactRemovalFailure {
    error: git::GitError,
    partially_applied: bool,
    retained_quarantine: Option<PathBuf>,
}

impl ExactRemovalFailure {
    fn not_started(error: git::GitError) -> Self {
        Self {
            error,
            partially_applied: false,
            retained_quarantine: None,
        }
    }

    fn retained(error: git::GitError, path: PathBuf) -> Self {
        Self {
            error,
            partially_applied: true,
            retained_quarantine: Some(path),
        }
    }

    fn recovered_partial(error: git::GitError) -> Self {
        Self {
            error,
            partially_applied: true,
            retained_quarantine: None,
        }
    }
}

#[cfg(unix)]
struct PendingExactRemoval {
    target: ExactTarget,
    quarantine: QuarantinedTarget,
}

#[cfg(unix)]
impl PendingExactRemoval {
    fn retire(
        self,
        assessment: &RemovalAssessment,
        boundary: &dyn RemovalMutationBoundary,
    ) -> Result<(), ExactRemovalFailure> {
        use std::os::fd::AsRawFd;

        let common = common_git_dir(&assessment.repo_path)
            .map_err(|error| ExactRemovalFailure::retained(error, self.quarantine.path.clone()))?;

        // Point Git back at the original name, then move the same held inode
        // back to quarantine without changing Git metadata. The original path
        // is now absent, so prune can retire metadata without ever receiving a
        // mutable pathname to delete recursively.
        self.target
            .restore(assessment, &self.quarantine, &common)
            .map_err(|error| ExactRemovalFailure::retained(error, self.quarantine.path.clone()))?;
        renameat_noreplace(
            self.target.parent.as_raw_fd(),
            &self.target.leaf,
            self.target.parent.as_raw_fd(),
            &self.quarantine.name,
        )
        .map_err(|error| {
            ExactRemovalFailure::retained(error.into(), self.quarantine.path.clone())
        })?;
        self.target
            .verify_quarantine_identity(&self.quarantine)
            .map_err(|error| ExactRemovalFailure::retained(error, self.quarantine.path.clone()))?;

        if let Err(error) = boundary.before_prune(&self.quarantine.path) {
            return Err(recover_after_deletion(
                error,
                &self.target,
                assessment,
                &self.quarantine,
                &common,
            ));
        }
        if let Err(error) = prune_worktrees(&assessment.repo_path) {
            return Err(recover_after_deletion(
                error,
                &self.target,
                assessment,
                &self.quarantine,
                &common,
            ));
        }
        delete_directory_contents(self.target.directory.as_raw_fd(), false)
            .map_err(|error| ExactRemovalFailure::retained(error, self.quarantine.path.clone()))?;
        self.target
            .verify_quarantine_identity(&self.quarantine)
            .map_err(|error| ExactRemovalFailure::retained(error, self.quarantine.path.clone()))?;
        let status = unsafe {
            libc::unlinkat(
                self.target.parent.as_raw_fd(),
                self.quarantine.name.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(ExactRemovalFailure::retained(
                std::io::Error::last_os_error().into(),
                self.quarantine.path,
            ))
        }
    }
}

#[cfg(not(unix))]
struct PendingExactRemoval;

#[cfg(not(unix))]
impl PendingExactRemoval {
    fn retire(
        self,
        _assessment: &RemovalAssessment,
        _boundary: &dyn RemovalMutationBoundary,
    ) -> Result<(), ExactRemovalFailure> {
        Err(ExactRemovalFailure::not_started(
            git::GitError::PreconditionsChanged,
        ))
    }
}

#[cfg(unix)]
impl ExactTarget {
    fn open(assessment: &RemovalAssessment) -> Result<Self, git::GitError> {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;

        let parent_path = assessment
            .path
            .parent()
            .ok_or(git::GitError::PreconditionsChanged)?;
        let parent_c = CString::new(parent_path.as_os_str().as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "worktree parent contains a NUL byte",
            )
        })?;
        let fd = unsafe {
            libc::open(
                parent_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let parent = unsafe { std::fs::File::from_raw_fd(fd) };
        let leaf = CString::new(
            assessment
                .path
                .file_name()
                .ok_or(git::GitError::PreconditionsChanged)?
                .as_bytes(),
        )
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "worktree leaf contains a NUL byte",
            )
        })?;
        let target_fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                leaf.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if target_fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let target = Self {
            parent,
            directory: unsafe { std::fs::File::from_raw_fd(target_fd) },
            parent_path: parent_path.to_path_buf(),
            leaf,
        };
        target.verify(assessment)?;
        Ok(target)
    }

    fn verify(&self, assessment: &RemovalAssessment) -> Result<(), git::GitError> {
        use std::os::unix::fs::MetadataExt;

        let held = self.directory.metadata()?;
        if (held.dev(), held.ino())
            != (
                assessment.directory_identity.device,
                assessment.directory_identity.inode,
            )
        {
            return Err(git::GitError::PreconditionsChanged);
        }
        let live_repo = git2::Repository::open(&assessment.path)?;
        let live_admin = live_repo.path().canonicalize().map_err(git::GitError::Io)?;
        if live_admin != assessment.admin_dir {
            return Err(git::GitError::PreconditionsChanged);
        }
        let live = std::fs::symlink_metadata(&assessment.path)?;
        if !live.file_type().is_dir() || (held.dev(), held.ino()) != (live.dev(), live.ino()) {
            return Err(git::GitError::PreconditionsChanged);
        }
        Ok(())
    }

    fn quarantine(
        &self,
        assessment: &RemovalAssessment,
    ) -> Result<QuarantinedTarget, git::GitError> {
        use std::ffi::{CString, OsStr};
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQUENCE: AtomicU64 = AtomicU64::new(0);

        for _ in 0..32 {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let name = CString::new(format!(".trench-remove-{}-{sequence}", std::process::id()))
                .expect("generated quarantine names contain no NUL bytes");
            match renameat_noreplace(
                self.parent.as_raw_fd(),
                &self.leaf,
                self.parent.as_raw_fd(),
                &name,
            ) {
                Ok(()) => {
                    let path = self.parent_path.join(OsStr::from_bytes(name.as_bytes()));
                    if self.verify_named(assessment, &name, &path).is_err() {
                        // The source changed after the final path verification.
                        // Restore the untrusted replacement when the original
                        // name is still free, but never pass it to Git.
                        let _ = renameat_noreplace(
                            self.parent.as_raw_fd(),
                            &name,
                            self.parent.as_raw_fd(),
                            &self.leaf,
                        );
                        return Err(git::GitError::PreconditionsChanged);
                    }
                    return Ok(QuarantinedTarget { name, path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not reserve a worktree quarantine name",
        )
        .into())
    }

    fn verify_named(
        &self,
        assessment: &RemovalAssessment,
        name: &std::ffi::CStr,
        path: &Path,
    ) -> Result<(), git::GitError> {
        use std::mem::MaybeUninit;
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;

        let mut named = MaybeUninit::<libc::stat>::uninit();
        let status = unsafe {
            libc::fstatat(
                self.parent.as_raw_fd(),
                name.as_ptr(),
                named.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let named = unsafe { named.assume_init() };
        let held = self.directory.metadata()?;
        if (held.dev(), held.ino()) != (stat_device(&named), named.st_ino) {
            return Err(git::GitError::PreconditionsChanged);
        }
        let live_repo = git2::Repository::open(path)?;
        let live_admin = live_repo.path().canonicalize().map_err(git::GitError::Io)?;
        if live_admin != assessment.admin_dir {
            return Err(git::GitError::PreconditionsChanged);
        }
        Ok(())
    }

    fn verify_quarantine(
        &self,
        assessment: &RemovalAssessment,
        quarantine: &QuarantinedTarget,
    ) -> Result<(), git::GitError> {
        self.verify_named(assessment, &quarantine.name, &quarantine.path)
    }

    fn verify_quarantine_identity(
        &self,
        quarantine: &QuarantinedTarget,
    ) -> Result<(), git::GitError> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;

        let held = self.directory.metadata()?;
        let named = stat_entry(self.parent.as_raw_fd(), &quarantine.name)?;
        if (held.dev(), held.ino()) != (named.device, named.inode)
            || named.file_type != libc::S_IFDIR
        {
            return Err(git::GitError::PreconditionsChanged);
        }
        Ok(())
    }

    fn restore(
        &self,
        assessment: &RemovalAssessment,
        quarantine: &QuarantinedTarget,
        common: &Path,
    ) -> Result<(), git::GitError> {
        use std::os::fd::AsRawFd;

        self.verify_quarantine(assessment, quarantine)?;
        renameat_noreplace(
            self.parent.as_raw_fd(),
            &quarantine.name,
            self.parent.as_raw_fd(),
            &self.leaf,
        )?;
        if let Err(error) = repair_worktree_metadata(common, &assessment.path) {
            // Keep the directory and Git metadata aligned on the quarantine
            // name if repairing the original path fails.
            let moved_back = renameat_noreplace(
                self.parent.as_raw_fd(),
                &self.leaf,
                self.parent.as_raw_fd(),
                &quarantine.name,
            )
            .is_ok();
            if moved_back {
                let _ = repair_worktree_metadata(common, &quarantine.path);
            }
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(all(unix, target_vendor = "apple"))]
fn renameat_noreplace(
    old_dir: std::os::fd::RawFd,
    old_name: &std::ffi::CStr,
    new_dir: std::os::fd::RawFd,
    new_name: &std::ffi::CStr,
) -> std::io::Result<()> {
    let status = unsafe {
        libc::renameatx_np(
            old_dir,
            old_name.as_ptr(),
            new_dir,
            new_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(unix, target_os = "linux"))]
fn renameat_noreplace(
    old_dir: std::os::fd::RawFd,
    old_name: &std::ffi::CStr,
    new_dir: std::os::fd::RawFd,
    new_name: &std::ffi::CStr,
) -> std::io::Result<()> {
    let status = unsafe {
        libc::renameat2(
            old_dir,
            old_name.as_ptr(),
            new_dir,
            new_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "linux"))))]
fn renameat_noreplace(
    _old_dir: std::os::fd::RawFd,
    _old_name: &std::ffi::CStr,
    _new_dir: std::os::fd::RawFd,
    _new_name: &std::ffi::CStr,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "exclusive descriptor-relative rename is unavailable",
    ))
}

#[cfg(unix)]
fn remove_exact_worktree(
    assessment: &RemovalAssessment,
    allow_dirty: bool,
) -> Result<(), ExactRemovalFailure> {
    let pending = prepare_exact_worktree_removal(assessment, allow_dirty)?;
    pending.retire(assessment, &ProductionRemovalBoundary)
}

#[cfg(unix)]
fn prepare_exact_worktree_removal(
    assessment: &RemovalAssessment,
    allow_dirty: bool,
) -> Result<PendingExactRemoval, ExactRemovalFailure> {
    prepare_exact_worktree_removal_with_boundary(
        assessment,
        allow_dirty,
        &ProductionRemovalBoundary,
    )
}

trait RemovalMutationBoundary {
    fn after_target_open(&self) {}
    fn after_quarantine(&self, _path: &Path) -> Result<(), git::GitError> {
        Ok(())
    }
    fn after_repair(&self, _path: &Path) -> Result<(), git::GitError> {
        Ok(())
    }
    fn before_descriptor_delete(&self, _path: &Path) -> Result<(), git::GitError> {
        Ok(())
    }
    fn before_prune(&self, _path: &Path) -> Result<(), git::GitError> {
        Ok(())
    }
}

struct ProductionRemovalBoundary;

impl RemovalMutationBoundary for ProductionRemovalBoundary {}

#[cfg(unix)]
fn remove_exact_worktree_with_boundary(
    assessment: &RemovalAssessment,
    allow_dirty: bool,
    boundary: &dyn RemovalMutationBoundary,
) -> Result<(), ExactRemovalFailure> {
    let pending = prepare_exact_worktree_removal_with_boundary(assessment, allow_dirty, boundary)?;
    pending.retire(assessment, boundary)
}

#[cfg(unix)]
fn prepare_exact_worktree_removal_with_boundary(
    assessment: &RemovalAssessment,
    allow_dirty: bool,
    boundary: &dyn RemovalMutationBoundary,
) -> Result<PendingExactRemoval, ExactRemovalFailure> {
    let target = ExactTarget::open(assessment).map_err(ExactRemovalFailure::not_started)?;
    target
        .verify(assessment)
        .map_err(ExactRemovalFailure::not_started)?;
    boundary.after_target_open();
    let common = common_git_dir(&assessment.repo_path).map_err(ExactRemovalFailure::not_started)?;
    let quarantine = target
        .quarantine(assessment)
        .map_err(ExactRemovalFailure::not_started)?;

    if let Err(error) = boundary.after_quarantine(&quarantine.path) {
        return Err(recover_or_retain(
            error,
            &target,
            assessment,
            &quarantine,
            &common,
        ));
    }
    if let Err(error) = repair_worktree_metadata(&common, &quarantine.path) {
        return Err(recover_or_retain(
            error,
            &target,
            assessment,
            &quarantine,
            &common,
        ));
    }
    if let Err(error) = boundary.after_repair(&quarantine.path) {
        return Err(recover_or_retain(
            error,
            &target,
            assessment,
            &quarantine,
            &common,
        ));
    }
    if let Err(error) = target.verify_quarantine(assessment, &quarantine) {
        return Err(recover_or_retain(
            error,
            &target,
            assessment,
            &quarantine,
            &common,
        ));
    }
    if let Err(error) = boundary.before_descriptor_delete(&quarantine.path) {
        return Err(recover_or_retain(
            error,
            &target,
            assessment,
            &quarantine,
            &common,
        ));
    }
    if let Err(error) = target.verify_quarantine(assessment, &quarantine) {
        return Err(recover_or_retain(
            error,
            &target,
            assessment,
            &quarantine,
            &common,
        ));
    }
    if !allow_dirty {
        let counts = match git::status::counts(&quarantine.path) {
            Ok(counts) => counts,
            Err(error) => {
                return Err(recover_or_retain(
                    error,
                    &target,
                    assessment,
                    &quarantine,
                    &common,
                ));
            }
        };
        if counts.staged > 0 || counts.modified > 0 || counts.untracked > 0 || counts.conflicted > 0
        {
            return Err(recover_or_retain(
                git::GitError::PreconditionsChanged,
                &target,
                assessment,
                &quarantine,
                &common,
            ));
        }
        if let Err(error) = target.verify_quarantine(assessment, &quarantine) {
            return Err(recover_or_retain(
                error,
                &target,
                assessment,
                &quarantine,
                &common,
            ));
        }
    }
    delete_quarantined_contents(&target, assessment, &quarantine)
        .map_err(|error| ExactRemovalFailure::retained(error, quarantine.path.clone()))?;
    Ok(PendingExactRemoval { target, quarantine })
}

#[cfg(unix)]
fn recover_or_retain(
    trigger: git::GitError,
    target: &ExactTarget,
    assessment: &RemovalAssessment,
    quarantine: &QuarantinedTarget,
    common: &Path,
) -> ExactRemovalFailure {
    match target.restore(assessment, quarantine, common) {
        Ok(()) => ExactRemovalFailure::not_started(trigger),
        Err(recovery) => ExactRemovalFailure::retained(
            git::GitError::CommandFailed {
                operation: "restoring quarantined worktree",
                message: format!("{trigger}; recovery failed: {recovery}"),
            },
            quarantine.path.clone(),
        ),
    }
}

#[cfg(unix)]
fn recover_after_deletion(
    trigger: git::GitError,
    target: &ExactTarget,
    assessment: &RemovalAssessment,
    quarantine: &QuarantinedTarget,
    common: &Path,
) -> ExactRemovalFailure {
    match target.restore(assessment, quarantine, common) {
        Ok(()) => ExactRemovalFailure::recovered_partial(trigger),
        Err(recovery) => ExactRemovalFailure::retained(
            git::GitError::CommandFailed {
                operation: "restoring emptied quarantined worktree",
                message: format!("{trigger}; recovery failed: {recovery}"),
            },
            quarantine.path.clone(),
        ),
    }
}

fn repair_worktree_metadata(common: &Path, path: &Path) -> Result<(), git::GitError> {
    let output = Command::new("git")
        .arg(format!("--git-dir={}", common.display()))
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["worktree", "repair"])
        .arg(path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .output()
        .map_err(|error| git::GitError::CommandFailed {
            operation: "repairing worktree metadata",
            message: error.to_string(),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(git::GitError::CommandFailed {
            operation: "repairing worktree metadata",
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

#[cfg(unix)]
fn delete_quarantined_contents(
    target: &ExactTarget,
    _assessment: &RemovalAssessment,
    quarantine: &QuarantinedTarget,
) -> Result<(), git::GitError> {
    use std::os::fd::AsRawFd;

    // Keep the linked worktree's .git marker until metadata pruning succeeds.
    // This makes every pre-prune failure descriptor-safely restorable.
    delete_directory_contents(target.directory.as_raw_fd(), true)?;
    target.verify_quarantine_identity(quarantine)?;
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct DescriptorIdentity {
    device: u64,
    inode: u64,
    file_type: libc::mode_t,
}

#[cfg(unix)]
fn descriptor_identity(metadata: &libc::stat) -> DescriptorIdentity {
    DescriptorIdentity {
        device: stat_device(metadata),
        inode: metadata.st_ino,
        file_type: metadata.st_mode & libc::S_IFMT,
    }
}

#[cfg(all(unix, target_vendor = "apple"))]
fn stat_device(metadata: &libc::stat) -> u64 {
    metadata.st_dev as u64
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn stat_device(metadata: &libc::stat) -> u64 {
    metadata.st_dev
}

#[cfg(unix)]
fn stat_descriptor(fd: std::os::fd::RawFd) -> Result<DescriptorIdentity, git::GitError> {
    use std::mem::MaybeUninit;

    let mut metadata = MaybeUninit::<libc::stat>::uninit();
    let status = unsafe { libc::fstat(fd, metadata.as_mut_ptr()) };
    if status != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(descriptor_identity(&unsafe { metadata.assume_init() }))
}

#[cfg(unix)]
fn stat_entry(
    parent: std::os::fd::RawFd,
    name: &std::ffi::CStr,
) -> Result<DescriptorIdentity, git::GitError> {
    use std::mem::MaybeUninit;

    let mut metadata = MaybeUninit::<libc::stat>::uninit();
    let status = unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(descriptor_identity(&unsafe { metadata.assume_init() }))
}

#[cfg(unix)]
struct DirectoryStream(*mut libc::DIR);

#[cfg(unix)]
impl Drop for DirectoryStream {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}

#[cfg(unix)]
fn directory_entry_names(
    directory: std::os::fd::RawFd,
) -> Result<Vec<std::ffi::CString>, git::GitError> {
    use std::ffi::{CStr, CString};

    let duplicate = unsafe { libc::dup(directory) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe {
            libc::close(duplicate);
        }
        return Err(std::io::Error::last_os_error().into());
    }
    let stream = DirectoryStream(stream);
    unsafe {
        libc::rewinddir(stream.0);
    }
    let mut names = Vec::new();
    loop {
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        names.push(CString::new(name.to_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "directory entry contains a NUL byte",
            )
        })?);
    }
    Ok(names)
}

#[cfg(unix)]
fn delete_directory_contents(
    directory: std::os::fd::RawFd,
    preserve_git_marker: bool,
) -> Result<(), git::GitError> {
    use std::os::fd::{AsRawFd, FromRawFd};

    for name in directory_entry_names(directory)? {
        if preserve_git_marker && name.as_bytes() == b".git" {
            continue;
        }
        let receipt = stat_entry(directory, &name)?;
        if receipt.file_type == libc::S_IFDIR {
            let child = unsafe {
                libc::openat(
                    directory,
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if child < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let child = unsafe { std::fs::File::from_raw_fd(child) };
            if stat_descriptor(child.as_raw_fd())? != receipt {
                return Err(git::GitError::PreconditionsChanged);
            }
            delete_directory_contents(child.as_raw_fd(), false)?;
            if stat_entry(directory, &name)? != receipt {
                return Err(git::GitError::PreconditionsChanged);
            }
            let status = unsafe { libc::unlinkat(directory, name.as_ptr(), libc::AT_REMOVEDIR) };
            if status != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        } else {
            if stat_entry(directory, &name)? != receipt {
                return Err(git::GitError::PreconditionsChanged);
            }
            let status = unsafe { libc::unlinkat(directory, name.as_ptr(), 0) };
            if status != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn remove_exact_worktree(
    _assessment: &RemovalAssessment,
    _allow_dirty: bool,
) -> Result<(), ExactRemovalFailure> {
    // The supported implementation relies on directory descriptors to keep
    // arbitrary paths out of Git's recursive worktree removal.
    Err(ExactRemovalFailure::not_started(
        git::GitError::PreconditionsChanged,
    ))
}

#[cfg(not(unix))]
fn prepare_exact_worktree_removal(
    _assessment: &RemovalAssessment,
    _allow_dirty: bool,
) -> Result<PendingExactRemoval, ExactRemovalFailure> {
    Err(ExactRemovalFailure::not_started(
        git::GitError::PreconditionsChanged,
    ))
}

fn prune_worktrees(repo_path: &Path) -> Result<(), git::GitError> {
    let common = common_git_dir(repo_path)?;
    let output = Command::new("git")
        .arg(format!("--git-dir={}", common.display()))
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["worktree", "prune", "--expire", "now"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(git::GitError::CommandFailed {
            operation: "pruning worktrees",
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

trait BranchDeletionBoundary {
    fn before_git_delete(&self) {}
}

struct ProductionBranchDeletionBoundary;

impl BranchDeletionBoundary for ProductionBranchDeletionBoundary {}

fn delete_exact_branch(assessment: &RemovalAssessment, force: bool) -> Result<(), git::GitError> {
    delete_exact_branch_with_boundary(assessment, force, &ProductionBranchDeletionBoundary)
}

fn delete_exact_branch_with_boundary(
    assessment: &RemovalAssessment,
    force: bool,
    boundary: &dyn BranchDeletionBoundary,
) -> Result<(), git::GitError> {
    boundary.before_git_delete();

    let branch = assessment
        .branch
        .as_deref()
        .ok_or(git::GitError::PreconditionsChanged)?;
    let branch_oid = assessment
        .branch_oid
        .ok_or(git::GitError::PreconditionsChanged)?;
    let repo = git2::Repository::open(&assessment.repo_path)?;
    if local_branch_oid(&repo, branch)? != branch_oid {
        return Err(git::GitError::PreconditionsChanged);
    }
    if let (Some(base_ref), Some(base_oid)) = (assessment.base_ref.as_deref(), assessment.base_oid)
    {
        let live_base = repo
            .find_reference(base_ref)?
            .peel_to_commit()
            .map(|commit| commit.id())?;
        if live_base != base_oid {
            return Err(git::GitError::PreconditionsChanged);
        }
    }

    // Git owns the final checkout-safety decision. If another worktree checks
    // out the branch after the OID receipt is checked, `git branch` refuses the
    // deletion instead of leaving that worktree with a broken symbolic HEAD.
    let output = Command::new("git")
        .arg("-C")
        .arg(&assessment.repo_path)
        .args(["branch", if force { "-D" } else { "-d" }, "--", branch])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(git::GitError::CommandFailed {
            operation: "deleting the exact local branch",
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookDef;

    struct Fixture {
        root: tempfile::TempDir,
        main: PathBuf,
        linked: PathBuf,
        branch: String,
    }

    impl Fixture {
        fn new(branch: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let main = root.path().join("main");
            let linked = root.path().join(branch.replace('/', "-"));
            std::fs::create_dir(&main).unwrap();
            let repo = init_repo(&main);
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            let local = repo.branch(branch, &head, false).unwrap();
            let mut options = git2::WorktreeAddOptions::new();
            options.reference(Some(local.get()));
            repo.worktree(&branch.replace('/', "-"), &linked, Some(&options))
                .unwrap();
            Self {
                root,
                main,
                linked,
                branch: branch.to_string(),
            }
        }

        fn assess(&self) -> RemovalAssessment {
            RemovalAssessment::discover(&self.main, &self.branch, Some("main")).unwrap()
        }

        fn options() -> RemoveOptions {
            RemoveOptions {
                yes: true,
                ..RemoveOptions::default()
            }
        }

        fn make_unmerged(&self) {
            std::fs::write(self.linked.join("change"), "new").unwrap();
            let repo = git2::Repository::open(&self.linked).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("change")).unwrap();
            index.write().unwrap();
            let tree_oid = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            let parent = repo.head().unwrap().peel_to_commit().unwrap();
            let signature = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                "branch commit",
                &tree,
                &[&parent],
            )
            .unwrap();
        }

        fn make_conflicted(&self) {
            commit_file(&self.linked, "conflict.txt", "feature", "feature conflict");
            commit_file(&self.main, "conflict.txt", "main", "main conflict");
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.linked)
                .args(["merge", "main"])
                .output()
                .unwrap();
            assert!(!output.status.success());
        }
    }

    fn init_repo(path: &Path) -> git2::Repository {
        let repo = git2::Repository::init(path).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_oid = repo.index().unwrap().write_tree().unwrap();
        {
            let tree = repo.find_tree(tree_oid).unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
                .unwrap();
        }
        repo
    }

    fn commit_file(repo_path: &Path, path: &str, contents: &str, message: &str) {
        std::fs::write(repo_path.join(path), contents).unwrap();
        let repo = git2::Repository::open(repo_path).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&parent],
        )
        .unwrap();
    }

    #[test]
    fn assessment_keeps_main_dirty_detached_and_merge_risks_independent() {
        let fixture = Fixture::new("feature/assessment");
        let clean = fixture.assess();
        assert!(!clean.is_main);
        assert!(!clean.detached);
        assert!(!clean.dirty);
        assert_eq!(clean.merged, Some(true));

        std::fs::write(fixture.linked.join("untracked"), "dirty").unwrap();
        let dirty = fixture.assess();
        assert!(dirty.dirty);
        assert_eq!(dirty.merged, Some(true));

        let main = RemovalAssessment::discover(&fixture.main, "main", Some("main")).unwrap();
        assert!(main.is_main);
        assert!(!main.detached);
    }

    #[test]
    fn conflicted_index_is_dirty_and_requires_force_worktree() {
        let fixture = Fixture::new("feature/conflicted");
        fixture.make_conflicted();

        let assessment = fixture.assess();
        assert!(assessment.dirty);
        assert_eq!(
            assessment.authorize(Fixture::options()).unwrap_err(),
            RemovalAuthorizationError::DirtyWorktree
        );
    }

    #[test]
    fn main_confirmation_dirty_and_branch_overrides_are_one_to_one() {
        let fixture = Fixture::new("feature/options");
        let assessment = fixture.assess();
        let without_confirmation = assessment.authorize(RemoveOptions::default()).unwrap_err();
        assert_eq!(
            without_confirmation,
            RemovalAuthorizationError::ConfirmationRequired
        );

        std::fs::write(fixture.linked.join("dirty"), "dirty").unwrap();
        let dirty = fixture.assess();
        assert_eq!(
            dirty.clone().authorize(Fixture::options()).unwrap_err(),
            RemovalAuthorizationError::DirtyWorktree
        );
        assert_eq!(
            dirty
                .clone()
                .authorize(RemoveOptions {
                    force_worktree: true,
                    ..RemoveOptions::default()
                })
                .unwrap_err(),
            RemovalAuthorizationError::ConfirmationRequired
        );
        let forced = dirty
            .authorize(RemoveOptions {
                force_worktree: true,
                ..Fixture::options()
            })
            .unwrap();
        assert!(forced.force_worktree_applied);
        assert!(!forced.delete_branch);

        let main = RemovalAssessment::discover(&fixture.main, "main", Some("main")).unwrap();
        assert_eq!(
            main.authorize(Fixture::options()).unwrap_err(),
            RemovalAuthorizationError::MainWorktree
        );
    }

    #[test]
    fn interactive_confirmation_is_typed_truthful_and_bound_to_the_assessment() {
        let fixture = Fixture::new("feature/interactive-confirmation");
        let assessment = fixture.assess();
        let terminal = InteractiveTerminal::for_test();
        let declined = assessment
            .confirm_interactively(terminal, || Ok(false))
            .unwrap();
        assert!(declined.is_none());

        let terminal = InteractiveTerminal::for_test();
        let receipt = assessment
            .confirm_interactively(terminal, || Ok(true))
            .unwrap()
            .expect("confirmed prompt should issue a receipt");
        let plan = assessment
            .clone()
            .authorize_confirmed(RemoveOptions::default(), receipt)
            .unwrap();
        let value = serde_json::to_value(&plan).unwrap();
        assert_eq!(value["yes"], false);
        assert_eq!(value["confirmation"], "interactive");

        let other = Fixture::new("feature/other-confirmation").assess();
        let terminal = InteractiveTerminal::for_test();
        let wrong_receipt = assessment
            .confirm_interactively(terminal, || Ok(true))
            .unwrap()
            .unwrap();
        assert_eq!(
            other
                .authorize_confirmed(RemoveOptions::default(), wrong_receipt)
                .unwrap_err(),
            RemovalAuthorizationError::ConfirmationReceiptMismatch
        );

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let outcome = runtime
            .block_on(execute(plan, None, &NoopRemovalEventSink))
            .unwrap();
        let value = serde_json::to_value(outcome).unwrap();
        assert_eq!(value["yes"], false);
        assert_eq!(value["confirmation"], "interactive");
    }

    #[test]
    fn force_branch_requires_opt_in_deletion_and_only_applies_when_unmerged() {
        let fixture = Fixture::new("feature/branch-flags");
        let merged = fixture.assess();
        assert_eq!(
            merged
                .clone()
                .authorize(RemoveOptions {
                    force_branch: true,
                    ..Fixture::options()
                })
                .unwrap_err(),
            RemovalAuthorizationError::ForceBranchRequiresDeleteBranch
        );
        let merged_plan = merged
            .authorize(RemoveOptions {
                delete_branch: true,
                force_branch: true,
                ..Fixture::options()
            })
            .unwrap();
        assert!(merged_plan.force_branch);
        assert!(!merged_plan.force_branch_applied);

        fixture.make_unmerged();
        let unmerged = fixture.assess();
        assert_eq!(unmerged.merged, Some(false));
        assert!(matches!(
            unmerged.clone().authorize(RemoveOptions {
                delete_branch: true,
                ..Fixture::options()
            }),
            Err(RemovalAuthorizationError::UnmergedBranch { .. })
        ));
        let forced = unmerged
            .authorize(RemoveOptions {
                delete_branch: true,
                force_branch: true,
                ..Fixture::options()
            })
            .unwrap();
        assert!(forced.force_branch_applied);
    }

    #[test]
    fn detached_worktree_is_removable_but_has_no_branch_delete_semantics() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let detached = root.path().join("detached");
        std::fs::create_dir(&main).unwrap();
        init_repo(&main);
        let output = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["worktree", "add", "--detach"])
            .arg(&detached)
            .output()
            .unwrap();
        assert!(output.status.success());

        let assessment =
            RemovalAssessment::discover(&main, detached.to_str().unwrap(), Some("main")).unwrap();
        assert!(assessment.detached);
        assert_eq!(assessment.branch, None);
        assessment.clone().authorize(Fixture::options()).unwrap();
        assert_eq!(
            assessment
                .authorize(RemoveOptions {
                    delete_branch: true,
                    ..Fixture::options()
                })
                .unwrap_err(),
            RemovalAuthorizationError::DetachedHasNoLocalBranch
        );
    }

    #[test]
    fn dry_run_authorizes_without_confirmation_and_has_no_side_effects() {
        let fixture = Fixture::new("feature/dry-run");
        let marker = fixture.root.path().join("hook-marker");
        let plan = fixture
            .assess()
            .authorize(RemoveOptions {
                dry_run: true,
                ..RemoveOptions::default()
            })
            .unwrap();
        let hooks = HooksConfig {
            pre_remove: Some(HookDef {
                run: Some(vec![format!("touch '{}'", marker.display())]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let events = RecordingRemovalEventSink::default();
        let outcome = runtime
            .block_on(execute(plan, Some(&hooks), &events))
            .unwrap();

        assert!(fixture.linked.exists());
        assert!(!marker.exists());
        assert_eq!(outcome.mutation_state, RemovalMutationState::NotStarted);
        assert_eq!(outcome.hooks, RemovalHooksStatus::Planned);
        assert_eq!(outcome.confirmation, RemovalConfirmation::DryRun);
        assert!(events.events().is_empty());
    }

    #[test]
    fn clean_removal_uses_git_and_preserves_unrelated_siblings() {
        let fixture = Fixture::new("feature/clean-remove");
        let unrelated = fixture.root.path().join("unrelated");
        std::fs::create_dir(&unrelated).unwrap();
        std::fs::write(unrelated.join("keep"), "safe").unwrap();
        let plan = fixture.assess().authorize(Fixture::options()).unwrap();
        let events = RecordingRemovalEventSink::default();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let outcome = runtime.block_on(execute(plan, None, &events)).unwrap();

        assert!(!fixture.linked.exists());
        assert_eq!(
            std::fs::read_to_string(unrelated.join("keep")).unwrap(),
            "safe"
        );
        assert!(git2::Repository::open(&fixture.main)
            .unwrap()
            .find_branch(&fixture.branch, git2::BranchType::Local)
            .is_ok());
        assert_eq!(outcome.mutation_state, RemovalMutationState::Applied);
        assert_eq!(outcome.confirmation, RemovalConfirmation::Flag);
        let stages = events
            .events()
            .into_iter()
            .filter_map(|event| match event {
                RemovalEvent::StageStarted { stage } => Some(stage),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stages,
            [
                RemovalStage::Revalidate,
                RemovalStage::PreRemove,
                RemovalStage::RemoveWorktree,
                RemovalStage::Prune,
                RemovalStage::PostRemove,
            ]
        );
    }

    #[test]
    fn dirty_removal_passes_force_only_after_dirty_override_is_authorized() {
        let fixture = Fixture::new("feature/dirty-remove");
        std::fs::write(fixture.linked.join("dirty"), "dirty").unwrap();
        let plan = fixture
            .assess()
            .authorize(RemoveOptions {
                force_worktree: true,
                ..Fixture::options()
            })
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let outcome = runtime
            .block_on(execute(plan, None, &NoopRemovalEventSink))
            .unwrap();

        assert!(!fixture.linked.exists());
        assert!(outcome.force_worktree_applied);
    }

    #[test]
    fn branch_deletion_is_optional_and_checks_the_exact_authorized_ref() {
        let fixture = Fixture::new("feature/delete-branch");
        let plan = fixture
            .assess()
            .authorize(RemoveOptions {
                delete_branch: true,
                ..Fixture::options()
            })
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let outcome = runtime
            .block_on(execute(plan, None, &NoopRemovalEventSink))
            .unwrap();

        assert!(outcome.branch_deleted);
        assert!(git2::Repository::open(&fixture.main)
            .unwrap()
            .find_branch(&fixture.branch, git2::BranchType::Local)
            .is_err());
    }

    #[test]
    fn branch_deletion_succeeds_when_base_is_the_target_branch() {
        let fixture = Fixture::new("feature/base-is-target");
        let assessment =
            RemovalAssessment::discover(&fixture.main, &fixture.branch, Some(&fixture.branch))
                .unwrap();
        assert_eq!(assessment.merged, Some(true));
        let plan = assessment
            .authorize(RemoveOptions {
                delete_branch: true,
                ..Fixture::options()
            })
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let outcome = runtime
            .block_on(execute(plan, None, &NoopRemovalEventSink))
            .unwrap();

        assert!(outcome.branch_deleted);
        assert!(git2::Repository::open(&fixture.main)
            .unwrap()
            .find_branch(&fixture.branch, git2::BranchType::Local)
            .is_err());
    }

    #[test]
    fn forced_unmerged_branch_deletion_is_reported_separately() {
        let fixture = Fixture::new("feature/delete-unmerged");
        fixture.make_unmerged();
        let plan = fixture
            .assess()
            .authorize(RemoveOptions {
                delete_branch: true,
                force_branch: true,
                ..Fixture::options()
            })
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let outcome = runtime
            .block_on(execute(plan, None, &NoopRemovalEventSink))
            .unwrap();

        assert!(outcome.branch_deleted);
        assert!(outcome.force_branch_applied);
        assert!(!outcome.force_worktree_applied);
    }

    #[test]
    fn pre_hook_streams_before_removal_and_no_hooks_bypasses_it() {
        let fixture = Fixture::new("feature/pre-hook");
        let marker = fixture.root.path().join("pre-marker");
        let hooks = HooksConfig {
            pre_remove: Some(HookDef {
                run: Some(vec![format!(
                    "printf hook-output; touch '{}'",
                    marker.display()
                )]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };
        let plan = fixture.assess().authorize(Fixture::options()).unwrap();
        let events = RecordingRemovalEventSink::default();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let outcome = runtime
            .block_on(execute(plan, Some(&hooks), &events))
            .unwrap();
        assert_eq!(outcome.hooks, RemovalHooksStatus::Ran);
        assert!(events.events().iter().any(|event| matches!(
            event,
            RemovalEvent::Output { hook: HookEvent::PreRemove, line, .. }
                if line == "hook-output"
        )));

        let skipped = Fixture::new("feature/skip-hook");
        let skipped_marker = skipped.root.path().join("pre-marker");
        let skipped_hooks = HooksConfig {
            pre_remove: Some(HookDef {
                run: Some(vec![format!("touch '{}'", skipped_marker.display())]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };
        let plan = skipped
            .assess()
            .authorize(RemoveOptions {
                no_hooks: true,
                ..Fixture::options()
            })
            .unwrap();
        let outcome = runtime
            .block_on(execute(plan, Some(&skipped_hooks), &NoopRemovalEventSink))
            .unwrap();
        assert_eq!(outcome.hooks, RemovalHooksStatus::Skipped);
        assert!(!skipped_marker.exists());
    }

    #[test]
    fn failed_pre_hook_keeps_worktree_and_post_hook_failure_is_warning_only() {
        let failed = Fixture::new("feature/failed-pre");
        let hooks = HooksConfig {
            pre_remove: Some(HookDef {
                run: Some(vec!["exit 7".to_string()]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };
        let plan = failed.assess().authorize(Fixture::options()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let error = runtime
            .block_on(execute(plan, Some(&hooks), &NoopRemovalEventSink))
            .unwrap_err();
        assert_eq!(error.stage, RemovalStage::PreRemove);
        assert_eq!(error.mutation_state, RemovalMutationState::NotStarted);
        assert!(failed.linked.exists());

        let warned = Fixture::new("feature/failed-post");
        let hooks = HooksConfig {
            post_remove: Some(HookDef {
                run: Some(vec!["exit 9".to_string()]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };
        let plan = warned.assess().authorize(Fixture::options()).unwrap();
        let outcome = runtime
            .block_on(execute(plan, Some(&hooks), &NoopRemovalEventSink))
            .unwrap();
        assert!(!warned.linked.exists());
        assert_eq!(outcome.mutation_state, RemovalMutationState::Applied);
        assert!(outcome.warning.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn replaced_target_path_is_never_removed() {
        let fixture = Fixture::new("feature/path-race");
        let plan = fixture.assess().authorize(Fixture::options()).unwrap();
        let foreign = fixture.root.path().join("foreign");
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("keep"), "safe").unwrap();
        std::fs::rename(&fixture.linked, fixture.root.path().join("moved-worktree")).unwrap();
        std::os::unix::fs::symlink(&foreign, &fixture.linked).unwrap();

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let error = runtime
            .block_on(execute(plan, None, &NoopRemovalEventSink))
            .unwrap_err();
        assert_eq!(error.class, RemovalErrorClass::PreconditionsChanged);
        assert_eq!(
            std::fs::read_to_string(foreign.join("keep")).unwrap(),
            "safe"
        );
    }

    #[cfg(unix)]
    struct SwapAfterTargetOpen {
        source: PathBuf,
        moved: PathBuf,
        foreign: PathBuf,
    }

    #[cfg(unix)]
    struct FailAfterQuarantine;

    #[cfg(unix)]
    impl RemovalMutationBoundary for FailAfterQuarantine {
        fn after_quarantine(&self, _path: &Path) -> Result<(), git::GitError> {
            Err(git::GitError::PreconditionsChanged)
        }
    }

    #[cfg(unix)]
    struct DirtyAfterQuarantine;

    #[cfg(unix)]
    impl RemovalMutationBoundary for DirtyAfterQuarantine {
        fn after_quarantine(&self, path: &Path) -> Result<(), git::GitError> {
            std::fs::write(path.join("late-untracked"), "preserve me").unwrap();
            Ok(())
        }
    }

    #[cfg(unix)]
    struct FailAfterRepair;

    #[cfg(unix)]
    impl RemovalMutationBoundary for FailAfterRepair {
        fn after_repair(&self, _path: &Path) -> Result<(), git::GitError> {
            Err(git::GitError::PreconditionsChanged)
        }
    }

    #[cfg(unix)]
    struct FailBeforeDescriptorDelete;

    #[cfg(unix)]
    impl RemovalMutationBoundary for FailBeforeDescriptorDelete {
        fn before_descriptor_delete(&self, _path: &Path) -> Result<(), git::GitError> {
            Err(git::GitError::PreconditionsChanged)
        }
    }

    #[cfg(unix)]
    struct FailBeforePrune;

    #[cfg(unix)]
    impl RemovalMutationBoundary for FailBeforePrune {
        fn before_prune(&self, _path: &Path) -> Result<(), git::GitError> {
            Err(git::GitError::PreconditionsChanged)
        }
    }

    #[cfg(unix)]
    struct ReplaceOriginalAfterRepair {
        original: PathBuf,
    }

    #[cfg(unix)]
    impl RemovalMutationBoundary for ReplaceOriginalAfterRepair {
        fn after_repair(&self, _path: &Path) -> Result<(), git::GitError> {
            std::fs::create_dir(&self.original).unwrap();
            std::fs::write(self.original.join("keep"), "foreign").unwrap();
            Err(git::GitError::PreconditionsChanged)
        }
    }

    #[cfg(unix)]
    #[test]
    fn failure_after_quarantine_restores_the_exact_worktree_and_git_metadata() {
        let fixture = Fixture::new("feature/fail-after-quarantine");
        let assessment = fixture.assess();

        let failure = remove_exact_worktree_with_boundary(&assessment, false, &FailAfterQuarantine)
            .unwrap_err();

        assert!(!failure.partially_applied);
        assert_eq!(failure.retained_quarantine, None);
        assert!(fixture.linked.exists());
        let live = RemovalAssessment::discover(&fixture.main, &fixture.branch, Some("main"))
            .expect("restored worktree should remain discoverable");
        assert_eq!(live.path, fixture.linked.canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn post_quarantine_dirtiness_requires_force_and_restores_safely() {
        let fixture = Fixture::new("feature/late-dirty");
        let assessment = fixture.assess();

        let failure =
            remove_exact_worktree_with_boundary(&assessment, false, &DirtyAfterQuarantine)
                .unwrap_err();

        assert!(!failure.partially_applied);
        assert_eq!(failure.retained_quarantine, None);
        assert_eq!(
            std::fs::read_to_string(fixture.linked.join("late-untracked")).unwrap(),
            "preserve me"
        );
        let live = RemovalAssessment::discover(&fixture.main, &fixture.branch, Some("main"))
            .expect("dirty worktree should be restored and discoverable");
        assert!(live.dirty());
    }

    #[cfg(unix)]
    #[test]
    fn failures_after_repair_and_before_delete_restore_without_partial_application() {
        for (branch, boundary) in [
            (
                "feature/fail-after-repair",
                &FailAfterRepair as &dyn RemovalMutationBoundary,
            ),
            (
                "feature/fail-before-delete",
                &FailBeforeDescriptorDelete as &dyn RemovalMutationBoundary,
            ),
        ] {
            let fixture = Fixture::new(branch);
            let assessment = fixture.assess();
            let failure =
                remove_exact_worktree_with_boundary(&assessment, false, boundary).unwrap_err();

            assert!(!failure.partially_applied);
            assert_eq!(failure.retained_quarantine, None);
            assert!(fixture.linked.exists());
            let live = RemovalAssessment::discover(&fixture.main, &fixture.branch, Some("main"))
                .expect("restored worktree should remain discoverable");
            assert_eq!(live.path, fixture.linked.canonicalize().unwrap());
        }
    }

    #[cfg(unix)]
    #[test]
    fn failure_before_prune_restores_the_emptied_worktree_and_reports_partial_application() {
        let fixture = Fixture::new("feature/fail-before-prune");
        let assessment = fixture.assess();

        let failure =
            remove_exact_worktree_with_boundary(&assessment, false, &FailBeforePrune).unwrap_err();

        assert!(failure.partially_applied);
        assert_eq!(failure.retained_quarantine, None);
        assert!(fixture.linked.exists());
        let live = RemovalAssessment::discover(&fixture.main, &fixture.branch, Some("main"))
            .expect("restored empty worktree should remain discoverable");
        assert_eq!(live.path, fixture.linked.canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn foreign_original_is_preserved_and_retained_quarantine_is_reported() {
        let fixture = Fixture::new("feature/retained-quarantine");
        let assessment = fixture.assess();
        let failure = remove_exact_worktree_with_boundary(
            &assessment,
            false,
            &ReplaceOriginalAfterRepair {
                original: fixture.linked.clone(),
            },
        )
        .unwrap_err();

        assert!(failure.partially_applied);
        let quarantine = failure
            .retained_quarantine
            .expect("retained quarantine should be explicit");
        assert!(quarantine.exists());
        assert_eq!(
            std::fs::read_to_string(fixture.linked.join("keep")).unwrap(),
            "foreign"
        );
        let live = RemovalAssessment::discover(&fixture.main, &fixture.branch, Some("main"))
            .expect("retained quarantine should remain discoverable");
        assert_eq!(live.path, quarantine.canonicalize().unwrap());
    }

    #[cfg(unix)]
    impl RemovalMutationBoundary for SwapAfterTargetOpen {
        fn after_target_open(&self) {
            std::fs::rename(&self.source, &self.moved).unwrap();
            std::fs::rename(&self.foreign, &self.source).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_never_removes_a_directory_replacement_after_final_verify() {
        let fixture = Fixture::new("feature/late-path-race");
        let assessment = fixture.assess();
        let moved = fixture.root.path().join("moved-authorized-worktree");
        let foreign = fixture.root.path().join("foreign-replacement");
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("keep"), "safe").unwrap();
        let boundary = SwapAfterTargetOpen {
            source: fixture.linked.clone(),
            moved,
            foreign: foreign.clone(),
        };

        let error = remove_exact_worktree_with_boundary(&assessment, false, &boundary).unwrap_err();

        assert!(matches!(error.error, git::GitError::PreconditionsChanged));
        assert_eq!(
            std::fs::read_to_string(fixture.linked.join("keep")).unwrap(),
            "safe"
        );
        assert!(boundary.moved.exists());
    }

    #[test]
    fn branch_receipt_refuses_to_delete_a_replaced_branch() {
        let fixture = Fixture::new("feature/replaced-branch");
        let assessment = fixture.assess();
        remove_exact_worktree(&assessment, false).unwrap();
        prune_worktrees(&fixture.main).unwrap();

        let repo = git2::Repository::open(&fixture.main).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let tree = parent.tree().unwrap();
        let replacement = repo
            .commit(
                None,
                &signature,
                &signature,
                "replacement branch commit",
                &tree,
                &[&parent],
            )
            .unwrap();
        repo.reference(
            &format!("refs/heads/{}", fixture.branch),
            replacement,
            true,
            "replace branch after authorization",
        )
        .unwrap();

        assert!(delete_exact_branch(&assessment, false).is_err());
        assert_eq!(
            repo.find_branch(&fixture.branch, git2::BranchType::Local)
                .unwrap()
                .get()
                .target(),
            Some(replacement)
        );
    }

    struct CheckoutAtBranchDelete {
        repo_path: PathBuf,
        checkout_path: PathBuf,
        branch: String,
    }

    struct ReplaceBranchAtDelete {
        repo_path: PathBuf,
        branch: String,
        replacement: git2::Oid,
    }

    impl BranchDeletionBoundary for ReplaceBranchAtDelete {
        fn before_git_delete(&self) {
            let repo = git2::Repository::open(&self.repo_path).unwrap();
            repo.reference(
                &format!("refs/heads/{}", self.branch),
                self.replacement,
                true,
                "replace branch at deletion boundary",
            )
            .unwrap();
        }
    }

    impl BranchDeletionBoundary for CheckoutAtBranchDelete {
        fn before_git_delete(&self) {
            let repo = git2::Repository::open(&self.repo_path).unwrap();
            let branch = repo
                .find_branch(&self.branch, git2::BranchType::Local)
                .unwrap();
            let mut options = git2::WorktreeAddOptions::new();
            options.reference(Some(branch.get()));
            repo.worktree("replacement-checkout", &self.checkout_path, Some(&options))
                .unwrap();
        }
    }

    #[test]
    fn git_branch_deletion_refuses_a_checkout_that_wins_the_final_race() {
        let fixture = Fixture::new("feature/rechecked-out-branch");
        let assessment = fixture.assess();
        remove_exact_worktree(&assessment, false).unwrap();
        prune_worktrees(&fixture.main).unwrap();

        let replacement_checkout = fixture.root.path().join("replacement-checkout");
        let repo = git2::Repository::open(&fixture.main).unwrap();
        let boundary = CheckoutAtBranchDelete {
            repo_path: fixture.main.clone(),
            checkout_path: replacement_checkout.clone(),
            branch: fixture.branch.clone(),
        };

        assert!(delete_exact_branch_with_boundary(&assessment, false, &boundary).is_err());
        assert!(replacement_checkout.exists());
        assert!(repo
            .find_branch(&fixture.branch, git2::BranchType::Local)
            .is_ok());
    }

    #[test]
    fn branch_replacement_at_deletion_boundary_is_preserved() {
        let fixture = Fixture::new("feature/replaced-at-delete");
        let assessment = fixture.assess();
        remove_exact_worktree(&assessment, false).unwrap();

        let repo = git2::Repository::open(&fixture.main).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let tree = parent.tree().unwrap();
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
        let boundary = ReplaceBranchAtDelete {
            repo_path: fixture.main.clone(),
            branch: fixture.branch.clone(),
            replacement,
        };

        assert!(delete_exact_branch_with_boundary(&assessment, true, &boundary).is_err());
        assert_eq!(
            repo.find_branch(&fixture.branch, git2::BranchType::Local)
                .unwrap()
                .get()
                .target(),
            Some(replacement)
        );
    }

    #[test]
    fn branch_transaction_refuses_when_the_merge_base_ref_moves() {
        let fixture = Fixture::new("feature/moved-base");
        let assessment = fixture.assess();
        remove_exact_worktree(&assessment, false).unwrap();
        prune_worktrees(&fixture.main).unwrap();

        let repo = git2::Repository::open(&fixture.main).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree()
            .unwrap();
        let divergent = repo
            .commit(None, &signature, &signature, "divergent base", &tree, &[])
            .unwrap();
        repo.reference(
            "refs/heads/main",
            divergent,
            true,
            "move base after authorization",
        )
        .unwrap();

        assert!(delete_exact_branch(&assessment, false).is_err());
        assert!(repo
            .find_branch(&fixture.branch, git2::BranchType::Local)
            .is_ok());
    }

    #[test]
    fn structured_plan_and_outcome_report_every_override_explicitly() {
        let fixture = Fixture::new("feature/json");
        let plan = fixture
            .assess()
            .authorize(RemoveOptions {
                yes: true,
                force_worktree: true,
                delete_branch: true,
                force_branch: true,
                no_hooks: true,
                dry_run: true,
            })
            .unwrap();
        let value = serde_json::to_value(&plan).unwrap();
        assert_eq!(value["dry_run"], true);
        assert_eq!(value["yes"], true);
        assert_eq!(value["confirmation"], "dry_run");
        assert_eq!(value["force_worktree"], true);
        assert_eq!(value["force_worktree_applied"], false);
        assert_eq!(value["delete_branch"], true);
        assert_eq!(value["force_branch"], true);
        assert_eq!(value["force_branch_applied"], false);
        assert_eq!(value["no_hooks"], true);
        assert_eq!(value["hook_policy"], "skip");
        assert!(value.get("repo_path").is_none());
        assert!(value.get("head_oid").is_none());

        let human = plan.to_string();
        assert!(human.contains("--yes=true"));
        assert!(human.contains("confirmation=dry_run"));
        assert!(human.contains("--force-worktree=true (applied=false)"));
        assert!(human.contains("--delete-branch=true"));
        assert!(human.contains("--force-branch=true (applied=false)"));
        assert!(human.contains("--no-hooks=true"));
    }
}
