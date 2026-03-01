use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};

/// Output from a single command execution.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// The command string that was executed.
    pub command: String,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Process exit code.
    pub exit_code: i32,
}

/// Result of executing the run step.
#[derive(Debug, Clone)]
pub struct RunResult {
    /// Output from each executed command, in order.
    pub executed: Vec<CommandOutput>,
}

/// Error returned when a command in the run step exits with a non-zero code.
/// Contains partial results from all commands that executed (including the failed one).
#[derive(Debug, thiserror::Error)]
#[error("command failed: `{command}` exited with code {exit_code}")]
pub struct RunStepError {
    pub command: String,
    pub exit_code: i32,
    pub results: RunResult,
}

/// Execute the run step of a hook: run commands sequentially with streaming output.
///
/// Each command string is executed via `sh -c "<command>"`.
/// Commands run with cwd set to `cwd` and TRENCH_* env vars from `env_vars`.
/// stdout/stderr stream to the terminal in real time and are captured for logging.
/// Stops on first non-zero exit code (FR-20, FR-22).
pub async fn execute_run_step(
    commands: &[String],
    cwd: &Path,
    env_vars: &HashMap<String, String>,
) -> Result<RunResult> {
    let mut executed = Vec::new();

    for cmd in commands {
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(cwd)
            .envs(env_vars.iter())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn command: {cmd}"))?;

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut stdout_done = false;
        let mut stderr_done = false;

        while !stdout_done || !stderr_done {
            tokio::select! {
                result = stdout_reader.next_line(), if !stdout_done => {
                    match result? {
                        Some(line) => {
                            println!("{line}");
                            if !stdout_buf.is_empty() {
                                stdout_buf.push('\n');
                            }
                            stdout_buf.push_str(&line);
                        }
                        None => stdout_done = true,
                    }
                }
                result = stderr_reader.next_line(), if !stderr_done => {
                    match result? {
                        Some(line) => {
                            eprintln!("{line}");
                            if !stderr_buf.is_empty() {
                                stderr_buf.push('\n');
                            }
                            stderr_buf.push_str(&line);
                        }
                        None => stderr_done = true,
                    }
                }
            }
        }

        let status = child
            .wait()
            .await
            .with_context(|| format!("failed to wait for command: {cmd}"))?;

        let exit_code = status.code().unwrap_or(-1);

        executed.push(CommandOutput {
            command: cmd.clone(),
            stdout: stdout_buf,
            stderr: stderr_buf,
            exit_code,
        });

        if !status.success() {
            return Err(RunStepError {
                command: cmd.clone(),
                exit_code,
                results: RunResult { executed },
            }
            .into());
        }
    }

    Ok(RunResult { executed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn single_command_executes_and_captures_stdout() {
        let dir = TempDir::new().unwrap();
        let commands = vec!["echo hello".to_string()];
        let env = HashMap::new();

        let result = execute_run_step(&commands, dir.path(), &env).await.unwrap();

        assert_eq!(result.executed.len(), 1);
        assert_eq!(result.executed[0].command, "echo hello");
        assert_eq!(result.executed[0].stdout.trim(), "hello");
        assert_eq!(result.executed[0].exit_code, 0);
    }

    #[tokio::test]
    async fn sequential_commands_execute_in_order() {
        let dir = TempDir::new().unwrap();
        let commands = vec![
            "echo first".to_string(),
            "echo second".to_string(),
            "echo third".to_string(),
        ];
        let env = HashMap::new();

        let result = execute_run_step(&commands, dir.path(), &env).await.unwrap();

        assert_eq!(result.executed.len(), 3);
        assert_eq!(result.executed[0].stdout.trim(), "first");
        assert_eq!(result.executed[1].stdout.trim(), "second");
        assert_eq!(result.executed[2].stdout.trim(), "third");
    }

    #[tokio::test]
    async fn commands_run_with_specified_working_directory() {
        let dir = TempDir::new().unwrap();
        let commands = vec!["pwd".to_string()];
        let env = HashMap::new();

        let result = execute_run_step(&commands, dir.path(), &env).await.unwrap();

        let output_path = result.executed[0].stdout.trim();
        // Canonicalize both to handle symlinks like /tmp -> /private/tmp on macOS
        let expected = dir.path().canonicalize().unwrap();
        let actual = std::path::PathBuf::from(output_path).canonicalize().unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn env_vars_available_in_commands() {
        let dir = TempDir::new().unwrap();
        let commands = vec![
            "echo $TRENCH_BRANCH".to_string(),
            "echo $TRENCH_EVENT".to_string(),
        ];
        let mut env = HashMap::new();
        env.insert("TRENCH_BRANCH".to_string(), "feature/auth".to_string());
        env.insert("TRENCH_EVENT".to_string(), "post_create".to_string());

        let result = execute_run_step(&commands, dir.path(), &env).await.unwrap();

        assert_eq!(result.executed[0].stdout.trim(), "feature/auth");
        assert_eq!(result.executed[1].stdout.trim(), "post_create");
    }
}
