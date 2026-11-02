use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};

use crate::{
    ref_catalog::{RefCatalog, RefCatalogError, RefSnapshot},
    tui::app::{WorktreeId, WorktreeIdentity},
    worktree_catalog::{CatalogError, WorktreeCatalog},
};

pub const WARNING_DURATION: Duration = Duration::from_secs(3);

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskToken(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmediateSnapshot {
    pub identities: Vec<WorktreeIdentity>,
    pub refs: RefSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshTask {
    FetchOrigin { token: TaskToken },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    Updated(RefSnapshot),
    NoOrigin(RefSnapshot),
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshCompletion {
    FetchOrigin {
        token: TaskToken,
        outcome: FetchOutcome,
    },
}

pub trait RefreshSource {
    type Error;

    fn immediate_snapshot(&self) -> Result<ImmediateSnapshot, Self::Error>;
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
}

impl CatalogRefreshSource {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into() }
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
    pub updating_refs: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
struct Warning {
    message: String,
    expires_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveFetch {
    token: TaskToken,
    origin: Vec<String>,
    origin_head: Option<String>,
    has_origin: bool,
}

impl ActiveFetch {
    fn new(token: TaskToken, refs: &RefSnapshot) -> Self {
        Self {
            token,
            origin: refs.origin.clone(),
            origin_head: refs.origin_head.clone(),
            has_origin: refs.has_origin,
        }
    }

    fn still_current(&self, refs: &RefSnapshot) -> bool {
        self.origin == refs.origin
            && self.origin_head == refs.origin_head
            && self.has_origin == refs.has_origin
    }
}

#[derive(Debug, Default)]
pub struct RefreshCoordinator {
    next_fetch: u64,
    snapshot: Option<ImmediateSnapshot>,
    active_fetch: Option<ActiveFetch>,
    queued_tasks: VecDeque<RefreshTask>,
    warning: Option<Warning>,
}

impl RefreshCoordinator {
    pub fn request_from<S: RefreshSource>(
        &mut self,
        cause: RefreshCause,
        source: &S,
    ) -> Result<(), S::Error> {
        let snapshot = source.immediate_snapshot()?;
        self.request(cause, snapshot);
        Ok(())
    }

    pub fn complete_at<C: Clock>(&mut self, completion: RefreshCompletion, clock: &C) {
        self.complete(completion, clock.now());
    }

    pub fn publication_at<C: Clock>(&mut self, clock: &C) -> RefreshPublication {
        self.publication(clock.now())
    }

    pub fn request(&mut self, cause: RefreshCause, snapshot: ImmediateSnapshot) {
        self.snapshot = Some(snapshot);

        if cause.fetches_origin()
            && self.active_fetch.is_none()
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.refs.has_origin)
        {
            self.next_fetch = self.next_fetch.wrapping_add(1);
            let token = TaskToken(self.next_fetch);
            let refs = &self.snapshot.as_ref().expect("snapshot checked above").refs;
            self.active_fetch = Some(ActiveFetch::new(token, refs));
            self.queued_tasks
                .push_back(RefreshTask::FetchOrigin { token });
        }
    }

    pub fn complete(&mut self, completion: RefreshCompletion, now: Instant) {
        match completion {
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
        RefreshPublication {
            identities: self
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.identities.clone())
                .unwrap_or_default(),
            refs: self.snapshot.as_ref().map(|snapshot| snapshot.refs.clone()),
            updating_refs: self.active_fetch.is_some(),
            warning: self.warning.as_ref().map(|warning| warning.message.clone()),
        }
    }

    fn complete_fetch(&mut self, token: TaskToken, outcome: FetchOutcome, now: Instant) {
        let Some(active_fetch) = self.active_fetch.as_ref() else {
            return;
        };
        if active_fetch.token != token {
            return;
        }
        let active_fetch = self
            .active_fetch
            .take()
            .expect("active fetch checked above");
        match outcome {
            FetchOutcome::Updated(refs) | FetchOutcome::NoOrigin(refs) => {
                if let Some(snapshot) = self.snapshot.as_mut() {
                    // Fetch owns origin-tracking data only. A newer local refresh
                    // may have published branches after this task captured refs,
                    // and newer remote state must not be resurrected either.
                    if active_fetch.still_current(&snapshot.refs) {
                        snapshot.refs.origin = refs.origin;
                        snapshot.refs.origin_head = refs.origin_head;
                        snapshot.refs.has_origin = refs.has_origin;
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
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
        }
    }

    #[test]
    fn fetch_completion_never_overwrites_local_refs_from_a_newer_generation() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let initial = snapshot(vec![alpha], true);
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, initial.clone());
        let RefreshTask::FetchOrigin { token: fetch_token } = coordinator.drain_tasks()[0];

        let mut newer_local = initial.clone();
        newer_local
            .refs
            .local
            .push("topic-created-locally".to_string());
        coordinator.request(RefreshCause::Watcher, newer_local);

        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: fetch_token,
                outcome: FetchOutcome::Updated(RefSnapshot {
                    local: vec!["main".to_string()],
                    origin: vec!["origin/main".to_string()],
                    origin_head: Some("origin/main".to_string()),
                    main_branch: Some("main".to_string()),
                    has_origin: true,
                }),
            },
            now,
        );

        let refs = coordinator.publication(now).refs.unwrap();
        assert_eq!(refs.local, ["main", "topic-created-locally"]);
        assert_eq!(refs.origin, ["origin/main"]);
    }

    #[test]
    fn fetch_completion_never_resurrects_origin_removed_by_a_newer_generation() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let initial = snapshot(vec![alpha], true);
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, initial.clone());
        let RefreshTask::FetchOrigin { token: fetch_token } = coordinator.drain_tasks()[0];

        let mut origin_removed = initial;
        origin_removed.refs.origin.clear();
        origin_removed.refs.origin_head = None;
        origin_removed.refs.has_origin = false;
        coordinator.request(RefreshCause::Watcher, origin_removed);

        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: fetch_token,
                outcome: FetchOutcome::Updated(RefSnapshot {
                    local: vec!["main".to_string()],
                    origin: vec!["origin/main".to_string()],
                    origin_head: Some("origin/main".to_string()),
                    main_branch: Some("main".to_string()),
                    has_origin: true,
                }),
            },
            now,
        );

        let refs = coordinator.publication(now).refs.unwrap();
        assert!(!refs.has_origin);
        assert!(refs.origin.is_empty());
        assert!(refs.origin_head.is_none());
    }

    #[test]
    fn only_explicit_network_causes_fetch_and_no_origin_is_silent() {
        let alpha = identity("/worktrees/alpha", "alpha");
        for cause in [
            RefreshCause::Watcher,
            RefreshCause::PostOperation,
            RefreshCause::EditorReturn,
        ] {
            let mut coordinator = RefreshCoordinator::default();
            coordinator.request(cause, snapshot(vec![alpha.clone()], true));
            assert!(coordinator.drain_tasks().is_empty());
        }
        for cause in [
            RefreshCause::Launch,
            RefreshCause::Manual,
            RefreshCause::RefPicker,
        ] {
            let mut coordinator = RefreshCoordinator::default();
            coordinator.request(cause, snapshot(vec![alpha.clone()], true));
            assert_eq!(coordinator.drain_tasks().len(), 1);
        }

        let mut local_only = RefreshCoordinator::default();
        local_only.request(RefreshCause::Manual, snapshot(vec![alpha], false));
        let publication = local_only.publication(Instant::now());
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
        coordinator.request(RefreshCause::Launch, snapshot(vec![alpha], true));
        let RefreshTask::FetchOrigin { token: fetch_token } = coordinator.drain_tasks()[0];

        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: fetch_token,
                outcome: FetchOutcome::NoOrigin(refs(false)),
            },
            now,
        );

        let publication = coordinator.publication(now);
        assert!(!publication.refs.unwrap().has_origin);
        assert!(publication.warning.is_none());
        assert!(!publication.updating_refs);
    }

    #[test]
    fn fetch_failure_preserves_stale_data_and_warning_expires() {
        let now = Instant::now();
        let alpha = identity("/worktrees/alpha", "alpha");
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, snapshot(vec![alpha], true));
        let tasks = coordinator.drain_tasks();
        let RefreshTask::FetchOrigin { token: fetch_token } = tasks[0];
        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: fetch_token,
                outcome: FetchOutcome::Failed,
            },
            now,
        );

        let failed = coordinator.publication(now);
        assert_eq!(failed.refs, Some(refs(true)));
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
    fn repeated_requests_share_fetch_and_late_completions_cannot_finish_a_new_fetch() {
        let now = Instant::now();
        let initial = snapshot(Vec::new(), true);
        let mut coordinator = RefreshCoordinator::default();
        coordinator.request(RefreshCause::Launch, initial.clone());
        let RefreshTask::FetchOrigin { token: first } = coordinator.drain_tasks()[0];
        coordinator.request(RefreshCause::Manual, initial.clone());
        assert!(coordinator.drain_tasks().is_empty());
        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: first,
                outcome: FetchOutcome::Updated(refs(true)),
            },
            now,
        );
        coordinator.request(RefreshCause::Manual, initial);
        let RefreshTask::FetchOrigin { token: second } = coordinator.drain_tasks()[0];
        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: first,
                outcome: FetchOutcome::Failed,
            },
            now,
        );
        assert!(coordinator.publication(now).updating_refs);
        assert!(coordinator.publication(now).warning.is_none());
        coordinator.complete(
            RefreshCompletion::FetchOrigin {
                token: second,
                outcome: FetchOutcome::Updated(refs(true)),
            },
            now,
        );
        assert!(!coordinator.publication(now).updating_refs);
    }
}
