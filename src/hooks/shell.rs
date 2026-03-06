use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};

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
}
