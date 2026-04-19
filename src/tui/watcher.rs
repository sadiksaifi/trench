use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{Config, Event, PollWatcher, RecommendedWatcher, Watcher};

/// Default debounce window: 500ms of quiet before triggering a refresh.
pub const DEBOUNCE_DURATION: Duration = Duration::from_millis(500);
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
fn handle_watch_event(res: notify::Result<Event>, tx: &mpsc::Sender<()>) {
    match res {
        Ok(_) => {
            let _ = tx.send(());
        }
        Err(err) => {
            tracing::warn!("file watch error: {err}");
        }
    }
}

/// Watches filesystem paths for changes and signals when a TUI refresh is needed.
///
/// Uses the `notify` crate to monitor worktree directories and `.git` directories
/// for changes. Events are coalesced — callers should debounce before refreshing.
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
    pub fn new(paths: &[&Path]) -> Result<Self> {
        match Self::new_with_backend(paths, WatchBackend::Recommended) {
            Ok(watcher) => Ok(watcher),
            Err(native_err) => {
                tracing::warn!(
                    "native watcher unavailable, falling back to polling: {native_err:#}"
                );
                Self::new_with_backend(
                    paths,
                    WatchBackend::Poll {
                        interval: FALLBACK_POLL_INTERVAL,
                        compare_contents: true,
                    },
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
        )
    }

    fn new_with_backend(paths: &[&Path], backend: WatchBackend) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = Self::build_watcher(backend, tx)?;

        let mut watched = 0;
        let mut watch_errors = 0;
        for path in paths {
            if path.exists() {
                match watcher.watch(path, notify::RecursiveMode::Recursive) {
                    Ok(()) => watched += 1,
                    Err(err) => {
                        tracing::warn!(path = %path.display(), "failed to watch path: {err}");
                        watch_errors += 1;
                    }
                }
            } else {
                tracing::warn!(path = %path.display(), "path does not exist, skipping");
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

    fn build_watcher(backend: WatchBackend, tx: mpsc::Sender<()>) -> notify::Result<ActiveWatcher> {
        match backend {
            WatchBackend::Recommended => {
                let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
                    handle_watch_event(res, &tx);
                })?;
                Ok(ActiveWatcher::Recommended(watcher))
            }
            WatchBackend::Poll {
                interval,
                compare_contents,
            } => {
                let watcher = PollWatcher::new(
                    move |res: notify::Result<Event>| {
                        handle_watch_event(res, &tx);
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
/// After the first filesystem event, waits until `DEBOUNCE_DURATION` (500ms)
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

    #[cfg(test)]
    fn has_pending_refresh(&self) -> bool {
        self.pending_refresh
    }
}

pub struct DebouncedWatcher {
    inner: FileWatcher,
    state: DebounceState,
}

impl DebouncedWatcher {
    /// Create a debounced watcher monitoring the given paths.
    pub fn new(paths: &[&Path]) -> Result<Self> {
        Self::from_file_watcher(FileWatcher::new(paths)?, DEBOUNCE_DURATION)
    }

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
    /// For each worktree path, also watches the `.git` directory (if it exists)
    /// so that ref changes (commits, fetches, branch switches) trigger refresh.
    pub fn from_worktree_paths(worktree_paths: &[&Path], debounce: Duration) -> Result<Self> {
        let all_paths = collect_watch_paths(worktree_paths);
        let path_refs: Vec<&Path> = all_paths.iter().map(|p| p.as_path()).collect();
        Self::from_file_watcher(FileWatcher::new(&path_refs)?, debounce)
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
            FileWatcher::with_polling(&path_refs, poll_interval)?,
            debounce,
        )
    }

    fn from_file_watcher(inner: FileWatcher, debounce: Duration) -> Result<Self> {
        Ok(Self {
            inner,
            state: DebounceState::new(debounce),
        })
    }

    #[cfg(test)]
    pub fn has_pending_refresh(&self) -> bool {
        self.state.has_pending_refresh()
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
    pub fn should_refresh(&mut self) -> bool {
        self.poll_events();
        self.state.take_refresh()
    }
}

fn collect_watch_paths(worktree_paths: &[&Path]) -> Vec<std::path::PathBuf> {
    let mut all_paths: Vec<std::path::PathBuf> = Vec::new();

    for path in worktree_paths {
        all_paths.push(path.to_path_buf());

        let git_dir = path.join(".git");
        if git_dir.is_dir() {
            all_paths.push(git_dir);
        } else if git_dir.is_file() {
            if let Ok(content) = std::fs::read_to_string(&git_dir) {
                if let Some(gitdir) = content.strip_prefix("gitdir: ") {
                    let gitdir_path = Path::new(gitdir.trim());
                    let real_git_dir = if gitdir_path.is_relative() {
                        path.join(gitdir_path)
                    } else {
                        gitdir_path.to_path_buf()
                    };
                    if real_git_dir.is_dir() {
                        all_paths.push(real_git_dir);
                    }
                }
            }
        }
    }

    all_paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use std::io;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    #[derive(Clone, Default)]
    struct SharedLogBuffer {
        inner: Arc<Mutex<Vec<u8>>>,
    }

    struct SharedLogWriter {
        inner: Arc<Mutex<Vec<u8>>>,
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter {
                inner: Arc::clone(&self.inner),
            }
        }
    }

    impl io::Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.inner.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SharedLogBuffer {
        fn contents(&self) -> String {
            String::from_utf8(self.inner.lock().unwrap().clone()).unwrap()
        }
    }

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
        let mut dw = DebouncedWatcher::from_worktree_paths_with_polling(
            &[dir.path()],
            Duration::from_millis(50),
            TEST_POLL_INTERVAL,
        )
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
    fn new_logs_warning_for_nonexistent_paths() {
        let log_buffer = SharedLogBuffer::default();

        let subscriber = tracing_subscriber::fmt()
            .with_writer(log_buffer.clone())
            .with_ansi(false)
            .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
            .finish();

        let good_dir = TempDir::new().unwrap();
        let bad_path = std::path::PathBuf::from("/nonexistent/path/that/does/not/exist");

        tracing::subscriber::with_default(subscriber, || {
            let _watcher = FileWatcher::new(&[good_dir.path(), bad_path.as_path()]).unwrap();
        });

        let contents = log_buffer.contents();

        assert!(
            contents.contains("path does not exist, skipping"),
            "should log warning for nonexistent paths: got {contents:?}"
        );
    }

    #[test]
    #[serial]
    fn handle_watch_event_logs_notify_errors() {
        let log_buffer = SharedLogBuffer::default();

        let subscriber = tracing_subscriber::fmt()
            .with_writer(log_buffer.clone())
            .with_ansi(false)
            .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
            .finish();

        let (tx, _rx) = mpsc::channel();
        let err = notify::Error::generic("synthetic test error");

        tracing::subscriber::with_default(subscriber, || {
            handle_watch_event(Err(err), &tx);
        });

        let contents = log_buffer.contents();

        assert!(
            contents.contains("synthetic test error"),
            "notify errors should be logged: got {contents:?}"
        );
    }
}
