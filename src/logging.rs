use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use crate::paths;

const DEFAULT_FILTER: &str = "warn";
const MAX_FILE_BYTES: u64 = 1024 * 1024;

const ENV_FILTER_VAR: &str = "TRENCH_LOG";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiagnosticFilter {
    Debug,
    #[default]
    Warn,
}

impl DiagnosticFilter {
    fn from_env_value(value: Option<&str>) -> Self {
        match value {
            Some(value) if value.eq_ignore_ascii_case("debug") => Self::Debug,
            _ => Self::Warn,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    List,
    Create,
    Switch,
    Open,
    Sync,
    Remove,
    Tui,
}

impl Operation {
    fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Create => "create",
            Self::Switch => "switch",
            Self::Open => "open",
            Self::Sync => "sync",
            Self::Remove => "remove",
            Self::Tui => "tui",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Resolve,
    Validate,
    Hook,
    Git,
    Render,
    Complete,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::Validate => "validate",
            Self::Hook => "hook",
            Self::Git => "git",
            Self::Render => "render",
            Self::Complete => "complete",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticError {
    Io,
    PermissionDenied,
    NotFound,
    InvalidInput,
    Git,
    Hook,
    Config,
    Internal,
}

impl DiagnosticError {
    fn as_str(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::PermissionDenied => "permission_denied",
            Self::NotFound => "not_found",
            Self::InvalidInput => "invalid_input",
            Self::Git => "git",
            Self::Hook => "hook",
            Self::Config => "config",
            Self::Internal => "internal",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticLevel {
    Debug,
    Warn,
    Error,
}

impl DiagnosticLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticEvent {
    level: DiagnosticLevel,
    operation: Operation,
    stage: Stage,
    duration: Duration,
    error: Option<DiagnosticError>,
}

impl DiagnosticEvent {
    pub fn debug(operation: Operation, stage: Stage, duration: Duration) -> Self {
        Self {
            level: DiagnosticLevel::Debug,
            operation,
            stage,
            duration,
            error: None,
        }
    }

    pub fn warning(
        operation: Operation,
        stage: Stage,
        duration: Duration,
        error: DiagnosticError,
    ) -> Self {
        Self {
            level: DiagnosticLevel::Warn,
            operation,
            stage,
            duration,
            error: Some(error),
        }
    }
}

pub struct Diagnostics {
    path: PathBuf,
    filter: DiagnosticFilter,
    write_lock: Mutex<()>,
}

impl Diagnostics {
    pub fn at_path(path: &Path, filter: DiagnosticFilter) -> Self {
        Self {
            path: path.to_path_buf(),
            filter,
            write_lock: Mutex::new(()),
        }
    }

    pub fn record(&self, event: DiagnosticEvent) {
        if self.filter == DiagnosticFilter::Warn && event.level == DiagnosticLevel::Debug {
            return;
        }

        let Ok(_guard) = self.write_lock.lock() else {
            return;
        };
        self.rotate_if_full();
        let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        else {
            return;
        };
        let error = event.error.map(|error| error.as_str()).unwrap_or("none");
        let _ = writeln!(
            file,
            "level={} operation={} stage={} duration_ms={} error={}",
            event.level.as_str(),
            event.operation.as_str(),
            event.stage.as_str(),
            event.duration.as_millis(),
            error
        );
    }

    fn rotate_if_full(&self) {
        let Ok(metadata) = std::fs::metadata(&self.path) else {
            return;
        };
        if metadata.len() < MAX_FILE_BYTES {
            return;
        }

        let rotated = PathBuf::from(format!("{}.1", self.path.display()));
        let _ = std::fs::remove_file(&rotated);
        let _ = std::fs::rename(&self.path, rotated);
    }
}

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
fn build_subscriber<W: Write + Send + 'static>(
    writer: W,
) -> impl tracing::Subscriber + Send + Sync {
    let filter =
        EnvFilter::try_from_env(ENV_FILTER_VAR).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    build_subscriber_with_filter(writer, filter)
}

/// Initialize the tracing subscriber with file-based logging.
///
/// Writes logs to trench's state directory as resolved by [`crate::paths`].
/// Linux and macOS default to XDG-style state paths; Windows defaults to the
/// native state directory unless `XDG_STATE_HOME` is set.
pub fn init() -> Result<()> {
    match paths::state_dir()
        .and_then(|_| paths::log_file_path())
        .and_then(|path| init_with_log_path(&path))
    {
        Ok(()) => Ok(()),
        Err(_) => {
            let subscriber = build_subscriber(std::io::sink());
            let _ = tracing::subscriber::set_global_default(subscriber);
            Ok(())
        }
    }
}

fn init_with_log_path(log_path: &std::path::Path) -> Result<()> {
    let file = File::options()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("failed to open log file: {}", log_path.display()))?;

    let subscriber = build_subscriber(file);

    // May fail if a global subscriber is already set — that's OK, first one wins.
    let _ = tracing::subscriber::set_global_default(subscriber);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn default_filter_records_warnings_but_not_debug_events() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("trench.log");
        let diagnostics = Diagnostics::at_path(&log_path, DiagnosticFilter::default());

        diagnostics.record(DiagnosticEvent::debug(
            Operation::List,
            Stage::Git,
            std::time::Duration::from_millis(7),
        ));
        diagnostics.record(DiagnosticEvent::warning(
            Operation::List,
            Stage::Git,
            std::time::Duration::from_millis(11),
            DiagnosticError::Io,
        ));

        let contents = std::fs::read_to_string(log_path).unwrap();
        assert!(!contents.contains("duration_ms=7"));
        assert!(contents.contains("level=warn operation=list stage=git duration_ms=11 error=io"));
    }

    #[test]
    fn trench_log_debug_enables_diagnostic_detail() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("trench.log");
        let diagnostics =
            Diagnostics::at_path(&log_path, DiagnosticFilter::from_env_value(Some("debug")));

        diagnostics.record(DiagnosticEvent::debug(
            Operation::Create,
            Stage::Validate,
            std::time::Duration::from_millis(3),
        ));

        let contents = std::fs::read_to_string(log_path).unwrap();
        assert!(contents
            .contains("level=debug operation=create stage=validate duration_ms=3 error=none"));
    }

    #[test]
    fn rotates_the_active_file_at_one_mibibyte() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("trench.log");
        std::fs::write(&log_path, vec![b'x'; 1024 * 1024]).unwrap();
        let diagnostics = Diagnostics::at_path(&log_path, DiagnosticFilter::default());

        diagnostics.record(DiagnosticEvent::warning(
            Operation::Sync,
            Stage::Git,
            std::time::Duration::from_millis(29),
            DiagnosticError::Git,
        ));

        assert_eq!(
            std::fs::metadata(dir.path().join("trench.log.1"))
                .unwrap()
                .len(),
            1024 * 1024
        );
        let active = std::fs::read_to_string(log_path).unwrap();
        assert_eq!(
            active,
            "level=warn operation=sync stage=git duration_ms=29 error=git\n"
        );
    }

    #[test]
    fn init_creates_log_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("trench.log");

        assert!(!log_path.exists(), "log file should not exist before init");

        // init_with_log_path may fail to set the global subscriber (parallel tests),
        // but the log file should still be created.
        let _ = init_with_log_path(&log_path);

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
