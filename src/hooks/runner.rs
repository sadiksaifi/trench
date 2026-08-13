use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::copy::execute_copy_step;
use super::run::execute_run_step_streaming;
use super::shell::execute_shell_step_streaming;
use super::types::{HookEmitter, HookStep, HookStreamEvent};
use super::{build_env, HookConfig, HookEnvContext, HookEvent};

#[derive(Debug, thiserror::Error)]
#[error("hook timed out after {timeout_secs}s")]
pub struct HookTimeoutError {
    pub timeout_secs: u64,
}

#[derive(Debug)]
pub struct HookResult {
    pub duration: Duration,
}

/// Execute one lifecycle hook in copy → run → shell order.
///
/// Output is emitted only to the current caller. The runner has no database,
/// TUI, history, diagnostic, or other persistence dependency.
pub async fn execute_hook(
    event: &HookEvent,
    config: &HookConfig,
    env_ctx: &HookEnvContext,
    source_dir: &Path,
    work_dir: &Path,
    emitter: &dyn HookEmitter,
) -> Result<HookResult> {
    let started = Instant::now();
    let environment = build_env(env_ctx, event);
    let timeout_secs = config.timeout_secs.unwrap_or(120);

    if let Some(patterns) = &config.copy {
        execute_step(emitter, HookStep::Copy, || {
            execute_copy_step(source_dir, work_dir, patterns)
                .context("copy step failed")
                .map(|_| ())
        })?;
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    if let Some(commands) = &config.run {
        let step_started = Instant::now();
        emitter.emit(HookStreamEvent::StepStarted {
            step: HookStep::Run,
        });
        match execute_run_step_streaming(
            commands,
            work_dir,
            &environment,
            emitter,
            deadline,
            timeout_secs,
        )
        .await
        {
            Ok(_) => finish_step(emitter, HookStep::Run, step_started, true),
            Err(error) => {
                finish_step(emitter, HookStep::Run, step_started, false);
                return Err(error);
            }
        }
    }

    if let Some(script) = &config.shell {
        let step_started = Instant::now();
        emitter.emit(HookStreamEvent::StepStarted {
            step: HookStep::Shell,
        });
        match execute_shell_step_streaming(
            script,
            work_dir,
            &environment,
            emitter,
            deadline,
            timeout_secs,
        )
        .await
        {
            Ok(_) => finish_step(emitter, HookStep::Shell, step_started, true),
            Err(error) => {
                finish_step(emitter, HookStep::Shell, step_started, false);
                return Err(error);
            }
        }
    }

    Ok(HookResult {
        duration: started.elapsed(),
    })
}

fn execute_step(
    emitter: &dyn HookEmitter,
    step: HookStep,
    execute: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let started = Instant::now();
    emitter.emit(HookStreamEvent::StepStarted { step });
    match execute() {
        Ok(()) => {
            finish_step(emitter, step, started, true);
            Ok(())
        }
        Err(error) => {
            finish_step(emitter, step, started, false);
            Err(error)
        }
    }
}

fn finish_step(emitter: &dyn HookEmitter, step: HookStep, started: Instant, success: bool) {
    emitter.emit(HookStreamEvent::StepFinished {
        step,
        success,
        duration: started.elapsed(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookDef;
    use crate::hooks::types::{OutputStream, RecordingHookEmitter};

    fn context(source: &Path, work: &Path) -> HookEnvContext {
        HookEnvContext {
            worktree_path: work.to_string_lossy().into_owned(),
            worktree_name: "test-wt".into(),
            branch: "test-branch".into(),
            repo_name: "test-repo".into(),
            repo_path: source.to_string_lossy().into_owned(),
            base_branch: "main".into(),
        }
    }

    #[tokio::test]
    async fn streams_copy_run_shell_in_order_without_persistence() {
        let source = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join(".env"), "test=true").unwrap();
        let hook = HookDef {
            copy: Some(vec![".env".into()]),
            run: Some(vec!["printf run-output".into()]),
            shell: Some("printf shell-output >&2".into()),
            timeout_secs: Some(30),
        };
        let emitter = RecordingHookEmitter::default();

        execute_hook(
            &HookEvent::PostCreate,
            &hook,
            &context(source.path(), work.path()),
            source.path(),
            work.path(),
            &emitter,
        )
        .await
        .unwrap();

        let events = emitter.events();
        let started = events
            .iter()
            .filter_map(|event| match event {
                HookStreamEvent::StepStarted { step } => Some(*step),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(started, [HookStep::Copy, HookStep::Run, HookStep::Shell]);
        assert!(events.contains(&HookStreamEvent::Output {
            step: HookStep::Run,
            stream: OutputStream::Stdout,
            line: "run-output".into(),
        }));
        assert!(events.contains(&HookStreamEvent::Output {
            step: HookStep::Shell,
            stream: OutputStream::Stderr,
            line: "shell-output".into(),
        }));
    }

    #[tokio::test]
    async fn timeout_kills_hook_process_group_before_it_can_mutate_later() {
        let source = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let marker = work.path().join("late-side-effect");
        let hook = HookDef {
            run: Some(vec![format!(
                "(sleep 2; touch '{}') & wait",
                marker.display()
            )]),
            timeout_secs: Some(1),
            ..Default::default()
        };

        let error = execute_hook(
            &HookEvent::PreCreate,
            &hook,
            &context(source.path(), work.path()),
            source.path(),
            work.path(),
            &crate::hooks::types::NoopHookEmitter,
        )
        .await
        .unwrap_err();

        assert!(error.is::<HookTimeoutError>());
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(
            !marker.exists(),
            "timed-out hook kept mutating after return"
        );
    }

    #[tokio::test]
    async fn timeout_still_applies_after_hook_closes_its_output_streams() {
        let source = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let marker = work.path().join("closed-stream-side-effect");
        let hook = HookDef {
            shell: Some(format!(
                "exec >/dev/null 2>/dev/null; sleep 2; touch '{}'",
                marker.display()
            )),
            timeout_secs: Some(1),
            ..Default::default()
        };

        let error = execute_hook(
            &HookEvent::PreCreate,
            &hook,
            &context(source.path(), work.path()),
            source.path(),
            work.path(),
            &crate::hooks::types::NoopHookEmitter,
        )
        .await
        .unwrap_err();

        assert!(error.is::<HookTimeoutError>());
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!marker.exists());
    }
}
