use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    time::{Duration, Instant},
};

use crate::{
    ref_catalog::{RefCatalog, RefCatalogError, RefSnapshot},
    tui::app::{WorktreeId, WorktreeIdentity, WorktreeStatus},
    worktree_catalog::{CatalogError, WorktreeCatalog},
};

pub const WARNING_DURATION: Duration = Duration::from_secs(3);
const FETCH_WATCHER_SUPPRESSION: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshCause {
    Launch,
    Manual,
    Watcher,
    PostOperation,
    EditorReturn,
    RefPicker,
}

impl RefreshCause {
    fn fetches_origin(self) -> bool {
        matches!(self, Self::Launch | Self::Manual | Self::RefPicker)
    }

    fn priority(self) -> u8 {
        match self {
            Self::Launch | Self::Watcher => 0,
            Self::RefPicker => 1,
            Self::EditorReturn => 2,
            Self::PostOperation => 3,
            Self::Manual => 4,
        }
    }

    fn may_observe_local_changes(self) -> bool {
        matches!(
            self,
            Self::Watcher | Self::PostOperation | Self::EditorReturn
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskToken {
    pub generation: u64,
    pub ref_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmediateSnapshot {
    pub identities: Vec<WorktreeIdentity>,
    pub refs: RefSnapshot,
    pub configured_base: Option<String>,
}

impl ImmediateSnapshot {
    fn base(&self) -> Option<String> {
        self.refs.default_base(self.configured_base.as_deref()).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshTask {
    Row {
        token: TaskToken,
        identity: WorktreeIdentity,
        base: Option<String>,
    },
    FetchOrigin {
        token: TaskToken,
    },
}

impl RefreshTask {
    pub fn token(&self) -> TaskToken {
        match self {
            Self::Row { token, .. } | Self::FetchOrigin { token } => *token,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    Updated(RefSnapshot),
    NoOrigin(RefSnapshot),
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshTaskFailure;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshCompletion {
    Row {
        token: TaskToken,
        id: WorktreeId,
        result: Result<WorktreeStatus, RefreshTaskFailure>,
    },
    FetchOrigin {
        token: TaskToken,
        outcome: FetchOutcome,
    },
}

pub trait RefreshSource {
    type Error;

    fn immediate_snapshot(&self) -> Result<ImmediateSnapshot, Self::Error>;

    fn row_status(
        &self,
        identity: &WorktreeIdentity,
        base: Option<&str>,
    ) -> Result<WorktreeStatus, Self::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogRefreshError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Refs(#[from] RefCatalogError),
}

#[derive(Debug, Clone)]
pub struct CatalogRefreshSource {
    cwd: PathBuf,
    configured_base: Option<String>,
}

impl CatalogRefreshSource {
    pub fn new(cwd: impl Into<PathBuf>, configured_base: Option<String>) -> Self {
        Self {
            cwd: cwd.into(),
            configured_base,
        }
    }
}

impl RefreshSource for CatalogRefreshSource {
    type Error = CatalogRefreshError;

    fn immediate_snapshot(&self) -> Result<ImmediateSnapshot, Self::Error> {
        let catalog = WorktreeCatalog::discover(&self.cwd)?;
        let identities = catalog
            .identities()
            .iter()
            .map(|identity| WorktreeIdentity {
                id: WorktreeId::new(identity.path.clone()),
                worktree: identity.worktree.clone(),
                branch: identity.branch.clone(),
                path: identity.path.clone(),
                head: identity.head.clone(),
                is_main: identity.is_main,
                is_current: identity.is_current,
                detached: identity.detached,
            })
            .collect();
        Ok(ImmediateSnapshot {
            identities,
            refs: RefCatalog::discover(&self.cwd)?,
            configured_base: self.configured_base.clone(),
        })
    }

    fn row_status(
        &self,
        identity: &WorktreeIdentity,
        base: Option<&str>,
    ) -> Result<WorktreeStatus, Self::Error> {
        let status = WorktreeCatalog::discover(&self.cwd)?
            .with_base(base)
            .status(identity.id.as_path())?;
        Ok(WorktreeStatus {
            base: status.base,
            staged: status.staged,
            modified: status.modified,
            untracked: status.untracked,
            ahead: status.ahead,
            behind: status.behind,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginFetchError {
    NoOrigin,
    Failed,
}

pub trait OriginFetcher {
    fn fetch_origin(&self) -> Result<(), OriginFetchError>;
}

#[derive(Debug, Clone)]
pub struct GitOriginFetcher {
    repo_path: PathBuf,
}

impl GitOriginFetcher {
    pub fn new(repo_path: impl Into<PathBuf>) -> Self {
        Self {
            repo_path: repo_path.into(),
        }
    }
}

impl OriginFetcher for GitOriginFetcher {
    fn fetch_origin(&self) -> Result<(), OriginFetchError> {
        match RefCatalog::discover(&self.repo_path) {
            Ok(snapshot) if !snapshot.has_origin => return Err(OriginFetchError::NoOrigin),
            Ok(_) => {}
            Err(_) => return Err(OriginFetchError::Failed),
        }
        RefCatalog::fetch_origin(&self.repo_path).map_err(|_| OriginFetchError::Failed)
    }
}

pub trait Clock {
    fn now(&self) -> Instant;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub fn execute_task<S: RefreshSource, F: OriginFetcher>(
    source: &S,
    fetcher: &F,
    task: RefreshTask,
) -> RefreshCompletion {
    match task {
        RefreshTask::Row {
            token,
            identity,
            base,
        } => RefreshCompletion::Row {
            token,
            id: identity.id.clone(),
            result: source
                .row_status(&identity, base.as_deref())
                .map_err(|_| RefreshTaskFailure),
        },
        RefreshTask::FetchOrigin { token } => {
            let outcome = match fetcher.fetch_origin() {
                Ok(()) => source
                    .immediate_snapshot()
                    .map(|snapshot| FetchOutcome::Updated(snapshot.refs))
                    .unwrap_or(FetchOutcome::Failed),
                Err(OriginFetchError::NoOrigin) => source
                    .immediate_snapshot()
                    .map(|snapshot| FetchOutcome::NoOrigin(snapshot.refs))
                    .unwrap_or(FetchOutcome::Failed),
                Err(OriginFetchError::Failed) => FetchOutcome::Failed,
            };
            RefreshCompletion::FetchOrigin { token, outcome }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshPublication {
    pub identities: Vec<WorktreeIdentity>,
    pub refs: Option<RefSnapshot>,
    pub statuses: BTreeMap<WorktreeId, WorktreeStatus>,
    pub waiting_rows: BTreeSet<WorktreeId>,
    pub updating_refs: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
struct LocalRun {
    token: TaskToken,
    cause: RunCause,
    remaining: BTreeSet<WorktreeId>,
}

#[derive(Debug, Clone)]
struct PendingRun {
    token: TaskToken,
    cause: RunCause,
    snapshot: ImmediateSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunCause {
    Requested(RefreshCause),
    RefsUpdated,
}

impl RunCause {
    fn priority(self) -> u8 {
        match self {
            Self::Requested(cause) => cause.priority(),
            Self::RefsUpdated => u8::MAX,
        }
    }
}

#[derive(Debug, Clone)]
struct Warning {
    message: String,
    expires_at: Instant,
}

#[derive(Debug, Default)]
pub struct RefreshCoordinator {
    generation: u64,
    ref_revision: u64,
    snapshot: Option<ImmediateSnapshot>,
    statuses: BTreeMap<WorktreeId, WorktreeStatus>,
    active_local: Option<LocalRun>,
    pending_local: Option<PendingRun>,
    active_fetch: Option<TaskToken>,
    queued_tasks: VecDeque<RefreshTask>,
    warning: Option<Warning>,
    suppress_watcher_until: Option<Instant>,
}

impl RefreshCoordinator {
    pub fn request_from<S: RefreshSource, C: Clock>(
        &mut self,
        cause: RefreshCause,
        source: &S,
        clock: &C,
    ) -> Result<(), S::Error> {
        let snapshot = source.immediate_snapshot()?;
        self.request(cause, snapshot, clock.now());
        Ok(())
    }

    pub fn complete_at<C: Clock>(&mut self, completion: RefreshCompletion, clock: &C) {
        self.complete(completion, clock.now());
    }

    pub fn publication_at<C: Clock>(&mut self, clock: &C) -> RefreshPublication {
        self.publication(clock.now())
    }

    pub fn request(&mut self, cause: RefreshCause, snapshot: ImmediateSnapshot, now: Instant) {
        if cause == RefreshCause::Watcher
            && (self.active_fetch.is_some()
                || self
                    .suppress_watcher_until
                    .is_some_and(|deadline| now <= deadline))
        {
            return;
        }

        let changed = self.publish_immediate(snapshot.clone());
        let fetch_already_covers_request = cause.fetches_origin() && self.active_fetch.is_some();

        match self.active_local.as_ref() {
            None => self.start_new_run(RunCause::Requested(cause), snapshot),
            Some(active)
                if changed
                    || (!fetch_already_covers_request
                        && (cause.priority() > active.cause.priority()
                            || cause.may_observe_local_changes())) =>
            {
                self.queue_pending(RunCause::Requested(cause), snapshot);
            }
            Some(_) => {}
        }

        if cause.fetches_origin()
            && self.active_fetch.is_none()
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.refs.has_origin)
        {
            let token = self.latest_token();
            self.active_fetch = Some(token);
            self.queued_tasks
                .push_back(RefreshTask::FetchOrigin { token });
        }
    }

    pub fn complete(&mut self, completion: RefreshCompletion, now: Instant) {
        match completion {
            RefreshCompletion::Row { token, id, result } => self.complete_row(token, id, result),
            RefreshCompletion::FetchOrigin { token, outcome } => {
                self.complete_fetch(token, outcome, now)
            }
        }
    }

    pub fn drain_tasks(&mut self) -> Vec<RefreshTask> {
        self.queued_tasks.drain(..).collect()
    }

    pub fn publication(&mut self, now: Instant) -> RefreshPublication {
        if self
            .warning
            .as_ref()
            .is_some_and(|warning| now >= warning.expires_at)
        {
            self.warning = None;
        }
        let mut waiting_rows = BTreeSet::new();
        if let Some(active) = &self.active_local {
            waiting_rows.extend(active.remaining.iter().cloned());
        }
        if let Some(pending) = &self.pending_local {
            waiting_rows.extend(
                pending
                    .snapshot
                    .identities
                    .iter()
                    .map(|identity| identity.id.clone()),
            );
        }
        if let Some(snapshot) = &self.snapshot {
            waiting_rows.retain(|id| snapshot.identities.iter().any(|row| &row.id == id));
        } else {
            waiting_rows.clear();
        }

        RefreshPublication {
            identities: self
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.identities.clone())
                .unwrap_or_default(),
            refs: self.snapshot.as_ref().map(|snapshot| snapshot.refs.clone()),
            statuses: self.statuses.clone(),
            waiting_rows,
            updating_refs: self.active_fetch.is_some(),
            warning: self.warning.as_ref().map(|warning| warning.message.clone()),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn ref_revision(&self) -> u64 {
        self.ref_revision
    }

    fn publish_immediate(&mut self, snapshot: ImmediateSnapshot) -> bool {
        let changed = self.snapshot.as_ref() != Some(&snapshot);
        let ids = snapshot
            .identities
            .iter()
            .map(|identity| identity.id.clone())
            .collect::<BTreeSet<_>>();
        self.statuses.retain(|id, _| ids.contains(id));
        self.snapshot = Some(snapshot);
        changed
    }

    fn start_new_run(&mut self, cause: RunCause, snapshot: ImmediateSnapshot) {
        self.generation = self.generation.wrapping_add(1);
        let token = self.latest_token();
        self.start_run(PendingRun {
            token,
            cause,
            snapshot,
        });
    }

    fn queue_pending(&mut self, cause: RunCause, snapshot: ImmediateSnapshot) {
        if self.pending_local.as_ref().is_some_and(|pending| {
            pending.snapshot == snapshot && pending.cause.priority() >= cause.priority()
        }) {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let token = self.latest_token();
        self.pending_local = Some(PendingRun {
            token,
            cause,
            snapshot,
        });
    }

    fn start_run(&mut self, run: PendingRun) {
        let base = run.snapshot.base();
        let remaining = run
            .snapshot
            .identities
            .iter()
            .map(|identity| identity.id.clone())
            .collect::<BTreeSet<_>>();
        for identity in &run.snapshot.identities {
            self.queued_tasks.push_back(RefreshTask::Row {
                token: run.token,
                identity: identity.clone(),
                base: base.clone(),
            });
        }
        self.active_local = (!remaining.is_empty()).then_some(LocalRun {
            token: run.token,
            cause: run.cause,
            remaining,
        });
    }

    fn complete_row(
        &mut self,
        token: TaskToken,
        id: WorktreeId,
        result: Result<WorktreeStatus, RefreshTaskFailure>,
    ) {
        let latest = self.latest_token();
        let Some(active) = self.active_local.as_mut() else {
            return;
        };
        if active.token != token || !active.remaining.remove(&id) {
            return;
        }

        let current_id = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.identities.iter().any(|identity| identity.id == id));
        if token == latest && current_id {
            if let Ok(status) = result {
                self.statuses.insert(id, status);
            }
        }

        if active.remaining.is_empty() {
            self.active_local = None;
            if let Some(pending) = self.pending_local.take() {
                self.start_run(pending);
            }
        }
    }

    fn complete_fetch(&mut self, token: TaskToken, outcome: FetchOutcome, now: Instant) {
        if self.active_fetch != Some(token) {
            return;
        }
        self.active_fetch = None;
        match outcome {
            FetchOutcome::Updated(refs) | FetchOutcome::NoOrigin(refs) => {
                self.ref_revision = self.ref_revision.wrapping_add(1);
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.refs = refs;
                }
                self.suppress_watcher_until = Some(now + FETCH_WATCHER_SUPPRESSION);
                if let Some(snapshot) = self.snapshot.clone() {
                    match self.active_local {
                        Some(_) => self.queue_pending(RunCause::RefsUpdated, snapshot),
                        None => self.start_new_run(RunCause::RefsUpdated, snapshot),
                    }
                }
            }
            FetchOutcome::Failed => {
                self.warning = Some(Warning {
                    message: "Could not update origin; showing local refs".to_string(),
                    expires_at: now + WARNING_DURATION,
                });
            }
        }
    }

    fn latest_token(&self) -> TaskToken {
        TaskToken {
            generation: self.generation,
            ref_revision: self.ref_revision,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, path::PathBuf};

    use super::*;

    fn identity(path: &str, worktree: &str) -> WorktreeIdentity {
        WorktreeIdentity {
            id: WorktreeId::new(path),
            worktree: worktree.to_string(),
            branch: Some(format!("feature/{worktree}")),
            path: PathBuf::from(path),
            head: Some("1234567890abcdef".to_string()),
            is_main: false,
            is_current: false,
            detached: false,
        }
    }

    fn refs(has_origin: bool) -> RefSnapshot {
        RefSnapshot {
            local: vec!["main".to_string()],
            origin: has_origin
                .then(|| "origin/main".to_string())
                .into_iter()
                .collect(),
            origin_head: has_origin.then(|| "origin/main".to_string()),
            main_branch: Some("main".to_string()),
            has_origin,
        }
    }

    fn snapshot(rows: Vec<WorktreeIdentity>, has_origin: bool) -> ImmediateSnapshot {
        ImmediateSnapshot {
            identities: rows,
            refs: refs(has_origin),
            configured_base: None,
        }
    }

    fn row_task(tasks: &[RefreshTask], index: usize) -> (TaskToken, WorktreeId) {
        let RefreshTask::Row {
            token, identity, ..
        } = &tasks[index]
        else {
            panic!("expected row task");
        };
        (*token, identity.id.clone())
    }

    #[test]
    fn launch_publishes_identities_and_local_refs_before_progressive_rows() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let beta = identity("/worktrees/beta", "beta");
        let mut coordinator = RefreshCoordinator::default();

        coordinator.request(
            RefreshCause::Launch,
            snapshot(vec![alpha.clone(), beta.clone()], false),
            now,
        );

        let initial = coordinator.publication(now);
        assert_eq!(initial.identities, [alpha.clone(), beta.clone()]);
        assert_eq!(initial.refs.unwrap().local, ["main"]);
        assert!(initial.statuses.is_empty());
        assert_eq!(initial.waiting_rows, BTreeSet::from([alpha.id, beta.id]));

        let tasks = coordinator.drain_tasks();
        let (token, id) = row_task(&tasks, 0);
        let status = WorktreeStatus {
            modified: 2,
            ..WorktreeStatus::default()
        };
        coordinator.complete(
            RefreshCompletion::Row {
                token,
                id: id.clone(),
                result: Ok(status.clone()),
            },
            now,
        );

        let progressive = coordinator.publication(now);
        assert_eq!(progressive.statuses.get(&id), Some(&status));
        assert!(!progressive.waiting_rows.contains(&id));
        assert_eq!(progressive.waiting_rows.len(), 1);
    }

    #[test]
    fn replacement_keeps_last_successful_status_visible_until_new_result() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let mut coordinator = RefreshCoordinator::default();
        let immediate = snapshot(vec![alpha.clone()], false);
        coordinator.request(RefreshCause::Launch, immediate.clone(), now);
        let first = coordinator.drain_tasks();
        let (first_token, id) = row_task(&first, 0);
        let stale = WorktreeStatus {
            modified: 1,
            ..WorktreeStatus::default()
        };
        coordinator.complete(
            RefreshCompletion::Row {
                token: first_token,
                id: id.clone(),
                result: Ok(stale.clone()),
            },
            now,
        );

        coordinator.request(RefreshCause::Manual, immediate, now);

        let refreshing = coordinator.publication(now);
        assert_eq!(refreshing.statuses.get(&id), Some(&stale));
        assert!(refreshing.waiting_rows.contains(&id));
    }

    #[test]
    fn newer_generation_rejects_late_row_results_and_reconciles_by_stable_id() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let mut renamed = alpha.clone();
        renamed.worktree = "alpha-renamed".to_string();
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(
            RefreshCause::Launch,
            snapshot(vec![alpha.clone()], false),
            now,
        );
        let first = coordinator.drain_tasks();
        let (stale_token, id) = row_task(&first, 0);

        coordinator.request(
            RefreshCause::Watcher,
            snapshot(vec![renamed.clone()], false),
            now,
        );
        coordinator.complete(
            RefreshCompletion::Row {
                token: stale_token,
                id: id.clone(),
                result: Ok(WorktreeStatus {
                    modified: 9,
                    ..WorktreeStatus::default()
                }),
            },
            now,
        );

        let after_late = coordinator.publication(now);
        assert_eq!(after_late.identities, [renamed]);
        assert!(!after_late.statuses.contains_key(&id));
        let replacement = coordinator.drain_tasks();
        let (current_token, current_id) = row_task(&replacement, 0);
        assert!(current_token.generation > stale_token.generation);
        coordinator.complete(
            RefreshCompletion::Row {
                token: current_token,
                id: current_id.clone(),
                result: Ok(WorktreeStatus {
                    modified: 1,
                    ..WorktreeStatus::default()
                }),
            },
            now,
        );
        assert_eq!(
            coordinator
                .publication(now)
                .statuses
                .get(&current_id)
                .map(|status| status.modified),
            Some(1)
        );
    }

    #[test]
    fn removed_worktrees_never_accept_in_flight_results() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let beta = identity("/worktrees/beta", "beta");
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(
            RefreshCause::Launch,
            snapshot(vec![alpha.clone(), beta.clone()], false),
            now,
        );
        let tasks = coordinator.drain_tasks();
        let (alpha_token, alpha_id) = row_task(&tasks, 0);
        let (beta_token, beta_id) = row_task(&tasks, 1);

        coordinator.request(RefreshCause::Watcher, snapshot(vec![beta], false), now);
        coordinator.complete(
            RefreshCompletion::Row {
                token: alpha_token,
                id: alpha_id.clone(),
                result: Ok(WorktreeStatus {
                    staged: 7,
                    ..WorktreeStatus::default()
                }),
            },
            now,
        );
        coordinator.complete(
            RefreshCompletion::Row {
                token: beta_token,
                id: beta_id,
                result: Err(RefreshTaskFailure),
            },
            now,
        );

        assert!(!coordinator
            .publication(now)
            .statuses
            .contains_key(&alpha_id));
    }

    #[test]
    fn repeated_dirty_events_keep_only_one_pending_local_recompute() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let immediate = snapshot(vec![alpha], false);
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, immediate.clone(), now);
        let first = coordinator.drain_tasks();
        let first_generation = coordinator.generation();

        coordinator.request(RefreshCause::Watcher, immediate.clone(), now);
        coordinator.request(RefreshCause::Watcher, immediate.clone(), now);
        coordinator.request(RefreshCause::Watcher, immediate, now);

        assert_eq!(coordinator.generation(), first_generation + 1);
        assert!(coordinator.drain_tasks().is_empty());
        let (token, id) = row_task(&first, 0);
        coordinator.complete(
            RefreshCompletion::Row {
                token,
                id,
                result: Err(RefreshTaskFailure),
            },
            now,
        );
        let pending = coordinator.drain_tasks();
        assert_eq!(
            pending
                .iter()
                .filter(|task| matches!(task, RefreshTask::Row { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn same_or_weaker_requests_coalesce_while_work_is_active() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let immediate = snapshot(vec![alpha], false);
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Manual, immediate.clone(), now);
        let initial_generation = coordinator.generation();
        let tasks = coordinator.drain_tasks();

        coordinator.request(RefreshCause::Launch, immediate.clone(), now);
        coordinator.request(RefreshCause::RefPicker, immediate, now);

        assert_eq!(coordinator.generation(), initial_generation);
        assert!(coordinator.drain_tasks().is_empty());
        let (token, id) = row_task(&tasks, 0);
        coordinator.complete(
            RefreshCompletion::Row {
                token,
                id,
                result: Err(RefreshTaskFailure),
            },
            now,
        );
        assert!(coordinator.drain_tasks().is_empty());
    }

    #[test]
    fn manual_refresh_attaches_to_the_active_global_fetch_slot() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let immediate = snapshot(vec![alpha], true);
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, immediate.clone(), now);
        let first = coordinator.drain_tasks();
        assert_eq!(
            first
                .iter()
                .filter(|task| matches!(task, RefreshTask::FetchOrigin { .. }))
                .count(),
            1
        );

        coordinator.request(RefreshCause::Manual, immediate, now);

        assert!(coordinator.drain_tasks().is_empty());
        assert!(coordinator.publication(now).updating_refs);
    }

    #[test]
    fn fetch_completion_advances_ref_revision_and_recomputes_comparisons() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, snapshot(vec![alpha], true), now);
        let tasks = coordinator.drain_tasks();
        let (stale_row_token, stale_row_id) = row_task(&tasks, 0);
        let fetch_token = tasks
            .iter()
            .find_map(|task| match task {
                RefreshTask::FetchOrigin { token } => Some(*token),
                RefreshTask::Row { .. } => None,
            })
            .unwrap();
        let revision = coordinator.ref_revision();
        let refreshed_refs = RefSnapshot {
            local: vec!["main".to_string()],
            origin: vec!["origin/main".to_string(), "origin/topic".to_string()],
            origin_head: Some("origin/main".to_string()),
            main_branch: Some("main".to_string()),
            has_origin: true,
        };

        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: fetch_token,
                outcome: FetchOutcome::Updated(refreshed_refs.clone()),
            },
            now,
        );

        assert_eq!(coordinator.ref_revision(), revision + 1);
        assert_eq!(coordinator.publication(now).refs, Some(refreshed_refs));
        assert!(!coordinator.publication(now).updating_refs);
        assert!(coordinator
            .publication(now)
            .waiting_rows
            .contains(&WorktreeId::new("/worktrees/alpha")));

        coordinator.complete(
            RefreshCompletion::Row {
                token: stale_row_token,
                id: stale_row_id.clone(),
                result: Ok(WorktreeStatus {
                    ahead: Some(99),
                    ..WorktreeStatus::default()
                }),
            },
            now,
        );
        assert!(!coordinator
            .publication(now)
            .statuses
            .contains_key(&stale_row_id));
    }

    #[test]
    fn watcher_storms_during_and_just_after_fetch_do_not_schedule_work() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let immediate = snapshot(vec![alpha], true);
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, immediate.clone(), now);
        let tasks = coordinator.drain_tasks();
        let fetch_token = tasks
            .iter()
            .find_map(|task| match task {
                RefreshTask::FetchOrigin { token } => Some(*token),
                RefreshTask::Row { .. } => None,
            })
            .unwrap();
        let generation = coordinator.generation();

        for _ in 0..10 {
            coordinator.request(RefreshCause::Watcher, immediate.clone(), now);
        }
        assert_eq!(coordinator.generation(), generation);
        assert!(coordinator.drain_tasks().is_empty());

        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: fetch_token,
                outcome: FetchOutcome::Updated(immediate.refs.clone()),
            },
            now,
        );
        let after_fetch_generation = coordinator.generation();
        coordinator.request(
            RefreshCause::Watcher,
            immediate,
            now + Duration::from_millis(200),
        );
        assert_eq!(coordinator.generation(), after_fetch_generation);
    }

    #[test]
    fn only_explicit_network_causes_fetch_and_no_origin_is_silent() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        for cause in [
            RefreshCause::Watcher,
            RefreshCause::PostOperation,
            RefreshCause::EditorReturn,
        ] {
            let mut coordinator = RefreshCoordinator::default();
            coordinator.request(cause, snapshot(vec![alpha.clone()], true), now);
            assert!(coordinator
                .drain_tasks()
                .iter()
                .all(|task| !matches!(task, RefreshTask::FetchOrigin { .. })));
        }
        for cause in [
            RefreshCause::Launch,
            RefreshCause::Manual,
            RefreshCause::RefPicker,
        ] {
            let mut coordinator = RefreshCoordinator::default();
            coordinator.request(cause, snapshot(vec![alpha.clone()], true), now);
            assert!(coordinator
                .drain_tasks()
                .iter()
                .any(|task| matches!(task, RefreshTask::FetchOrigin { .. })));
        }

        let mut local_only = RefreshCoordinator::default();
        local_only.request(RefreshCause::Manual, snapshot(vec![alpha], false), now);
        let publication = local_only.publication(now);
        assert!(!publication.updating_refs);
        assert!(publication.warning.is_none());
        assert!(local_only
            .drain_tasks()
            .iter()
            .all(|task| !matches!(task, RefreshTask::FetchOrigin { .. })));
    }

    #[test]
    fn origin_removed_during_fetch_remains_a_silent_local_only_transition() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, snapshot(vec![alpha], true), now);
        let fetch_token = coordinator
            .drain_tasks()
            .into_iter()
            .find_map(|task| match task {
                RefreshTask::FetchOrigin { token } => Some(token),
                RefreshTask::Row { .. } => None,
            })
            .unwrap();

        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: fetch_token,
                outcome: FetchOutcome::NoOrigin(refs(false)),
            },
            now,
        );

        let publication = coordinator.publication(now);
        assert_eq!(coordinator.ref_revision(), 1);
        assert!(!publication.refs.unwrap().has_origin);
        assert!(publication.warning.is_none());
        assert!(!publication.updating_refs);
    }

    #[test]
    fn fetch_failure_preserves_stale_data_and_warning_expires() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, snapshot(vec![alpha], true), now);
        let tasks = coordinator.drain_tasks();
        let (row_token, row_id) = row_task(&tasks, 0);
        let fetch_token = tasks
            .iter()
            .find_map(|task| match task {
                RefreshTask::FetchOrigin { token } => Some(*token),
                RefreshTask::Row { .. } => None,
            })
            .unwrap();
        let stale = WorktreeStatus {
            ahead: Some(2),
            ..WorktreeStatus::default()
        };
        coordinator.complete(
            RefreshCompletion::Row {
                token: row_token,
                id: row_id.clone(),
                result: Ok(stale.clone()),
            },
            now,
        );

        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: fetch_token,
                outcome: FetchOutcome::Failed,
            },
            now,
        );

        let failed = coordinator.publication(now);
        assert_eq!(failed.statuses.get(&row_id), Some(&stale));
        assert!(!failed.updating_refs);
        assert_eq!(
            failed.warning.as_deref(),
            Some("Could not update origin; showing local refs")
        );
        assert!(coordinator
            .publication(now + WARNING_DURATION)
            .warning
            .is_none());
    }

    #[test]
    fn injected_source_clock_tasks_and_fetcher_keep_boundaries_deterministic() {
        #[derive(Clone)]
        struct FakeSource {
            immediate: ImmediateSnapshot,
            status: WorktreeStatus,
        }

        impl RefreshSource for FakeSource {
            type Error = ();

            fn immediate_snapshot(&self) -> Result<ImmediateSnapshot, Self::Error> {
                Ok(self.immediate.clone())
            }

            fn row_status(
                &self,
                _identity: &WorktreeIdentity,
                _base: Option<&str>,
            ) -> Result<WorktreeStatus, Self::Error> {
                Ok(self.status.clone())
            }
        }

        struct FakeFetcher(Cell<usize>);

        impl OriginFetcher for FakeFetcher {
            fn fetch_origin(&self) -> Result<(), OriginFetchError> {
                self.0.set(self.0.get() + 1);
                Ok(())
            }
        }

        struct FakeClock(Instant);

        impl Clock for FakeClock {
            fn now(&self) -> Instant {
                self.0
            }
        }

        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let id = alpha.id.clone();
        let source = FakeSource {
            immediate: snapshot(vec![alpha], true),
            status: WorktreeStatus {
                untracked: 3,
                ..WorktreeStatus::default()
            },
        };
        let fetcher = FakeFetcher(Cell::new(0));
        let clock = FakeClock(now);
        let mut coordinator = RefreshCoordinator::default();

        coordinator
            .request_from(RefreshCause::Launch, &source, &clock)
            .unwrap();
        let tasks = coordinator.drain_tasks();
        let completions = tasks
            .into_iter()
            .map(|task| execute_task(&source, &fetcher, task))
            .collect::<Vec<_>>();
        for completion in completions {
            coordinator.complete_at(completion, &clock);
        }

        assert_eq!(fetcher.0.get(), 1);
        assert_eq!(
            coordinator
                .publication_at(&clock)
                .statuses
                .get(&id)
                .map(|status| status.untracked),
            Some(3)
        );
    }
}
