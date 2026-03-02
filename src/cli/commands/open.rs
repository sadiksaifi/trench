use std::path::Path;

use anyhow::Result;

use crate::state::Database;

/// Result of resolving the open command (before actually launching the editor).
#[derive(Debug)]
pub struct OpenResult {
    /// Sanitized name of the worktree.
    pub name: String,
    /// Absolute path to the worktree.
    pub path: String,
    /// Editor command that should be used to open the worktree.
    pub editor: String,
}

/// Resolve the editor command from the fallback chain:
/// config override → $EDITOR → $VISUAL → error.
fn resolve_editor(config_editor: Option<&str>) -> Result<String> {
    if let Some(cmd) = config_editor {
        return Ok(cmd.to_string());
    }
    if let Ok(editor) = std::env::var("EDITOR") {
        if !editor.is_empty() {
            return Ok(editor);
        }
    }
    if let Ok(visual) = std::env::var("VISUAL") {
        if !visual.is_empty() {
            return Ok(visual);
        }
    }
    anyhow::bail!(
        "no editor configured. Set $EDITOR, $VISUAL, or add [editor] command = \"...\" to your config"
    )
}

/// Resolve the worktree and editor for `trench open <identifier>`.
///
/// Does NOT launch the editor — returns the resolved information so the
/// caller (or tests) can decide what to do with it.
pub fn resolve(
    identifier: &str,
    cwd: &Path,
    db: &Database,
    config_editor: Option<&str>,
) -> Result<OpenResult> {
    let repo_info = crate::git::discover_repo(cwd)?;
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db
        .get_repo_by_path(repo_path_str)?
        .ok_or_else(|| anyhow::anyhow!("repository not tracked by trench"))?;

    // Try the identifier as-is first, then try sanitizing it
    let wt = match db.find_worktree_by_identifier(repo.id, identifier)? {
        Some(wt) => wt,
        None => {
            let sanitized = crate::paths::sanitize_branch(identifier);
            if sanitized != identifier {
                db.find_worktree_by_identifier(repo.id, &sanitized)?
            } else {
                None
            }
            .ok_or_else(|| anyhow::anyhow!("worktree not found: {identifier}"))?
        }
    };

    let editor = resolve_editor(config_editor)?;

    // Update last_accessed timestamp
    let now = crate::state::unix_epoch_secs() as i64;
    db.update_worktree(
        wt.id,
        &crate::state::WorktreeUpdate {
            last_accessed: Some(Some(now)),
            ..Default::default()
        },
    )?;

    // Record "opened" event
    db.insert_event(repo.id, Some(wt.id), "opened", None)?;

    Ok(OpenResult {
        name: wt.name.clone(),
        path: wt.path.clone(),
        editor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Database;
    use std::path::Path;

    fn init_repo_with_commit(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).expect("failed to init repo");
        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
                .unwrap();
        }
        repo
    }

    #[test]
    fn resolve_returns_worktree_path_and_config_editor() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
        db.insert_worktree(db_repo.id, "my-feature", "my-feature", "/wt/my-feature", Some("main"))
            .unwrap();

        let result = resolve("my-feature", repo_dir.path(), &db, Some("code")).unwrap();

        assert_eq!(result.name, "my-feature");
        assert_eq!(result.path, "/wt/my-feature");
        assert_eq!(result.editor, "code");
    }
}
