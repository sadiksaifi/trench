use std::path::{Path, PathBuf};
use std::process::Command;

use super::GitError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub is_main: bool,
    pub is_current: bool,
    pub detached: bool,
}

pub fn discover(cwd: &Path) -> Result<Vec<DiscoveredWorktree>, GitError> {
    let active = git2::Repository::discover(cwd).map_err(|_| GitError::NotAGitRepo {
        path: cwd.to_path_buf(),
    })?;
    let current_path = active
        .workdir()
        .and_then(|path| path.canonicalize().ok())
        .ok_or_else(|| GitError::NotAGitRepo {
            path: cwd.to_path_buf(),
        })?;

    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["worktree", "list", "--porcelain", "-z"])
        .output()?;
    if !output.status.success() {
        return Err(GitError::CommandFailed {
            operation: "listing worktrees",
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    parse_porcelain(&output.stdout, &current_path)
}

fn parse_porcelain(bytes: &[u8], current_path: &Path) -> Result<Vec<DiscoveredWorktree>, GitError> {
    #[derive(Default)]
    struct Pending {
        path: Option<PathBuf>,
        branch: Option<String>,
        head: Option<String>,
        detached: bool,
    }

    fn finish(entries: &mut Vec<DiscoveredWorktree>, pending: &mut Pending, current_path: &Path) {
        let Some(path) = pending.path.take() else {
            return;
        };
        let path = path.canonicalize().unwrap_or(path);
        let is_current = path == current_path;
        entries.push(DiscoveredWorktree {
            path,
            branch: pending.branch.take(),
            head: pending.head.take(),
            is_main: entries.is_empty(),
            is_current,
            detached: std::mem::take(&mut pending.detached),
        });
    }

    let mut entries = Vec::new();
    let mut pending = Pending::default();
    for field in bytes.split(|byte| *byte == 0) {
        if field.is_empty() {
            finish(&mut entries, &mut pending, current_path);
            continue;
        }
        let field = String::from_utf8_lossy(field);
        if let Some(path) = field.strip_prefix("worktree ") {
            pending.path = Some(PathBuf::from(path));
        } else if let Some(head) = field.strip_prefix("HEAD ") {
            pending.head = Some(head.to_string());
        } else if let Some(branch) = field.strip_prefix("branch refs/heads/") {
            pending.branch = Some(branch.to_string());
        } else if field == "detached" {
            pending.detached = true;
        }
    }
    finish(&mut entries, &mut pending, current_path);

    if entries.is_empty() {
        return Err(GitError::CommandFailed {
            operation: "parsing worktree list",
            message: "Git returned no worktrees".to_string(),
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_fixture_preserves_detached_head_and_classification() {
        let fixture = b"worktree /repo\0HEAD 0123456789abcdef\0branch refs/heads/main\0\0worktree /repo/wt\0HEAD abcdef0123456789\0detached\0\0";
        let entries = parse_porcelain(fixture, Path::new("/repo/wt")).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_main);
        assert!(!entries[0].is_current);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(entries[1].is_current);
        assert!(entries[1].detached);
        assert_eq!(entries[1].head.as_deref(), Some("abcdef0123456789"));
        assert_eq!(entries[1].branch, None);
    }
}
