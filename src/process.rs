//! Process detection for worktree directories.
//!
//! Detects running processes (dev servers, watchers, etc.) whose current
//! working directory is within a worktree path. Uses `lsof` on macOS and
//! `/proc` on Linux. Detection failures are graceful — they return an
//! empty list, never an error.

use std::collections::HashSet;
use std::path::Path;

/// Information about a process running in a worktree directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
}

/// Check whether `cwd` is equal to or a subdirectory of `worktree_path`,
/// normalizing trailing slashes so `/repo/wt/` and `/repo/wt` are equivalent.
fn within_worktree(cwd: &str, worktree_path: &str) -> bool {
    let wt = match worktree_path.trim_end_matches('/') {
        "" => "/",
        normalized => normalized,
    };
    let cwd = match cwd.trim_end_matches('/') {
        "" => "/",
        normalized => normalized,
    };
    cwd == wt || cwd.starts_with(&format!("{wt}/"))
}

/// Parse `lsof -F pcn -d cwd` output into process entries whose cwd is
/// within `worktree_path`.
///
/// The `-F pcn` format emits one field per line:
/// - `p<pid>` — process ID
/// - `c<command>` — command name
/// - `n<path>` — file name (cwd path when `-d cwd` is used)
pub fn parse_lsof_output(output: &str, worktree_path: &str) -> Vec<ProcessInfo> {
    let mut results = Vec::new();
    let mut seen_pids = HashSet::new();
    let mut current_pid: Option<u32> = None;
    let mut current_name: Option<String> = None;

    for line in output.lines() {
        if let Some(pid_str) = line.strip_prefix('p') {
            // Emit previous entry if we had one pending (shouldn't happen in
            // well-formed output, but be defensive)
            current_pid = pid_str.parse().ok();
            current_name = None;
        } else if let Some(cmd) = line.strip_prefix('c') {
            current_name = Some(cmd.to_string());
        } else if let Some(path) = line.strip_prefix('n') {
            if let (Some(pid), Some(ref name)) = (current_pid, &current_name) {
                if within_worktree(path, worktree_path)
                    && seen_pids.insert(pid)
                {
                    results.push(ProcessInfo {
                        pid,
                        name: name.clone(),
                    });
                }
            }
        }
    }

    results
}

/// Scan a `/proc`-style directory for processes whose cwd is within
/// `worktree_path`. Used on Linux where `/proc/<pid>/cwd` is a symlink
/// to the process's current working directory.
pub fn scan_proc_dir(proc_path: &Path, worktree_path: &str) -> Vec<ProcessInfo> {
    let mut results = Vec::new();

    let entries = match std::fs::read_dir(proc_path) {
        Ok(e) => e,
        Err(_) => return results,
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        // Only look at numeric directories (PIDs)
        let pid: u32 = match name_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        let cwd_link = entry.path().join("cwd");
        let cwd = match std::fs::read_link(&cwd_link) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let cwd_str = cwd.to_string_lossy();
        if !within_worktree(cwd_str.as_ref(), worktree_path) {
            continue;
        }

        let comm_path = entry.path().join("comm");
        let name = std::fs::read_to_string(&comm_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("<pid {pid}>"));

        results.push(ProcessInfo { pid, name });
    }

    results
}

/// Detect processes running in the given worktree directory.
///
/// Returns an empty `Vec` if detection fails or the path doesn't exist.
/// This is intentionally graceful — process detection is informational,
/// not critical.
pub fn detect_processes(worktree_path: &str) -> Vec<ProcessInfo> {
    if !Path::new(worktree_path).exists() {
        return Vec::new();
    }

    #[cfg(target_os = "macos")]
    {
        detect_via_lsof(worktree_path)
    }

    #[cfg(target_os = "linux")]
    {
        scan_proc_dir(Path::new("/proc"), worktree_path)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Vec::new()
    }
}

/// macOS: run `lsof -d cwd -F pcn` and filter for worktree path.
#[cfg(target_os = "macos")]
fn detect_via_lsof(worktree_path: &str) -> Vec<ProcessInfo> {
    let output = match std::process::Command::new("lsof")
        .args(["-d", "cwd", "-F", "pcn"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_lsof_output(&stdout, worktree_path)
}

fn build_process_warning(procs: &[ProcessInfo]) -> Option<String> {
    if procs.is_empty() {
        return None;
    }

    let names: Vec<&str> = procs.iter().map(|p| p.name.as_str()).collect();
    let count = procs.len();
    Some(format!(
        "warning: {count} process{} running in this worktree: {}",
        if count == 1 { "" } else { "es" },
        names.join(", "),
    ))
}

/// Format a warning message about running processes for display before
/// destructive operations like `trench remove`.
///
/// Returns `None` if no processes are detected.
pub fn format_process_warning(worktree_path: &str) -> Option<String> {
    let procs = detect_processes(worktree_path);
    build_process_warning(&procs)
}

/// Build a warning string from already-detected processes (for TUI use
/// where detection is done separately).
pub fn format_process_warning_from(procs: &[ProcessInfo]) -> Option<String> {
    build_process_warning(procs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_warning_with_single_process() {
        let procs = vec![ProcessInfo { pid: 1234, name: "node".into() }];
        let warning = format_process_warning_from(&procs);
        assert_eq!(
            warning.as_deref(),
            Some("warning: 1 process running in this worktree: node"),
        );
    }

    #[test]
    fn format_warning_with_multiple_processes() {
        let procs = vec![
            ProcessInfo { pid: 1234, name: "node".into() },
            ProcessInfo { pid: 5678, name: "vite".into() },
        ];
        let warning = format_process_warning_from(&procs);
        assert_eq!(
            warning.as_deref(),
            Some("warning: 2 processes running in this worktree: node, vite"),
        );
    }

    #[test]
    fn format_warning_returns_none_for_empty() {
        let warning = format_process_warning_from(&[]);
        assert!(warning.is_none());
    }

    #[test]
    fn build_process_warning_shared_by_both_functions() {
        let procs = vec![
            ProcessInfo { pid: 1, name: "node".into() },
            ProcessInfo { pid: 2, name: "vite".into() },
        ];
        let from_helper = build_process_warning(&procs);
        let from_public = format_process_warning_from(&procs);
        assert_eq!(from_helper, from_public);
    }

    #[test]
    fn format_process_warning_returns_none_for_nonexistent_path() {
        let warning = format_process_warning("/nonexistent/path/xyz");
        assert!(warning.is_none());
    }

    #[test]
    fn parse_lsof_output_extracts_matching_processes() {
        // lsof -F pcn -d cwd produces output like:
        // p1234
        // cnode
        // n/Users/sdk/.worktrees/myrepo/feature-branch
        // p5678
        // cbun
        // n/Users/sdk/other-project
        let output = "\
p1234\n\
cnode\n\
n/Users/sdk/.worktrees/myrepo/feature-branch\n\
p5678\n\
cbun\n\
n/Users/sdk/other-project\n\
p9999\n\
cvite\n\
n/Users/sdk/.worktrees/myrepo/feature-branch/packages/app\n";

        let result = parse_lsof_output(
            output,
            "/Users/sdk/.worktrees/myrepo/feature-branch",
        );

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], ProcessInfo { pid: 1234, name: "node".into() });
        assert_eq!(result[1], ProcessInfo { pid: 9999, name: "vite".into() });
    }

    #[test]
    fn parse_lsof_output_returns_empty_for_no_matches() {
        let output = "p1234\ncbun\nn/Users/sdk/other-project\n";
        let result = parse_lsof_output(output, "/Users/sdk/.worktrees/myrepo/feature");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_lsof_output_handles_empty_input() {
        let result = parse_lsof_output("", "/some/path");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_lsof_output_handles_malformed_input() {
        let output = "garbage\nmore garbage\n";
        let result = parse_lsof_output(output, "/some/path");
        assert!(result.is_empty());
    }

    #[test]
    fn scan_proc_finds_processes_with_matching_cwd() {
        // Create a fake /proc-like directory structure
        let proc_dir = tempfile::tempdir().unwrap();
        let worktree_dir = tempfile::tempdir().unwrap();
        let worktree_path = worktree_dir.path().to_str().unwrap();

        // PID 100: cwd points to worktree (match)
        let pid100 = proc_dir.path().join("100");
        std::fs::create_dir(&pid100).unwrap();
        std::fs::write(pid100.join("comm"), "node\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(worktree_path, pid100.join("cwd")).unwrap();

        // PID 200: cwd points to subdirectory of worktree (match)
        let subdir = worktree_dir.path().join("packages");
        std::fs::create_dir(&subdir).unwrap();
        let pid200 = proc_dir.path().join("200");
        std::fs::create_dir(&pid200).unwrap();
        std::fs::write(pid200.join("comm"), "vite\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(subdir.to_str().unwrap(), pid200.join("cwd")).unwrap();

        // PID 300: cwd points elsewhere (no match)
        let other_dir = tempfile::tempdir().unwrap();
        let pid300 = proc_dir.path().join("300");
        std::fs::create_dir(&pid300).unwrap();
        std::fs::write(pid300.join("comm"), "bash\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            other_dir.path().to_str().unwrap(),
            pid300.join("cwd"),
        )
        .unwrap();

        // Non-numeric directory (should be skipped)
        std::fs::create_dir(proc_dir.path().join("self")).unwrap();

        let result = scan_proc_dir(proc_dir.path(), worktree_path);
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|p| p.pid == 100 && p.name == "node"));
        assert!(result.iter().any(|p| p.pid == 200 && p.name == "vite"));
    }

    #[test]
    fn scan_proc_handles_missing_comm() {
        let proc_dir = tempfile::tempdir().unwrap();
        let worktree_dir = tempfile::tempdir().unwrap();
        let worktree_path = worktree_dir.path().to_str().unwrap();

        // PID 999: cwd matches but no comm file
        let pid999 = proc_dir.path().join("999");
        std::fs::create_dir(&pid999).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(worktree_path, pid999.join("cwd")).unwrap();
        // deliberately no comm file

        let result = scan_proc_dir(proc_dir.path(), worktree_path);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].pid, 999);
        assert_eq!(result[0].name, "<pid 999>");
    }

    #[test]
    fn scan_proc_returns_empty_for_nonexistent_dir() {
        let result = scan_proc_dir(std::path::Path::new("/nonexistent/proc"), "/some/path");
        assert!(result.is_empty());
    }

    #[test]
    fn detect_processes_returns_empty_for_nonexistent_path() {
        let result = detect_processes("/nonexistent/worktree/path/xyz");
        assert!(result.is_empty(), "should return empty for non-existent path");
    }

    #[test]
    fn detect_processes_returns_empty_for_real_temp_dir() {
        // A real directory with no processes running in it
        let tmp = tempfile::tempdir().unwrap();
        let result = detect_processes(tmp.path().to_str().unwrap());
        assert!(result.is_empty(), "empty temp dir should have no processes");
    }

    #[test]
    fn within_worktree_normalizes_trailing_slash() {
        assert!(within_worktree("/repo/wt", "/repo/wt/"));
        assert!(within_worktree("/repo/wt/", "/repo/wt"));
        assert!(within_worktree("/repo/wt/sub", "/repo/wt/"));
        assert!(within_worktree("/repo/wt/sub", "/repo/wt"));
        assert!(!within_worktree("/repo/other", "/repo/wt"));
        assert!(!within_worktree("/repo/wt-extra", "/repo/wt"));
    }

    #[test]
    fn parse_lsof_output_handles_trailing_slash() {
        let output = "p1234\ncnode\nn/repo/wt\n";
        // worktree_path with trailing slash should still match
        let result = parse_lsof_output(output, "/repo/wt/");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].pid, 1234);
    }

    #[test]
    fn parse_lsof_output_deduplicates_by_pid() {
        // Same PID might appear multiple times if lsof output is weird
        let output = "\
p1234\n\
cnode\n\
n/worktree/path\n\
p1234\n\
cnode\n\
n/worktree/path/subdir\n";

        let result = parse_lsof_output(output, "/worktree/path");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].pid, 1234);
    }
}
