use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use crate::paths;

const LOG_FILENAME: &str = "trench.log";
const DEFAULT_FILTER: &str = "warn";

const ENV_FILTER_VAR: &str = "TRENCH_LOG";

/// Build a tracing subscriber with a specific filter, writing to the given writer.
fn build_subscriber_with_filter<W: Write + Send + 'static>(
    writer: W,
    filter: EnvFilter,
) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(writer))
        .with_ansi(false)
        .with_env_filter(filter)
        .finish()
}

/// Build a tracing subscriber that writes to the given writer.
///
/// Uses `TRENCH_LOG` env var for the filter if set, otherwise defaults to `warn`.
fn build_subscriber<W: Write + Send + 'static>(writer: W) -> impl tracing::Subscriber + Send + Sync {
    let filter = EnvFilter::try_from_env(ENV_FILTER_VAR)
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    build_subscriber_with_filter(writer, filter)
}

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

    let subscriber = build_subscriber(file);

    // May fail if another test already set the global subscriber — that's OK.
    let _ = tracing::subscriber::set_global_default(subscriber);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

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

    #[test]
    fn default_filter_level_is_warn() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("test.log");
        let file = File::options()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap();

        let subscriber = build_subscriber(file);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("this info should be filtered");
            tracing::warn!("this warn should appear");
        });

        let mut contents = String::new();
        File::open(&log_path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();

        assert!(
            !contents.contains("this info should be filtered"),
            "info events should be filtered out at default warn level"
        );
        assert!(
            contents.contains("this warn should appear"),
            "warn events should be logged at default warn level"
        );
    }

    #[test]
    fn custom_filter_overrides_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("test.log");
        let file = File::options()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap();

        let subscriber = build_subscriber_with_filter(file, EnvFilter::new("debug"));

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!("this debug should appear");
        });

        let mut contents = String::new();
        File::open(&log_path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();

        assert!(
            contents.contains("this debug should appear"),
            "debug events should be logged when filter is set to debug"
        );
    }
}
