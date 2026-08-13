use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    sync::mpsc,
};

use crate::tui::{
    app::WorktreeId,
    refresh::{
        execute_task, CatalogRefreshError, CatalogRefreshSource, GitOriginFetcher, RefreshCause,
        RefreshCompletion, RefreshCoordinator, RefreshPublication, SystemClock,
    },
    watcher::{DebouncedWatcher, DEBOUNCE_DURATION},
};

/// Live adapter around the pure refresh coordinator.
///
/// It owns the Git-backed boundaries, runs expensive row/fetch tasks off the
/// terminal event loop, and exposes ordered publications for the app reducer.
pub struct RefreshRuntime {
    source: CatalogRefreshSource,
    fetcher: GitOriginFetcher,
    coordinator: RefreshCoordinator,
    clock: SystemClock,
    filesystem_watcher: Option<DebouncedWatcher>,
    watched_rows: BTreeSet<WorktreeId>,
    completion_tx: mpsc::Sender<RefreshCompletion>,
    completion_rx: mpsc::Receiver<RefreshCompletion>,
    publications: VecDeque<RefreshPublication>,
    last_publication: Option<RefreshPublication>,
}

impl RefreshRuntime {
    pub fn new(
        cwd: impl Into<PathBuf>,
        repo_path: impl Into<PathBuf>,
        configured_base: Option<String>,
    ) -> Self {
        let (completion_tx, completion_rx) = mpsc::channel();
        Self {
            source: CatalogRefreshSource::new(cwd, configured_base),
            fetcher: GitOriginFetcher::new(repo_path),
            coordinator: RefreshCoordinator::default(),
            clock: SystemClock,
            filesystem_watcher: None,
            watched_rows: BTreeSet::new(),
            completion_tx,
            completion_rx,
            publications: VecDeque::new(),
            last_publication: None,
        }
    }

    pub fn launch(&mut self) -> Result<(), CatalogRefreshError> {
        self.request(RefreshCause::Launch)
    }

    pub fn manual(&mut self) -> Result<(), CatalogRefreshError> {
        self.request(RefreshCause::Manual)
    }

    pub fn watcher(&mut self) -> Result<(), CatalogRefreshError> {
        self.request(RefreshCause::Watcher)
    }

    pub fn editor_return(&mut self) -> Result<(), CatalogRefreshError> {
        self.request(RefreshCause::EditorReturn)
    }

    pub fn post_operation(&mut self) -> Result<(), CatalogRefreshError> {
        self.request(RefreshCause::PostOperation)
    }

    pub fn ref_picker(&mut self) -> Result<(), CatalogRefreshError> {
        self.request(RefreshCause::RefPicker)
    }

    /// Poll filesystem/completion channels and publish any state transition.
    /// This method never performs Git or network work on the calling thread.
    pub fn tick(&mut self) {
        let watcher_cause = self
            .filesystem_watcher
            .as_mut()
            .and_then(DebouncedWatcher::refresh_cause);
        if watcher_cause.is_some() {
            // Watcher discovery is best-effort: transient Git state should not
            // tear down the terminal session or discard the last publication.
            let _ = self.watcher();
        }

        while let Ok(completion) = self.completion_rx.try_recv() {
            self.coordinator.complete_at(completion, &self.clock);
            self.record_publication();
        }
        self.dispatch_tasks();

        // `publication_at` also expires a brief fetch warning on the clock.
        self.record_publication();
    }

    pub fn drain_publications(&mut self) -> Vec<RefreshPublication> {
        self.publications.drain(..).collect()
    }

    fn request(&mut self, cause: RefreshCause) -> Result<(), CatalogRefreshError> {
        self.coordinator
            .request_from(cause, &self.source, &self.clock)?;
        self.record_publication();
        self.dispatch_tasks();
        Ok(())
    }

    fn record_publication(&mut self) {
        let publication = self.coordinator.publication_at(&self.clock);
        self.rebuild_watcher(&publication);
        if self.last_publication.as_ref() == Some(&publication) {
            return;
        }
        self.last_publication = Some(publication.clone());
        self.publications.push_back(publication);
    }

    fn rebuild_watcher(&mut self, publication: &RefreshPublication) {
        let ids = publication
            .identities
            .iter()
            .map(|identity| identity.id.clone())
            .collect::<BTreeSet<_>>();
        if ids == self.watched_rows {
            return;
        }
        let paths = publication
            .identities
            .iter()
            .map(|identity| identity.path.as_path())
            .collect::<Vec<_>>();
        self.filesystem_watcher = if paths.is_empty() {
            None
        } else {
            DebouncedWatcher::from_worktree_paths(&paths, DEBOUNCE_DURATION).ok()
        };
        self.watched_rows = ids;
    }

    fn dispatch_tasks(&mut self) {
        for task in self.coordinator.drain_tasks() {
            let source = self.source.clone();
            let fetcher = self.fetcher.clone();
            let completion_tx = self.completion_tx.clone();
            std::thread::spawn(move || {
                let completion = execute_task(&source, &fetcher, task);
                let _ = completion_tx.send(completion);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::*;

    fn init_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
            .unwrap();
        drop(tree);
        drop(repo);
        dir
    }

    #[test]
    fn actual_adapter_publishes_identity_first_then_progress_and_manual_swr() {
        let repo = init_repo();
        let mut runtime = RefreshRuntime::new(repo.path(), repo.path(), None);

        runtime.launch().unwrap();
        let immediate = runtime.drain_publications();
        assert_eq!(immediate.len(), 1);
        assert_eq!(immediate[0].identities.len(), 1);
        assert_eq!(immediate[0].refs.as_ref().unwrap().local, ["main"]);
        assert!(immediate[0].statuses.is_empty());
        assert_eq!(immediate[0].waiting_rows.len(), 1);
        assert!(!immediate[0].updating_refs);

        let deadline = Instant::now() + Duration::from_secs(2);
        let settled = loop {
            runtime.tick();
            let publications = runtime.drain_publications();
            if let Some(publication) = publications
                .into_iter()
                .find(|publication| publication.waiting_rows.is_empty())
            {
                break publication;
            }
            assert!(Instant::now() < deadline, "row task did not settle");
            std::thread::yield_now();
        };
        assert_eq!(settled.statuses.len(), 1);

        runtime.manual().unwrap();
        let refreshing = runtime.drain_publications();
        assert_eq!(refreshing.len(), 1);
        assert_eq!(refreshing[0].statuses, settled.statuses);
        assert_eq!(refreshing[0].waiting_rows.len(), 1);
        assert!(!refreshing[0].updating_refs);
    }
}
