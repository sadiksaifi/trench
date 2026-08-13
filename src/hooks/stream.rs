use anyhow::Result;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::process::{ChildStderr, ChildStdout};
use tokio::time::Instant;

use super::types::OutputStream;

/// Stream stdout/stderr from a child process to the terminal in real time,
/// capturing both into buffers. Returns `(stdout, stderr)` strings.
pub async fn stream_and_collect(
    stdout: ChildStdout,
    stderr: ChildStderr,
) -> Result<(String, String)> {
    stream_and_collect_with(stdout, stderr, |_, _| {}).await
}

pub async fn stream_and_collect_with(
    stdout: ChildStdout,
    stderr: ChildStderr,
    mut on_line: impl FnMut(OutputStream, String),
) -> Result<(String, String)> {
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
                        on_line(OutputStream::Stdout, line.clone());
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
                        on_line(OutputStream::Stderr, line.clone());
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

    Ok((stdout_buf, stderr_buf))
}

pub async fn stream_child_with_deadline(
    child: &mut Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    deadline: Instant,
    timeout_secs: u64,
    on_line: impl FnMut(OutputStream, String),
) -> Result<(String, String)> {
    let stream = stream_and_collect_with(stdout, stderr, on_line);
    tokio::pin!(stream);
    tokio::select! {
        result = &mut stream => result,
        _ = tokio::time::sleep_until(deadline) => {
            kill_process_group_and_reap(child).await?;
            Err(super::runner::HookTimeoutError { timeout_secs }.into())
        }
    }
}

pub async fn wait_child_with_deadline(
    child: &mut Child,
    deadline: Instant,
    timeout_secs: u64,
) -> Result<std::process::ExitStatus> {
    tokio::select! {
        result = child.wait() => Ok(result?),
        _ = tokio::time::sleep_until(deadline) => {
            kill_process_group_and_reap(child).await?;
            Err(super::runner::HookTimeoutError { timeout_secs }.into())
        }
    }
}

async fn kill_process_group_and_reap(child: &mut Child) -> Result<()> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // Hook commands are spawned into their own process groups. Killing
            // the negative PID stops the shell and every descendant before we
            // reap the direct child.
            let result = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    child.start_kill()?;
                }
            }
        } else {
            child.start_kill()?;
        }
    }
    #[cfg(not(unix))]
    child.start_kill()?;

    let _ = child.wait().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    #[tokio::test]
    async fn captures_stdout_and_stderr_separately() {
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("echo out_line; echo err_line >&2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let (out, err) = stream_and_collect(stdout, stderr).await.unwrap();

        child.wait().await.unwrap();

        assert_eq!(out.trim(), "out_line");
        assert_eq!(err.trim(), "err_line");
    }

    #[tokio::test]
    async fn captures_multiline_output() {
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("echo first; echo second; echo third")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let (out, err) = stream_and_collect(stdout, stderr).await.unwrap();

        child.wait().await.unwrap();

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines, vec!["first", "second", "third"]);
        assert!(err.is_empty());
    }
}
