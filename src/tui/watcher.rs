use std::path::Path;
use std::sync::mpsc;

use anyhow::Result;
use notify::{RecommendedWatcher, Watcher};

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
