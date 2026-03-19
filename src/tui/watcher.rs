use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{RecommendedWatcher, Watcher};

/// Default debounce window: 500ms of quiet before triggering a refresh.
const DEBOUNCE_DURATION: Duration = Duration::from_millis(500);

/// Watches filesystem paths for changes and signals when a TUI refresh is needed.
///
/// Uses the `notify` crate to monitor worktree directories and `.git` directories
/// for changes. Events are coalesced — callers should debounce before refreshing.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    event_rx: mpsc::Receiver<()>,
}

impl FileWatcher {
    /// Create a new FileWatcher monitoring the given paths.
    ///
    /// Each path is watched recursively. A non-blocking `event_rx` signals
    /// when changes are detected. Returns an error if the watcher cannot
    /// be initialized.
    pub fn new(paths: &[&Path]) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let event_tx = tx;
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                let _ = event_tx.send(());
            }
        })?;

        for path in paths {
            if path.exists() {
                watcher.watch(path, notify::RecursiveMode::Recursive)?;
            }
        }

        Ok(Self {
            _watcher: watcher,
            event_rx: rx,
        })
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
pub struct DebouncedWatcher {
    inner: FileWatcher,
    last_event: Option<Instant>,
    debounce: Duration,
}

impl DebouncedWatcher {
    /// Create a debounced watcher monitoring the given paths.
    pub fn new(paths: &[&Path]) -> Result<Self> {
        Ok(Self {
            inner: FileWatcher::new(paths)?,
            last_event: None,
            debounce: DEBOUNCE_DURATION,
        })
    }

    /// Create with a custom debounce duration (for testing).
    #[cfg(test)]
    pub fn with_debounce(paths: &[&Path], debounce: Duration) -> Result<Self> {
        Ok(Self {
            inner: FileWatcher::new(paths)?,
            last_event: None,
            debounce,
        })
    }

    /// Poll for events and check if debounce period has elapsed.
    ///
    /// Call this every frame in the TUI event loop. Returns `true` when
    /// the debounce window has expired after events were detected,
    /// indicating the TUI should refresh its data.
    pub fn should_refresh(&mut self) -> bool {
        if self.inner.drain_events() {
            self.last_event = Some(Instant::now());
        }

        if let Some(last) = self.last_event {
            if last.elapsed() >= self.debounce {
                self.last_event = None;
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn watcher_detects_file_creation() {
        let dir = TempDir::new().unwrap();
        let watcher = FileWatcher::new(&[dir.path()]).unwrap();

        // No events initially
        assert!(!watcher.drain_events(), "should have no events before any changes");

        // Create a file — should trigger an event
        fs::write(dir.path().join("test.txt"), "hello").unwrap();

        // Give the watcher time to deliver the event
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert!(watcher.drain_events(), "should detect file creation");
    }

    #[test]
    fn watcher_detects_file_modification() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("existing.txt");
        fs::write(&file_path, "original").unwrap();

        // Small delay to ensure watcher doesn't pick up the initial write
        std::thread::sleep(std::time::Duration::from_millis(100));

        let watcher = FileWatcher::new(&[dir.path()]).unwrap();

        // Drain any spurious startup events
        std::thread::sleep(std::time::Duration::from_millis(100));
        watcher.drain_events();

        // Modify the file
        fs::write(&file_path, "modified").unwrap();

        std::thread::sleep(std::time::Duration::from_millis(200));

        assert!(watcher.drain_events(), "should detect file modification");
    }

    #[test]
    fn watcher_handles_nonexistent_paths() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("does-not-exist");
        // Should not error — nonexistent paths are silently skipped
        let watcher = FileWatcher::new(&[&nonexistent]);
        assert!(watcher.is_ok(), "should handle nonexistent paths gracefully");
    }

    #[test]
    fn drain_returns_false_when_no_events() {
        let dir = TempDir::new().unwrap();
        let watcher = FileWatcher::new(&[dir.path()]).unwrap();

        // Drain any startup noise
        std::thread::sleep(std::time::Duration::from_millis(100));
        watcher.drain_events();

        // No changes made — should return false
        assert!(!watcher.drain_events(), "should return false with no pending events");
    }

    #[test]
    fn debounced_watcher_no_refresh_without_events() {
        let dir = TempDir::new().unwrap();
        let mut dw = DebouncedWatcher::with_debounce(
            &[dir.path()],
            Duration::from_millis(50),
        )
        .unwrap();

        // Drain any startup noise
        std::thread::sleep(Duration::from_millis(100));
        dw.should_refresh();
        std::thread::sleep(Duration::from_millis(100));

        assert!(!dw.should_refresh(), "should not refresh without events");
    }

    #[test]
    fn debounced_watcher_no_refresh_during_debounce_window() {
        let dir = TempDir::new().unwrap();
        let mut dw = DebouncedWatcher::with_debounce(
            &[dir.path()],
            Duration::from_millis(300),
        )
        .unwrap();

        // Drain startup noise
        std::thread::sleep(Duration::from_millis(100));
        dw.should_refresh();

        // Create a file
        fs::write(dir.path().join("test.txt"), "hello").unwrap();
        std::thread::sleep(Duration::from_millis(100));

        // Event received but debounce window (300ms) not yet elapsed
        assert!(!dw.should_refresh(), "should not refresh during debounce window");
    }

    #[test]
    fn debounced_watcher_refreshes_after_debounce_window() {
        let dir = TempDir::new().unwrap();
        let mut dw = DebouncedWatcher::with_debounce(
            &[dir.path()],
            Duration::from_millis(100),
        )
        .unwrap();

        // Drain startup noise
        std::thread::sleep(Duration::from_millis(100));
        dw.should_refresh();

        // Create a file
        fs::write(dir.path().join("test.txt"), "hello").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        dw.should_refresh(); // picks up event, starts debounce

        // Wait for debounce to expire
        std::thread::sleep(Duration::from_millis(150));

        assert!(dw.should_refresh(), "should refresh after debounce window expires");
    }

    #[test]
    fn debounced_watcher_resets_on_new_events() {
        let dir = TempDir::new().unwrap();
        let mut dw = DebouncedWatcher::with_debounce(
            &[dir.path()],
            Duration::from_millis(200),
        )
        .unwrap();

        // Drain startup noise
        std::thread::sleep(Duration::from_millis(100));
        dw.should_refresh();

        // First event
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        dw.should_refresh(); // picks up event

        // Second event before debounce expires — should reset timer
        fs::write(dir.path().join("b.txt"), "b").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!dw.should_refresh(), "debounce should reset on new events");

        // Wait for debounce to fully expire after the last event
        std::thread::sleep(Duration::from_millis(250));
        assert!(dw.should_refresh(), "should refresh after events settle");
    }

    #[test]
    fn debounced_watcher_clears_after_refresh() {
        let dir = TempDir::new().unwrap();
        let mut dw = DebouncedWatcher::with_debounce(
            &[dir.path()],
            Duration::from_millis(50),
        )
        .unwrap();

        // Drain startup noise
        std::thread::sleep(Duration::from_millis(100));
        dw.should_refresh();

        // Trigger event and let debounce expire
        fs::write(dir.path().join("test.txt"), "hello").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        dw.should_refresh(); // picks up event
        std::thread::sleep(Duration::from_millis(100));

        assert!(dw.should_refresh(), "first call should return true");
        assert!(!dw.should_refresh(), "second call should return false (cleared)");
    }

    #[test]
    fn watcher_watches_multiple_directories() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let watcher = FileWatcher::new(&[dir1.path(), dir2.path()]).unwrap();

        // Drain startup noise
        std::thread::sleep(std::time::Duration::from_millis(100));
        watcher.drain_events();

        // Change in dir2
        fs::write(dir2.path().join("file.txt"), "data").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert!(watcher.drain_events(), "should detect changes in second directory");
    }
}
