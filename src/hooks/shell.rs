use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};

use super::stream::stream_and_collect;

/// Output from executing the shell step.
#[derive(Debug, Clone)]
pub struct ShellOutput {
    /// The script that was executed.
    pub script: String,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Process exit code.
    pub exit_code: i32,
}

/// Error returned when a shell script exits with a non-zero code.
#[derive(Debug, thiserror::Error)]
#[error("shell script failed with exit code {exit_code}")]
pub struct ShellStepError {
    pub exit_code: i32,
    pub output: ShellOutput,
}

/// Execute the shell step of a hook: run a multiline script via `sh -c`.
///
/// The script runs with cwd set to `cwd` and TRENCH_* env vars from `env_vars`.
/// stdout/stderr stream to the terminal in real time and are captured for logging.
/// Returns error on non-zero exit (FR-20).
pub async fn execute_shell_step(
    script: &str,
    cwd: &Path,
    env_vars: &HashMap<String, String>,
) -> Result<ShellOutput> {
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .envs(env_vars.iter())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn shell script")?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let (stdout_buf, stderr_buf) = stream_and_collect(stdout, stderr).await?;

    let status = child
        .wait()
        .await
        .context("failed to wait for shell script")?;

    let exit_code = status.code().unwrap_or(-1);

    let output = ShellOutput {
        script: script.to_string(),
        stdout: stdout_buf,
        stderr: stderr_buf,
        exit_code,
    };

    if !status.success() {
        return Err(ShellStepError {
            exit_code,
            output,
        }
        .into());
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn simple_script_executes_and_captures_stdout() {
        let dir = TempDir::new().unwrap();
        let env = HashMap::new();

        let result = execute_shell_step("echo hello", dir.path(), &env)
            .await
            .unwrap();

        assert_eq!(result.script, "echo hello");
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn env_vars_accessible_in_script() {
        let dir = TempDir::new().unwrap();
        let mut env = HashMap::new();
        env.insert("TRENCH_BRANCH".to_string(), "feature/auth".to_string());
        env.insert("TRENCH_EVENT".to_string(), "post_create".to_string());

        let result = execute_shell_step(
            "echo $TRENCH_BRANCH; echo $TRENCH_EVENT",
            dir.path(),
            &env,
        )
        .await
        .unwrap();

        let lines: Vec<&str> = result.stdout.lines().collect();
        assert_eq!(lines[0], "feature/auth");
        assert_eq!(lines[1], "post_create");
    }

    #[tokio::test]
    async fn multiline_script_executes_all_lines() {
        let dir = TempDir::new().unwrap();
        let env = HashMap::new();

        let script = "VAR=hello\necho $VAR\necho world";
        let result = execute_shell_step(script, dir.path(), &env)
            .await
            .unwrap();

        let lines: Vec<&str> = result.stdout.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "hello");
        assert_eq!(lines[1], "world");
    }

    #[tokio::test]
    async fn cwd_set_to_specified_directory() {
        let dir = TempDir::new().unwrap();
        let env = HashMap::new();

        let result = execute_shell_step("pwd", dir.path(), &env)
            .await
            .unwrap();

        let expected = dir.path().canonicalize().unwrap();
        let actual = std::path::PathBuf::from(result.stdout.trim())
            .canonicalize()
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn nonzero_exit_returns_error_with_output() {
        let dir = TempDir::new().unwrap();
        let env = HashMap::new();

        let err = execute_shell_step("echo before_fail; exit 42", dir.path(), &env)
            .await
            .unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains("42"), "error should contain exit code: {msg}");

        let shell_err = err.downcast_ref::<ShellStepError>().unwrap();
        assert_eq!(shell_err.exit_code, 42);
        assert_eq!(shell_err.output.stdout.trim(), "before_fail");
    }

    #[tokio::test]
    async fn stderr_captured_separately_from_stdout() {
        let dir = TempDir::new().unwrap();
        let env = HashMap::new();

        let result = execute_shell_step(
            "echo out_msg; echo err_msg >&2",
            dir.path(),
            &env,
        )
        .await
        .unwrap();

        assert_eq!(result.stdout.trim(), "out_msg");
        assert_eq!(result.stderr.trim(), "err_msg");
    }

    #[tokio::test]
    async fn integration_with_build_env_all_seven_vars() {
        use crate::hooks::{build_env, HookEnvContext, HookEvent};

        let dir = TempDir::new().unwrap();
        let ctx = HookEnvContext {
            worktree_path: "/tmp/wt".into(),
            worktree_name: "feat-auth".into(),
            branch: "feature/auth".into(),
            repo_name: "myrepo".into(),
            repo_path: "/tmp/repo".into(),
            base_branch: "main".into(),
        };
        let env = build_env(&ctx, &HookEvent::PostCreate);

        let script = r#"
echo $TRENCH_WORKTREE_PATH
echo $TRENCH_WORKTREE_NAME
echo $TRENCH_BRANCH
echo $TRENCH_REPO_NAME
echo $TRENCH_REPO_PATH
echo $TRENCH_BASE_BRANCH
echo $TRENCH_EVENT
"#;

        let result = execute_shell_step(script, dir.path(), &env)
            .await
            .unwrap();

        let lines: Vec<&str> = result.stdout.lines().collect();
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[0], "/tmp/wt");
        assert_eq!(lines[1], "feat-auth");
        assert_eq!(lines[2], "feature/auth");
        assert_eq!(lines[3], "myrepo");
        assert_eq!(lines[4], "/tmp/repo");
        assert_eq!(lines[5], "main");
        assert_eq!(lines[6], "post_create");
    }
}
