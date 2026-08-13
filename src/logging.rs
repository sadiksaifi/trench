use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::paths;

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const ROTATED_FILE_COUNT: usize = 4;
const ENV_FILTER_VAR: &str = "TRENCH_LOG";

static DIAGNOSTICS: OnceLock<Diagnostics> = OnceLock::new();

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

    pub fn error(
        operation: Operation,
        stage: Stage,
        duration: Duration,
        error: DiagnosticError,
    ) -> Self {
        Self {
            level: DiagnosticLevel::Error,
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

    fn prepare(&self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path);
    }

    pub fn record(&self, event: DiagnosticEvent) {
        if self.filter == DiagnosticFilter::Warn && event.level == DiagnosticLevel::Debug {
            return;
        }

        let Ok(_guard) = self.write_lock.lock() else {
            return;
        };
        self.prepare();
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

        self.remove_excess_rotated_files();
        for index in (1..ROTATED_FILE_COUNT).rev() {
            let source = self.rotated_path(index);
            let destination = self.rotated_path(index + 1);
            let _ = std::fs::remove_file(&destination);
            let _ = std::fs::rename(source, destination);
        }
        let _ = std::fs::rename(&self.path, self.rotated_path(1));
    }

    fn remove_excess_rotated_files(&self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        let Some(file_name) = self.path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let prefix = format!("{file_name}.");
        let Ok(entries) = std::fs::read_dir(parent) else {
            return;
        };

        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(index) = name
                .to_str()
                .and_then(|name| name.strip_prefix(&prefix))
                .and_then(|suffix| suffix.parse::<usize>().ok())
            else {
                continue;
            };
            if index >= ROTATED_FILE_COUNT {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    fn rotated_path(&self, index: usize) -> PathBuf {
        let mut path = self.path.as_os_str().to_os_string();
        path.push(format!(".{index}"));
        PathBuf::from(path)
    }
}

/// Initialize bounded file diagnostics without exposing a general-purpose log sink.
///
/// Path resolution, directory creation, file opening, and installation are all
/// best effort: diagnostics can never change product behavior or exit status.
pub fn init() {
    let Ok(path) = paths::log_file_path() else {
        return;
    };
    let env_filter = std::env::var(ENV_FILTER_VAR).ok();
    let diagnostics = Diagnostics::at_path(
        &path,
        DiagnosticFilter::from_env_value(env_filter.as_deref()),
    );
    diagnostics.prepare();
    let _ = DIAGNOSTICS.set(diagnostics);
}

/// Record a typed diagnostic event when diagnostics initialized successfully.
pub fn record(event: DiagnosticEvent) {
    if let Some(diagnostics) = DIAGNOSTICS.get() {
        diagnostics.record(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn retains_only_the_active_file_and_four_rotated_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("trench.log");
        std::fs::write(&log_path, vec![b'x'; 1024 * 1024]).unwrap();
        for index in 1..=5 {
            std::fs::write(
                dir.path().join(format!("trench.log.{index}")),
                format!("old-{index}"),
            )
            .unwrap();
        }
        let diagnostics = Diagnostics::at_path(&log_path, DiagnosticFilter::default());

        diagnostics.record(DiagnosticEvent::warning(
            Operation::Remove,
            Stage::Complete,
            std::time::Duration::from_millis(31),
            DiagnosticError::Io,
        ));

        let mut names = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            [
                "trench.log",
                "trench.log.1",
                "trench.log.2",
                "trench.log.3",
                "trench.log.4",
            ]
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("trench.log.1")).unwrap(),
            "x".repeat(1024 * 1024)
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("trench.log.4")).unwrap(),
            "old-3"
        );
    }

    #[test]
    fn typed_events_never_persist_secret_bearing_inputs() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("trench.log");
        let diagnostics = Diagnostics::at_path(&log_path, DiagnosticFilter::Debug);
        let forbidden_inputs = [
            "hook-stdout-canary",
            "hook-stderr-canary",
            "HOOK_TOKEN=environment-canary",
            "--password=argument-canary",
            "editor --credential configured-command-canary",
            "remote rejected token=error-canary",
        ];

        diagnostics.record(DiagnosticEvent::error(
            Operation::Create,
            Stage::Hook,
            std::time::Duration::from_millis(41),
            DiagnosticError::Hook,
        ));

        let contents = std::fs::read_to_string(log_path).unwrap();
        assert_eq!(
            contents,
            "level=error operation=create stage=hook duration_ms=41 error=hook\n"
        );
        for canary in forbidden_inputs {
            assert!(!contents.contains(canary));
        }
    }

    #[test]
    fn missing_parent_directories_are_created_best_effort() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("missing/state/trench/trench.log");
        let diagnostics = Diagnostics::at_path(&log_path, DiagnosticFilter::default());

        diagnostics.record(DiagnosticEvent::warning(
            Operation::Open,
            Stage::Resolve,
            std::time::Duration::from_millis(5),
            DiagnosticError::NotFound,
        ));

        assert_eq!(
            std::fs::read_to_string(log_path).unwrap(),
            "level=warn operation=open stage=resolve duration_ms=5 error=not_found\n"
        );
    }

    #[test]
    fn init_creates_log_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("trench.log");

        assert!(!log_path.exists(), "log file should not exist before init");

        Diagnostics::at_path(&log_path, DiagnosticFilter::default()).prepare();

        assert!(log_path.exists(), "log file should exist after init");
    }
}
