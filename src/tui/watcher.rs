use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{Config, Event, PollWatcher, RecommendedWatcher, Watcher};

use crate::tui::refresh::RefreshCause;

/// Fixed trailing debounce window before emitting one local-only watcher cause.
pub const DEBOUNCE_DURATION: Duration = Duration::from_millis(150);
const FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(250);
#[cfg(test)]
const TEST_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug)]
enum WatchBackend {
    Recommended,
    Poll {
        interval: Duration,
        compare_contents: bool,
    },
}

enum ActiveWatcher {
    Recommended(RecommendedWatcher),
    Poll(PollWatcher),
}

impl ActiveWatcher {
    fn watch(&mut self, path: &Path, mode: notify::RecursiveMode) -> notify::Result<()> {
        match self {
            Self::Recommended(watcher) => watcher.watch(path, mode),
            Self::Poll(watcher) => watcher.watch(path, mode),
        }
    }
}

/// Handle a notify watch event: forward successful events to the channel,
/// log errors at warn level.
fn handle_watch_event(
    res: notify::Result<Event>,
    tx: &mpsc::Sender<()>,
    git_roots: Option<&[PathBuf]>,
) {
    match res {
        Ok(event) => {
            if git_roots.is_none_or(|roots| git_event_matters(&event, roots)) {
                let _ = tx.send(());
            }
        }
        Err(_) => {
            crate::logging::record(crate::logging::DiagnosticEvent::warning(
                crate::logging::Operation::Watch,
                crate::logging::Stage::Observe,
                Duration::ZERO,
                crate::logging::DiagnosticError::Io,
            ));
        }
    }
}

fn git_event_matters(event: &Event, roots: &[PathBuf]) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|path| {
        roots.iter().any(|root| {
            let Ok(relative) = path.strip_prefix(root) else {
                return false;
            };
            let parts: Vec<_> = relative.iter().filter_map(|part| part.to_str()).collect();
            match parts.as_slice() {
                ["HEAD" | "packed-refs" | "config"] => true,
                ["refs", ..] => {
                    !path.is_dir() && !path.extension().is_some_and(|ext| ext == "lock")
                }
                ["worktrees"] | ["worktrees", _] => {
                    matches!(
                        event.kind,
                        notify::EventKind::Create(_)
                            | notify::EventKind::Remove(_)
                            | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                    )
                }
                ["worktrees", _, "HEAD" | "gitdir" | "commondir" | "locked"] => true,
                _ => false,
            }
        })
    })
}

/// Watches filesystem paths for changes and signals when a TUI refresh is needed.
///
/// Uses the `notify` crate to monitor filesystem paths.
/// Callers debounce events before refreshing.
pub struct FileWatcher {
    _watcher: ActiveWatcher,
    event_rx: mpsc::Receiver<()>,
}

impl FileWatcher {
    /// Create a new FileWatcher monitoring the given paths.
    ///
    /// Each path is watched recursively. A non-blocking `event_rx` signals
    /// when changes are detected. Returns an error if the watcher cannot
    /// be initialized.
    #[cfg(test)]
    pub fn new(paths: &[&Path]) -> Result<Self> {
        Self::with_fallback(paths, false)
    }

    fn with_fallback(paths: &[&Path], metadata_only: bool) -> Result<Self> {
        match Self::new_with_backend(paths, WatchBackend::Recommended, metadata_only) {
            Ok(watcher) => Ok(watcher),
            Err(_) => {
                crate::logging::record(crate::logging::DiagnosticEvent::warning(
                    crate::logging::Operation::Watch,
                    crate::logging::Stage::Initialize,
                    Duration::ZERO,
                    crate::logging::DiagnosticError::Io,
                ));
                Self::new_with_backend(
                    paths,
                    WatchBackend::Poll {
                        interval: FALLBACK_POLL_INTERVAL,
                        compare_contents: true,
                    },
                    metadata_only,
                )
            }
        }
    }

    #[cfg(test)]
    pub fn with_polling(paths: &[&Path], interval: Duration) -> Result<Self> {
        Self::new_with_backend(
            paths,
            WatchBackend::Poll {
                interval,
                compare_contents: true,
            },
            false,
        )
    }

    fn new_with_backend(
        paths: &[&Path],
        backend: WatchBackend,
        metadata_only: bool,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let roots = metadata_only.then(|| paths.iter().map(|path| path.to_path_buf()).collect());
        let mut watcher = Self::build_watcher(backend, tx, roots)?;
        let mut registrations = Vec::new();
        for path in paths {
            registrations.push((
                path.to_path_buf(),
                if metadata_only {
                    notify::RecursiveMode::NonRecursive
                } else {
                    notify::RecursiveMode::Recursive
                },
            ));
            if metadata_only {
                for subdir in ["refs", "worktrees"] {
                    let child = path.join(subdir);
                    if child.is_dir() {
                        registrations.push((child, notify::RecursiveMode::Recursive));
                    }
                }
            }
        }

        let mut watched = 0;
        let mut watch_errors = 0;
        for (path, mode) in registrations {
            if path.exists() {
                match watcher.watch(&path, mode) {
                    Ok(()) => watched += 1,
                    Err(_) => {
                        crate::logging::record(crate::logging::DiagnosticEvent::warning(
                            crate::logging::Operation::Watch,
                            crate::logging::Stage::Register,
                            Duration::ZERO,
                            crate::logging::DiagnosticError::Io,
                        ));
                        watch_errors += 1;
                    }
                }
            } else {
                crate::logging::record(crate::logging::DiagnosticEvent::warning(
                    crate::logging::Operation::Watch,
                    crate::logging::Stage::Register,
                    Duration::ZERO,
                    crate::logging::DiagnosticError::NotFound,
                ));
            }
        }

        if watched == 0 && watch_errors > 0 {
            anyhow::bail!("no paths could be watched");
        }

        Ok(Self {
            _watcher: watcher,
            event_rx: rx,
        })
    }

    fn build_watcher(
        backend: WatchBackend,
        tx: mpsc::Sender<()>,
        roots: Option<Vec<PathBuf>>,
    ) -> notify::Result<ActiveWatcher> {
        match backend {
            WatchBackend::Recommended => {
                let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
                    handle_watch_event(res, &tx, roots.as_deref());
                })?;
                Ok(ActiveWatcher::Recommended(watcher))
            }
            WatchBackend::Poll {
                interval,
                compare_contents,
            } => {
                let watcher = PollWatcher::new(
                    move |res: notify::Result<Event>| {
                        handle_watch_event(res, &tx, roots.as_deref());
                    },
                    Config::default()
                        .with_poll_interval(interval)
                        .with_compare_contents(compare_contents),
                )?;
                Ok(ActiveWatcher::Poll(watcher))
            }
        }
    }

    /// Drain all pending events. Returns `true` if any events were received.
    pub fn drain_events(&self) -> bool {
        let mut got_any = false;
        while self.event_rx.try_recv().is_ok() {
            got_any = true;
        }
        got_any
    }
}

/// Wraps a `FileWatcher` with trailing-edge debounce logic.
///
/// After the first filesystem event, waits until `DEBOUNCE_DURATION` (150ms)
/// passes with no new events before signaling that a refresh is needed.
/// This prevents excessive refreshes during rapid file changes (e.g. `git fetch`).
#[derive(Debug)]
struct DebounceState {
    last_event: Option<Instant>,
    pending_refresh: bool,
    debounce: Duration,
}

impl DebounceState {
    fn new(debounce: Duration) -> Self {
        Self {
            last_event: None,
            pending_refresh: false,
            debounce,
        }
    }

    fn record_event(&mut self, now: Instant) {
        self.last_event = Some(now);
    }

    fn poll_at(&mut self, now: Instant) {
        if let Some(last) = self.last_event {
            if now.checked_duration_since(last).unwrap_or_default() >= self.debounce {
                self.last_event = None;
                self.pending_refresh = true;
            }
        }
    }

    fn take_refresh(&mut self) -> bool {
        std::mem::take(&mut self.pending_refresh)
    }

    fn take_cause(&mut self) -> Option<RefreshCause> {
        self.take_refresh().then_some(RefreshCause::Watcher)
    }
}

pub struct DebouncedWatcher {
    inner: FileWatcher,
    state: DebounceState,
}

impl DebouncedWatcher {
    /// Create with a custom debounce duration (for testing).
    #[cfg(test)]
    pub fn with_debounce(paths: &[&Path], debounce: Duration) -> Result<Self> {
        Self::from_file_watcher(
            FileWatcher::with_polling(paths, TEST_POLL_INTERVAL)?,
            debounce,
        )
    }

    /// Create a debounced watcher from worktree paths, auto-discovering `.git` dirs.
    ///
    /// Watches shared Git metadata only, including linked worktree HEAD files.
    pub fn from_worktree_paths(worktree_paths: &[&Path], debounce: Duration) -> Result<Self> {
        let all_paths = collect_watch_paths(worktree_paths);
        let path_refs: Vec<&Path> = all_paths.iter().map(|p| p.as_path()).collect();
        Self::from_file_watcher(FileWatcher::with_fallback(&path_refs, true)?, debounce)
    }

    #[cfg(test)]
    pub fn from_worktree_paths_with_polling(
        worktree_paths: &[&Path],
        debounce: Duration,
        poll_interval: Duration,
    ) -> Result<Self> {
        let all_paths = collect_watch_paths(worktree_paths);
        let path_refs: Vec<&Path> = all_paths.iter().map(|p| p.as_path()).collect();
        Self::from_file_watcher(
            FileWatcher::new_with_backend(
                &path_refs,
                WatchBackend::Poll {
                    interval: poll_interval,
                    compare_contents: true,
                },
                true,
            )?,
            debounce,
        )
    }

    fn from_file_watcher(inner: FileWatcher, debounce: Duration) -> Result<Self> {
        Ok(Self {
            inner,
            state: DebounceState::new(debounce),
        })
    }

    /// Drain pending filesystem events and update internal state.
    ///
    /// Call this every frame to keep the event queue clear. Does NOT
    /// consume the pending refresh — use `should_refresh()` for that.
    pub fn poll_events(&mut self) {
        if self.inner.drain_events() {
            self.state.record_event(Instant::now());
        }
        self.state.poll_at(Instant::now());
    }

    /// Check if a refresh is pending and consume it.
    ///
    /// Returns `true` once after the debounce window expires following
    /// detected events. Clears the pending state so subsequent calls
    /// return `false` until new events arrive.
    #[cfg(test)]
    pub fn should_refresh(&mut self) -> bool {
        self.refresh_cause().is_some()
    }

    /// Consume one settled filesystem burst as a local-only refresh cause.
    ///
    /// The watcher deliberately carries no network policy. The refresh
    /// coordinator decides what each cause is allowed to schedule.
    pub fn refresh_cause(&mut self) -> Option<RefreshCause> {
        self.poll_events();
        self.state.take_cause()
    }
}

fn collect_watch_paths(worktree_paths: &[&Path]) -> Vec<PathBuf> {
    let mut roots = std::collections::BTreeSet::new();
    for path in worktree_paths {
        let dot_git = path.join(".git");
        let git_dir = if dot_git.is_dir() {
            dot_git
        } else {
            let Ok(content) = std::fs::read_to_string(&dot_git) else {
                continue;
            };
            let Some(gitdir) = content.strip_prefix("gitdir: ") else {
                continue;
            };
            path.join(gitdir.trim())
        };
        let common = std::fs::read_to_string(git_dir.join("commondir"))
            .map(|common| git_dir.join(common.trim()))
            .unwrap_or(git_dir);
        if let Ok(common) = common.canonicalize() {
            roots.insert(common);
        }
    }
    roots.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    fn wait_until(timeout: Duration, step: Duration, mut predicate: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        loop {
            if predicate() {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(step);
        }
    }

    fn wait_for_events(watcher: &FileWatcher) -> bool {
        wait_until(Duration::from_secs(2), TEST_POLL_INTERVAL, || {
            watcher.drain_events()
        })
    }

    fn wait_for_refresh(watcher: &mut DebouncedWatcher) -> bool {
        wait_until(Duration::from_secs(2), TEST_POLL_INTERVAL, || {
            watcher.should_refresh()
        })
    }

    #[test]
    fn worktree_watcher_ignores_content_and_tracks_shared_and_linked_refs() {
        let root = TempDir::new().unwrap();
        let main = root.path().join("main");
        let linked = root.path().join("linked");
        let repo = git2::Repository::init(&main).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let branch = repo
            .branch("linked", &repo.find_commit(oid).unwrap(), false)
            .unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(branch.get()));
        repo.worktree("linked", &linked, Some(&options)).unwrap();
        let git_dir = main.join(".git");
        let mut watcher = DebouncedWatcher::from_worktree_paths_with_polling(
            &[linked.as_path()],
            Duration::from_millis(30),
            TEST_POLL_INTERVAL,
        )
        .unwrap();
        watcher.should_refresh();
        fs::write(linked.join("source.rs"), "source").unwrap();
        fs::create_dir_all(linked.join("target")).unwrap();
        fs::write(linked.join("target/build"), "output").unwrap();
        fs::write(git_dir.join("objects/ignored-object"), "object").unwrap();
        fs::write(git_dir.join("logs/HEAD"), "reflog").unwrap();
        fs::write(git_dir.join("worktrees/linked/index"), "index").unwrap();
        fs::write(git_dir.join("worktrees/linked/index.lock"), "lock").unwrap();
        assert!(
            !wait_until(
                Duration::from_millis(300),
                Duration::from_millis(10),
                || watcher.should_refresh()
            ),
            "content changes must not refresh"
        );
        fs::write(git_dir.join("refs/heads/new-branch"), format!("{oid}\n")).unwrap();
        assert!(
            wait_for_refresh(&mut watcher),
            "linked-only watcher must observe shared refs"
        );
        fs::write(git_dir.join("worktrees/linked/HEAD"), format!("{oid}\n")).unwrap();
        assert!(
            wait_for_refresh(&mut watcher),
            "linked HEAD changes must refresh"
        );
        let added = root.path().join("added");
        repo.worktree("added", &added, None).unwrap();
        assert!(
            wait_for_refresh(&mut watcher),
            "worktree creation must refresh"
        );
        let removal = std::process::Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["worktree", "remove"])
            .arg(&added)
            .output()
            .unwrap();
        assert!(
            removal.status.success(),
            "{}",
            String::from_utf8_lossy(&removal.stderr)
        );
        assert!(
            wait_for_refresh(&mut watcher),
            "worktree removal must refresh"
        );
    }

    #[test]
    fn watcher_detects_file_creation() {
        let dir = TempDir::new().unwrap();
        let watcher = FileWatcher::with_polling(&[dir.path()], TEST_POLL_INTERVAL).unwrap();

        // No events initially
        assert!(
            !watcher.drain_events(),
            "should have no events before any changes"
        );

        // Create a file — should trigger an event
        fs::write(dir.path().join("test.txt"), "hello").unwrap();
        assert!(wait_for_events(&watcher), "should detect file creation");
    }

    #[test]
    fn watcher_detects_file_modification() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("existing.txt");
        fs::write(&file_path, "original").unwrap();

        let watcher = FileWatcher::with_polling(&[dir.path()], TEST_POLL_INTERVAL).unwrap();
        watcher.drain_events();

        // Modify the file
        fs::write(&file_path, "modified").unwrap();

        assert!(wait_for_events(&watcher), "should detect file modification");
    }

    #[test]
    fn watcher_handles_nonexistent_paths() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("does-not-exist");
        // Should not error — nonexistent paths are silently skipped
        let watcher = FileWatcher::new(&[&nonexistent]);
        assert!(
            watcher.is_ok(),
            "should handle nonexistent paths gracefully"
        );
    }

    #[test]
    fn drain_returns_false_when_no_events() {
        let dir = TempDir::new().unwrap();
        let watcher = FileWatcher::with_polling(&[dir.path()], TEST_POLL_INTERVAL).unwrap();
        watcher.drain_events();

        // No changes made — should return false
        assert!(
            !watcher.drain_events(),
            "should return false with no pending events"
        );
    }

    #[test]
    fn debounced_watcher_no_refresh_without_events() {
        let mut state = DebounceState::new(Duration::from_millis(50));
        let start = Instant::now();
        state.poll_at(start + Duration::from_millis(250));
        assert!(!state.take_refresh(), "should not refresh without events");
    }

    #[test]
    fn watcher_uses_a_fixed_150ms_trailing_edge_and_emits_only_watcher_cause() {
        assert_eq!(DEBOUNCE_DURATION, Duration::from_millis(150));
        let mut state = DebounceState::new(DEBOUNCE_DURATION);
        let start = Instant::now();
        state.record_event(start);
        state.poll_at(start + Duration::from_millis(149));
        assert_eq!(state.take_cause(), None);

        state.poll_at(start + Duration::from_millis(150));
        assert_eq!(state.take_cause(), Some(RefreshCause::Watcher));
        assert_eq!(state.take_cause(), None);
    }

    #[test]
    fn debounced_watcher_no_refresh_during_debounce_window() {
        let mut state = DebounceState::new(Duration::from_millis(300));
        let start = Instant::now();
        state.record_event(start);
        state.poll_at(start + Duration::from_millis(100));
        assert!(
            !state.take_refresh(),
            "should not refresh during debounce window"
        );
    }

    #[test]
    fn debounced_watcher_refreshes_after_debounce_window() {
        let mut state = DebounceState::new(Duration::from_millis(100));
        let start = Instant::now();
        state.record_event(start);
        state.poll_at(start + Duration::from_millis(150));
        assert!(
            state.take_refresh(),
            "should refresh after debounce window expires"
        );
    }

    #[test]
    fn debounced_watcher_resets_on_new_events() {
        let mut state = DebounceState::new(Duration::from_millis(200));
        let start = Instant::now();
        state.record_event(start);
        state.poll_at(start + Duration::from_millis(150));
        state.record_event(start + Duration::from_millis(150));
        state.poll_at(start + Duration::from_millis(300));
        assert!(!state.take_refresh(), "debounce should reset on new events");
        state.poll_at(start + Duration::from_millis(400));
        assert!(state.take_refresh(), "should refresh after events settle");
    }

    #[test]
    fn debounced_watcher_clears_after_refresh() {
        let mut state = DebounceState::new(Duration::from_millis(50));
        let start = Instant::now();
        state.record_event(start);
        state.poll_at(start + Duration::from_millis(100));
        assert!(state.take_refresh(), "first call should return true");
        assert!(
            !state.take_refresh(),
            "second call should return false (cleared)"
        );
    }

    #[test]
    fn watcher_detects_git_ref_changes() {
        let dir = TempDir::new().unwrap();

        // Initialize a git repo
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }

        let git_dir = dir.path().join(".git");
        let watcher = FileWatcher::with_polling(&[&git_dir], TEST_POLL_INTERVAL).unwrap();
        watcher.drain_events();

        // Simulate a ref change (like after a fetch or commit)
        let refs_dir = git_dir.join("refs").join("heads");
        fs::write(refs_dir.join("test-branch"), "fake-sha\n").unwrap();

        assert!(wait_for_events(&watcher), "should detect git ref changes");
    }

    #[test]
    fn watcher_detects_head_change() {
        let dir = TempDir::new().unwrap();
        let _repo = git2::Repository::init(dir.path()).unwrap();

        let git_dir = dir.path().join(".git");
        let watcher = FileWatcher::with_polling(&[&git_dir], TEST_POLL_INTERVAL).unwrap();
        watcher.drain_events();

        // Simulate HEAD change (like switching branches)
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/other-branch\n").unwrap();

        assert!(wait_for_events(&watcher), "should detect HEAD changes");
    }

    #[test]
    fn debounced_watcher_from_worktree_paths_discovers_git_dirs() {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }

        // Use the constructor that takes worktree paths and auto-discovers .git dirs
        let mut dw =
            DebouncedWatcher::from_worktree_paths(&[dir.path()], Duration::from_millis(50))
                .unwrap();
        dw.should_refresh();

        // Simulate a ref change inside .git
        let refs_dir = dir.path().join(".git").join("refs").join("heads");
        fs::write(refs_dir.join("new-branch"), "deadbeef\n").unwrap();

        assert!(
            wait_for_refresh(&mut dw),
            "should auto-discover and watch .git directory"
        );
    }

    #[test]
    fn watcher_continues_after_error_events() {
        // FileWatcher should log (or ignore) errors from notify without crashing.
        // We simulate this by watching a directory, triggering events, then verifying
        // that the watcher is still functional after errors would have occurred.
        let dir = TempDir::new().unwrap();
        let watcher = FileWatcher::with_polling(&[dir.path()], TEST_POLL_INTERVAL).unwrap();
        watcher.drain_events();

        // Create and immediately delete a file — may cause notify errors on some
        // platforms when trying to stat a deleted file
        let file = dir.path().join("ephemeral.txt");
        fs::write(&file, "temp").unwrap();
        fs::remove_file(&file).unwrap();
        let _ = wait_for_events(&watcher);

        // Watcher should still be functional after potential errors
        fs::write(dir.path().join("after.txt"), "still works").unwrap();
        assert!(
            wait_for_events(&watcher),
            "watcher should continue after error events"
        );
    }

    #[test]
    fn debounced_watcher_continues_after_watch_dir_removed() {
        let dir = TempDir::new().unwrap();
        let subdir = dir.path().join("watched");
        fs::create_dir(&subdir).unwrap();

        let mut dw =
            DebouncedWatcher::with_debounce(&[dir.path()], Duration::from_millis(50)).unwrap();

        dw.should_refresh();

        // Remove the subdirectory — watcher should not crash
        fs::remove_dir(&subdir).unwrap();

        // should_refresh should not panic — just returns bool
        let _ = dw.should_refresh();

        // Watcher should still detect changes in the root dir
        fs::write(dir.path().join("new.txt"), "data").unwrap();
        assert!(
            wait_for_refresh(&mut dw),
            "debounced watcher should continue after watched dir removed"
        );
    }

    #[test]
    fn watcher_watches_multiple_directories() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let watcher =
            FileWatcher::with_polling(&[dir1.path(), dir2.path()], TEST_POLL_INTERVAL).unwrap();
        watcher.drain_events();

        // Change in dir2
        fs::write(dir2.path().join("file.txt"), "data").unwrap();
        assert!(
            wait_for_events(&watcher),
            "should detect changes in second directory"
        );
    }

    #[test]
    fn from_worktree_paths_resolves_relative_gitdir() {
        // Simulate a worktree where .git is a file pointing to a relative gitdir path
        let dir = TempDir::new().unwrap();
        let worktree_dir = dir.path().join("my-worktree");
        fs::create_dir(&worktree_dir).unwrap();

        // Create the real git dir at a sibling path (relative from worktree: ../real-git-dir)
        let real_git_dir = dir.path().join("real-git-dir");
        fs::create_dir(&real_git_dir).unwrap();

        // Write a .git file with a relative gitdir path
        let dot_git_file = worktree_dir.join(".git");
        fs::write(&dot_git_file, "gitdir: ../real-git-dir\n").unwrap();

        let mut dw = DebouncedWatcher::from_worktree_paths_with_polling(
            &[worktree_dir.as_path()],
            Duration::from_millis(50),
            TEST_POLL_INTERVAL,
        )
        .unwrap();

        dw.should_refresh();

        // Write a file inside the real git dir — should be detected if path was resolved
        fs::write(real_git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        assert!(
            wait_for_refresh(&mut dw),
            "should resolve relative gitdir path and watch it"
        );
    }

    #[test]
    fn watcher_succeeds_with_mix_of_good_and_bad_paths() {
        let dir = TempDir::new().unwrap();
        let bad_path = std::path::PathBuf::from("/nonexistent/path/that/does/not/exist");

        // Should succeed even though one path is unwatchable
        let watcher =
            FileWatcher::with_polling(&[dir.path(), bad_path.as_path()], TEST_POLL_INTERVAL);
        assert!(
            watcher.is_ok(),
            "should succeed with at least one good path"
        );

        let watcher = watcher.unwrap();

        // Should still detect events on the valid path
        fs::write(dir.path().join("test.txt"), "hello").unwrap();
        assert!(
            wait_for_events(&watcher),
            "should detect changes on valid watched path"
        );
    }

    #[test]
    #[serial]
    fn production_watcher_diagnostics_exclude_path_and_error_details() {
        let state = TempDir::new().unwrap();
        let log_path = state.path().join("trench.log");
        crate::logging::init_at_path_for_test(&log_path);
        let good_dir = TempDir::new().unwrap();
        let path_canary = "credential-path-canary";
        let bad_path = state.path().join(path_canary);

        let _watcher =
            FileWatcher::with_polling(&[good_dir.path(), bad_path.as_path()], TEST_POLL_INTERVAL)
                .unwrap();
        let (tx, _rx) = mpsc::channel();
        let error_canary = "token-error-canary";
        handle_watch_event(Err(notify::Error::generic(error_canary)), &tx, None);

        let contents = std::fs::read_to_string(log_path).unwrap();

        assert!(contents
            .contains("level=warn operation=watch stage=register duration_ms=0 error=not_found"));
        assert!(
            contents.contains("level=warn operation=watch stage=observe duration_ms=0 error=io")
        );
        assert!(!contents.contains(path_canary));
        assert!(!contents.contains(error_canary));
    }
}
