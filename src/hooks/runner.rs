use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};

use super::copy::execute_copy_step;
use super::run::{execute_run_step, RunStepError};
use super::shell::{execute_shell_step, ShellStepError};
use super::{build_env, HookConfig, HookEnvContext, HookEvent};
use crate::state::Database;

/// Timeout error returned when run + shell steps exceed `timeout_secs`.
#[derive(Debug, thiserror::Error)]
#[error("hook timed out after {timeout_secs}s")]
pub struct HookTimeoutError {
    pub timeout_secs: u64,
}

/// Result of a successful hook execution.
#[derive(Debug)]
pub struct HookResult {
    /// Event id recorded in the database.
    pub event_id: i64,
    /// Total wall-clock duration in seconds.
    pub duration_secs: f64,
}

/// Execute a hook lifecycle event: copy → run → shell.
///
/// - `copy` runs first (not subject to timeout).
/// - `run` and `shell` share the `timeout_secs` budget.
/// - Any step failure stops remaining steps.
/// - All output is captured and logged to the database.
/// - Returns `HookTimeoutError` (exit code 7) on timeout.
pub async fn execute_hook(
    event: &HookEvent,
    config: &HookConfig,
    env_ctx: &HookEnvContext,
    source_dir: &Path,
    work_dir: &Path,
    db: &Database,
    repo_id: i64,
    worktree_id: Option<i64>,
) -> Result<HookResult> {
    let start = Instant::now();
    let env_vars = build_env(env_ctx, event);
    let timeout_secs = config.timeout_secs.unwrap_or(120);

    let mut all_output: Vec<(String, String)> = Vec::new(); // (stream, line)

    // Step 1: Copy (not subject to timeout)
    if let Some(ref patterns) = config.copy {
        if let Err(e) = execute_copy_step(source_dir, work_dir, patterns) {
            let duration = start.elapsed();
            record_execution(
                db, repo_id, worktree_id, event, 1, duration.as_secs_f64(), &all_output,
            )?;
            return Err(e.context("copy step failed"));
        }
    }

    // Step 2: Run (subject to timeout)
    let run_deadline = Instant::now() + std::time::Duration::from_secs(timeout_secs);
    if let Some(ref commands) = config.run {
        let remaining = run_deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, execute_run_step(commands, work_dir, &env_vars))
            .await
        {
            Ok(Ok(run_result)) => {
                for cmd_output in &run_result.executed {
                    collect_output(&mut all_output, &cmd_output.stdout, &cmd_output.stderr);
                }
            }
            Ok(Err(e)) => {
                // Collect partial output from the error if it's a RunStepError
                let exit_code = extract_run_error_output(&e, &mut all_output);
                let duration = start.elapsed();
                record_execution(
                    db, repo_id, worktree_id, event, exit_code, duration.as_secs_f64(), &all_output,
                )?;
                return Err(e);
            }
            Err(_) => {
                let duration = start.elapsed();
                record_execution(
                    db, repo_id, worktree_id, event, 7, duration.as_secs_f64(), &all_output,
                )?;
                return Err(HookTimeoutError { timeout_secs }.into());
            }
        }
    }

    // Step 3: Shell (remaining timeout budget)
    if let Some(ref script) = config.shell {
        let remaining = run_deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, execute_shell_step(script, work_dir, &env_vars))
            .await
        {
            Ok(Ok(shell_output)) => {
                collect_output(&mut all_output, &shell_output.stdout, &shell_output.stderr);
            }
            Ok(Err(e)) => {
                let exit_code = extract_shell_error_output(&e, &mut all_output);
                let duration = start.elapsed();
                record_execution(
                    db, repo_id, worktree_id, event, exit_code, duration.as_secs_f64(), &all_output,
                )?;
                return Err(e);
            }
            Err(_) => {
                let duration = start.elapsed();
                record_execution(
                    db, repo_id, worktree_id, event, 7, duration.as_secs_f64(), &all_output,
                )?;
                return Err(HookTimeoutError { timeout_secs }.into());
            }
        }
    }

    let duration = start.elapsed();
    let event_id = record_execution(
        db, repo_id, worktree_id, event, 0, duration.as_secs_f64(), &all_output,
    )?;

    Ok(HookResult {
        event_id,
        duration_secs: duration.as_secs_f64(),
    })
}

/// Extract partial output from a RunStepError and return the exit code.
fn extract_run_error_output(
    err: &anyhow::Error,
    all_output: &mut Vec<(String, String)>,
) -> i32 {
    if let Some(run_err) = err.downcast_ref::<RunStepError>() {
        for cmd_output in &run_err.results.executed {
            collect_output(all_output, &cmd_output.stdout, &cmd_output.stderr);
        }
        run_err.exit_code
    } else {
        1
    }
}

/// Extract output from a ShellStepError and return the exit code.
fn extract_shell_error_output(
    err: &anyhow::Error,
    all_output: &mut Vec<(String, String)>,
) -> i32 {
    if let Some(shell_err) = err.downcast_ref::<ShellStepError>() {
        collect_output(all_output, &shell_err.output.stdout, &shell_err.output.stderr);
        shell_err.exit_code
    } else {
        1
    }
}

fn collect_output(all_output: &mut Vec<(String, String)>, stdout: &str, stderr: &str) {
    for line in stdout.lines() {
        all_output.push(("stdout".to_string(), line.to_string()));
    }
    for line in stderr.lines() {
        all_output.push(("stderr".to_string(), line.to_string()));
    }
}

fn record_execution(
    db: &Database,
    repo_id: i64,
    worktree_id: Option<i64>,
    event: &HookEvent,
    exit_code: i32,
    duration_secs: f64,
    output: &[(String, String)],
) -> Result<i64> {
    let payload = serde_json::json!({
        "hook": event.as_str(),
        "exit_code": exit_code,
        "duration_secs": duration_secs,
    });

    let event_id = db.insert_event(
        repo_id,
        worktree_id,
        &format!("hook:{}", event.as_str()),
        Some(&payload),
    )?;

    for (i, (stream, line)) in output.iter().enumerate() {
        db.insert_log(event_id, stream, line, (i + 1) as i64)?;
    }

    Ok(event_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookDef;
    use tempfile::TempDir;

    fn test_env_ctx(source: &Path, work: &Path) -> HookEnvContext {
        HookEnvContext {
            worktree_path: work.to_string_lossy().into_owned(),
            worktree_name: "test-wt".into(),
            branch: "test-branch".into(),
            repo_name: "test-repo".into(),
            repo_path: source.to_string_lossy().into_owned(),
            base_branch: "main".into(),
        }
    }

    fn setup_db() -> (Database, i64, i64) {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "wt", "branch", "/wt", None)
            .unwrap();
        (db, repo.id, wt.id)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn success_path_executes_copy_run_shell_in_order() {
        let source = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let (db, repo_id, wt_id) = setup_db();

        // Create a file in source for copy step
        std::fs::write(source.path().join(".env"), "SECRET=123").unwrap();

        let config = HookDef {
            copy: Some(vec![".env".to_string()]),
            run: Some(vec!["echo run_output".to_string()]),
            shell: Some("echo shell_output".to_string()),
            timeout_secs: Some(30),
        };

        let env_ctx = test_env_ctx(source.path(), work.path());

        let result = execute_hook(
            &HookEvent::PostCreate,
            &config,
            &env_ctx,
            source.path(),
            work.path(),
            &db,
            repo_id,
            Some(wt_id),
        )
        .await
        .expect("hook should succeed");

        // Verify result
        assert!(result.event_id > 0);
        assert!(result.duration_secs >= 0.0);

        // Verify copy happened
        assert!(work.path().join(".env").exists());
        assert_eq!(
            std::fs::read_to_string(work.path().join(".env")).unwrap(),
            "SECRET=123"
        );

        // Verify event in DB
        let events = db.list_events(wt_id, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "hook:post_create");

        // Verify logs in DB contain output from run + shell
        let logs = db.get_logs(result.event_id).unwrap();
        let stdout_lines: Vec<&str> = logs
            .iter()
            .filter(|(s, _, _)| s == "stdout")
            .map(|(_, l, _)| l.as_str())
            .collect();
        assert!(stdout_lines.contains(&"run_output"));
        assert!(stdout_lines.contains(&"shell_output"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn only_configured_steps_execute() {
        let source = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let (db, repo_id, wt_id) = setup_db();

        // Config with only run — no copy, no shell
        let config = HookDef {
            copy: None,
            run: Some(vec!["echo only_run".to_string()]),
            shell: None,
            timeout_secs: Some(30),
        };

        let env_ctx = test_env_ctx(source.path(), work.path());

        let result = execute_hook(
            &HookEvent::PostCreate,
            &config,
            &env_ctx,
            source.path(),
            work.path(),
            &db,
            repo_id,
            Some(wt_id),
        )
        .await
        .expect("hook should succeed");

        // Only run output should be in logs
        let logs = db.get_logs(result.event_id).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].0, "stdout");
        assert_eq!(logs[0].1, "only_run");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_config_succeeds_with_no_output() {
        let source = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let (db, repo_id, wt_id) = setup_db();

        // All steps None
        let config = HookDef {
            copy: None,
            run: None,
            shell: None,
            timeout_secs: Some(30),
        };

        let env_ctx = test_env_ctx(source.path(), work.path());

        let result = execute_hook(
            &HookEvent::PostCreate,
            &config,
            &env_ctx,
            source.path(),
            work.path(),
            &db,
            repo_id,
            Some(wt_id),
        )
        .await
        .expect("hook should succeed");

        let logs = db.get_logs(result.event_id).unwrap();
        assert!(logs.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_failure_stops_shell_and_records_error() {
        let source = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let (db, repo_id, wt_id) = setup_db();

        let config = HookDef {
            copy: None,
            run: Some(vec![
                "echo before_fail".to_string(),
                "exit 42".to_string(),
            ]),
            shell: Some("echo should_not_run".to_string()),
            timeout_secs: Some(30),
        };

        let env_ctx = test_env_ctx(source.path(), work.path());

        let err = execute_hook(
            &HookEvent::PostCreate,
            &config,
            &env_ctx,
            source.path(),
            work.path(),
            &db,
            repo_id,
            Some(wt_id),
        )
        .await
        .expect_err("hook should fail");

        // Error message should mention the failed command
        let msg = err.to_string();
        assert!(msg.contains("exit 42") || msg.contains("42"), "error: {msg}");

        // Event should be recorded with non-zero exit code
        let events = db.list_events(wt_id, 10).unwrap();
        assert_eq!(events.len(), 1);
        let payload: serde_json::Value =
            serde_json::from_str(events[0].payload.as_deref().unwrap()).unwrap();
        assert_ne!(payload["exit_code"], 0);

        // Logs should contain "before_fail" but NOT "should_not_run"
        let event_id = events[0].id;
        let logs = db.get_logs(event_id).unwrap();
        let lines: Vec<&str> = logs.iter().map(|(_, l, _)| l.as_str()).collect();
        assert!(lines.contains(&"before_fail"), "should have run output before failure");
        assert!(
            !lines.iter().any(|l| l.contains("should_not_run")),
            "shell should not have run after run failure"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shell_failure_returns_error_and_records_event() {
        let source = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let (db, repo_id, wt_id) = setup_db();

        let config = HookDef {
            copy: None,
            run: Some(vec!["echo run_ok".to_string()]),
            shell: Some("echo shell_before; exit 1".to_string()),
            timeout_secs: Some(30),
        };

        let env_ctx = test_env_ctx(source.path(), work.path());

        let err = execute_hook(
            &HookEvent::PostCreate,
            &config,
            &env_ctx,
            source.path(),
            work.path(),
            &db,
            repo_id,
            Some(wt_id),
        )
        .await
        .expect_err("hook should fail");

        assert!(err.to_string().contains("exit code"), "error: {err}");

        // Both run and shell output should be logged
        let events = db.list_events(wt_id, 10).unwrap();
        assert_eq!(events.len(), 1);
        let event_id = events[0].id;
        let logs = db.get_logs(event_id).unwrap();
        let lines: Vec<&str> = logs.iter().map(|(_, l, _)| l.as_str()).collect();
        assert!(lines.contains(&"run_ok"));
        assert!(lines.contains(&"shell_before"));
    }
}
