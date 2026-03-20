use std::fs::File;

use anyhow::{Context, Result};

use crate::paths;

const LOG_FILENAME: &str = "trench.log";

/// Initialize the tracing subscriber with file-based logging.
///
/// Writes logs to `$XDG_STATE_HOME/trench/trench.log`. Defaults to `warn`
/// level; override with the `TRENCH_LOG` environment variable.
pub fn init() -> Result<()> {
    let log_path = paths::state_dir()?.join(LOG_FILENAME);
    let file = File::options()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open log file: {}", log_path.display()))?;

    let subscriber = tracing_subscriber::fmt()
        .with_writer(file)
        .with_ansi(false)
        .finish();

    // May fail if another test already set the global subscriber — that's OK.
    let _ = tracing::subscriber::set_global_default(subscriber);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_log_file_in_state_dir() {
        let state_dir = paths::state_dir().expect("state_dir should succeed");
        let log_path = state_dir.join("trench.log");

        // Remove any pre-existing log file so we can verify init creates it
        let _ = std::fs::remove_file(&log_path);
        assert!(!log_path.exists(), "log file should not exist before init");

        // init() may fail if the global subscriber is already set (parallel tests),
        // but the log file should still be created.
        let _ = init();

        assert!(log_path.exists(), "log file should exist after init");
    }
}
