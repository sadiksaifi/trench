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
    /// Git reported a lock on this worktree (`git worktree lock`).
    pub locked: bool,
    /// The worktree directory no longer exists on disk.
    pub missing: bool,
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

/// Canonicalize a path that may no longer exist.
///
/// A deleted worktree directory cannot be canonicalized, but its parent
/// usually can. Resolving the parent keeps a missing worktree addressable by
/// the same absolute path whether or not the path crosses a symlink.
pub fn canonicalize_lenient(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        match (
            path.parent().and_then(|parent| parent.canonicalize().ok()),
            path.file_name(),
        ) {
            (Some(parent), Some(name)) => parent.join(name),
            _ => path.to_path_buf(),
        }
    })
}

fn parse_porcelain(bytes: &[u8], current_path: &Path) -> Result<Vec<DiscoveredWorktree>, GitError> {
    #[derive(Default)]
    struct Pending {
        path: Option<PathBuf>,
        branch: Option<String>,
        head: Option<String>,
        detached: bool,
        locked: bool,
    }

    fn finish(entries: &mut Vec<DiscoveredWorktree>, pending: &mut Pending, current_path: &Path) {
        let Some(path) = pending.path.take() else {
            return;
        };
        let missing = matches!(
            std::fs::symlink_metadata(&path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        );
        let path = canonicalize_lenient(&path);
        let is_current = path == current_path;
        entries.push(DiscoveredWorktree {
            path,
            branch: pending.branch.take(),
            head: pending.head.take(),
            is_main: entries.is_empty(),
            is_current,
            detached: std::mem::take(&mut pending.detached),
            locked: std::mem::take(&mut pending.locked),
            missing,
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
        } else if field == "locked" || field.starts_with("locked ") {
            pending.locked = true;
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

    #[cfg(unix)]
    #[test]
    fn missing_paths_resolve_symlinked_parents() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().canonicalize().unwrap().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert_eq!(canonicalize_lenient(&link.join("gone")), real.join("gone"));
        assert_eq!(
            canonicalize_lenient(Path::new("/no-such-parent/gone")),
            Path::new("/no-such-parent/gone")
        );
    }

    #[test]
    fn porcelain_fixture_reports_locked_and_missing_worktrees() {
        let root = tempfile::tempdir().unwrap();
        let present = root.path().canonicalize().unwrap();
        let gone = present.join("deleted-by-hand");
        let fixture = format!(
            "worktree {present}\0HEAD 0123456789abcdef\0branch refs/heads/main\0\0\
             worktree {gone}\0HEAD abcdef0123456789\0branch refs/heads/gone\0locked\0prunable gitdir file points to non-existent location\0\0\
             worktree {present}\0HEAD abcdef0123456789\0branch refs/heads/kept\0locked waiting on a long build\0\0",
            present = present.display(),
            gone = gone.display(),
        );
        let entries = parse_porcelain(fixture.as_bytes(), &present).unwrap();

        assert_eq!(entries.len(), 3);
        assert!(!entries[0].locked);
        assert!(!entries[0].missing);
        assert!(entries[1].locked);
        assert!(entries[1].missing);
        assert_eq!(entries[1].path, gone);
        assert!(entries[2].locked);
        assert!(!entries[2].missing);
    }
}
