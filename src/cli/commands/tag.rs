use std::path::Path;

use anyhow::Result;

use crate::git;
use crate::state::Database;

/// Parsed tag operation from CLI input.
#[derive(Debug, PartialEq)]
pub enum TagOp {
    Add(String),
    Remove(String),
}

/// Parse raw CLI tag arguments into structured operations.
///
/// `+name` → Add, `-name` → Remove. Returns error for invalid format.
pub fn parse_tag_args(args: &[String]) -> Result<Vec<TagOp>> {
    let mut ops = Vec::new();
    for arg in args {
        if let Some(name) = arg.strip_prefix('+') {
            if name.is_empty() {
                anyhow::bail!("tag name cannot be empty: '{arg}'");
            }
            ops.push(TagOp::Add(name.to_string()));
        } else if let Some(name) = arg.strip_prefix('-') {
            if name.is_empty() {
                anyhow::bail!("tag name cannot be empty: '{arg}'");
            }
            ops.push(TagOp::Remove(name.to_string()));
        } else {
            anyhow::bail!(
                "invalid tag argument '{arg}': must start with '+' (add) or '-' (remove)"
            );
        }
    }
    Ok(ops)
}

/// Execute the `trench tag` command.
///
/// If `tags` is empty, lists current tags. Otherwise, applies add/remove operations.
/// Returns a formatted string for display.
pub fn execute(identifier: &str, tags: &[String], cwd: &Path, db: &Database) -> Result<String> {
    let repo_info = git::discover_repo(cwd)?;
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db
        .get_repo_by_path(repo_path_str)?
        .ok_or_else(|| anyhow::anyhow!("repo not tracked by trench: {repo_path_str}"))?;

    let wt = db
        .find_worktree_by_identifier(repo.id, identifier)?
        .ok_or_else(|| anyhow::anyhow!("worktree not found: {identifier}"))?;

    if tags.is_empty() {
        // List mode
        let current_tags = db.list_tags(wt.id)?;
        if current_tags.is_empty() {
            return Ok(format!("No tags on worktree '{}'.\n", wt.name));
        }
        return Ok(current_tags.join(", ") + "\n");
    }

    let ops = parse_tag_args(tags)?;
    for op in &ops {
        match op {
            TagOp::Add(name) => db.add_tag(wt.id, name)?,
            TagOp::Remove(name) => db.remove_tag(wt.id, name)?,
        }
    }

    let current_tags = db.list_tags(wt.id)?;
    if current_tags.is_empty() {
        Ok(format!("All tags removed from worktree '{}'.\n", wt.name))
    } else {
        Ok(format!(
            "Tags on '{}': {}\n",
            wt.name,
            current_tags.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_add_tags() {
        let ops = parse_tag_args(&["+wip".to_string(), "+review".to_string()]).unwrap();
        assert_eq!(
            ops,
            vec![TagOp::Add("wip".to_string()), TagOp::Add("review".to_string())]
        );
    }

    #[test]
    fn parse_remove_tags() {
        let ops = parse_tag_args(&["-wip".to_string()]).unwrap();
        assert_eq!(ops, vec![TagOp::Remove("wip".to_string())]);
    }

    #[test]
    fn parse_mixed_tags() {
        let ops = parse_tag_args(&["+wip".to_string(), "-old".to_string()]).unwrap();
        assert_eq!(
            ops,
            vec![TagOp::Add("wip".to_string()), TagOp::Remove("old".to_string())]
        );
    }

    #[test]
    fn parse_rejects_bare_name() {
        let err = parse_tag_args(&["bare".to_string()]).unwrap_err();
        assert!(err.to_string().contains("must start with"));
    }

    #[test]
    fn parse_rejects_empty_tag_name() {
        let err = parse_tag_args(&["+".to_string()]).unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn execute_adds_tags_to_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();
        let wt = db
            .insert_worktree(db_repo.id, "my-wt", "my-branch", "/wt/my-wt", Some("main"))
            .unwrap();

        let output = execute(
            "my-wt",
            &["+wip".to_string(), "+review".to_string()],
            repo_dir.path(),
            &db,
        )
        .unwrap();

        assert!(output.contains("wip"), "output should mention wip tag");
        assert!(output.contains("review"), "output should mention review tag");

        // Verify in DB
        let tags = db.list_tags(wt.id).unwrap();
        assert_eq!(tags, vec!["review", "wip"]); // sorted alphabetically
    }

    #[test]
    fn execute_lists_tags_when_no_ops() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();
        let wt = db
            .insert_worktree(db_repo.id, "my-wt", "my-branch", "/wt/my-wt", Some("main"))
            .unwrap();

        db.add_tag(wt.id, "wip").unwrap();

        let output = execute("my-wt", &[], repo_dir.path(), &db).unwrap();
        assert!(output.contains("wip"));
    }

    #[test]
    fn execute_shows_empty_state_when_no_tags() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();
        db.insert_worktree(db_repo.id, "my-wt", "my-branch", "/wt/my-wt", Some("main"))
            .unwrap();

        let output = execute("my-wt", &[], repo_dir.path(), &db).unwrap();
        assert!(output.contains("No tags"));
    }

    #[test]
    fn execute_removes_tags_from_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();
        let wt = db
            .insert_worktree(db_repo.id, "my-wt", "my-branch", "/wt/my-wt", Some("main"))
            .unwrap();

        db.add_tag(wt.id, "wip").unwrap();
        db.add_tag(wt.id, "review").unwrap();

        let output = execute(
            "my-wt",
            &["-wip".to_string()],
            repo_dir.path(),
            &db,
        )
        .unwrap();

        assert!(!output.contains("wip"), "wip should be removed");
        assert!(output.contains("review"), "review should remain");

        let tags = db.list_tags(wt.id).unwrap();
        assert_eq!(tags, vec!["review"]);
    }

    #[test]
    fn execute_removes_all_tags_shows_message() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();
        let wt = db
            .insert_worktree(db_repo.id, "my-wt", "my-branch", "/wt/my-wt", Some("main"))
            .unwrap();

        db.add_tag(wt.id, "wip").unwrap();

        let output = execute(
            "my-wt",
            &["-wip".to_string()],
            repo_dir.path(),
            &db,
        )
        .unwrap();

        assert!(output.contains("All tags removed"), "should show removal message");
        let tags = db.list_tags(wt.id).unwrap();
        assert!(tags.is_empty());
    }

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
}
