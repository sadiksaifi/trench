use anyhow::Result;

use crate::git::{self, RepoInfo};
use crate::paths;
use crate::state::{Database, Repo, Worktree};

/// Ensure the repo exists in the DB, inserting it if needed.
fn ensure_repo(db: &Database, repo_info: &RepoInfo) -> Result<Repo> {
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    if let Some(repo) = db.get_repo_by_path(repo_path_str)? {
        return Ok(repo);
    }

    db.insert_repo(
        &repo_info.name,
        repo_path_str,
        Some(&repo_info.default_branch),
    )
}

/// Resolve a worktree by identifier without writing to the database.
///
/// Like [`resolve_or_adopt`], tries the DB first (if provided) then falls back
/// to git worktree discovery. However, the git fallback constructs virtual
/// `Repo` and `Worktree` structs with placeholder IDs (id=0) instead of
/// inserting rows. Safe for read-only contexts like `--dry-run`.
pub fn resolve_only(
    identifier: &str,
    repo_info: &RepoInfo,
    db: Option<&Database>,
) -> Result<(Repo, Worktree)> {
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    // Try DB first (read-only)
    if let Some(db) = db {
        if let Some(repo) = db.get_repo_by_path(repo_path_str)? {
            if let Some(wt) = db.find_worktree_by_identifier(repo.id, identifier)? {
                return Ok((repo, wt));
            }
            let sanitized = paths::sanitize_branch(identifier);
            if sanitized != identifier {
                if let Some(wt) = db.find_worktree_by_identifier(repo.id, &sanitized)? {
                    return Ok((repo, wt));
                }
            }
        }
    }

    // Fall back to git worktrees — construct virtual structs (no writes)
    let git_worktrees = git::list_worktrees(&repo_info.path)?;
    let sanitized = paths::sanitize_branch(identifier);

    for gw in &git_worktrees {
        let branch_match = gw.branch.as_deref() == Some(identifier);
        let name_match = gw.name == identifier || gw.name == sanitized;
        let sanitized_branch_match = gw
            .branch
            .as_deref()
            .is_some_and(|b| paths::sanitize_branch(b) == sanitized);

        if branch_match || name_match || sanitized_branch_match {
            let branch = gw.branch.clone().unwrap_or_else(|| identifier.to_string());
            let name = paths::sanitize_branch(&branch);

            let repo = Repo {
                id: 0,
                name: repo_info.name.clone(),
                path: repo_path_str.to_string(),
                default_base: Some(repo_info.default_branch.clone()),
                created_at: 0,
            };
            let wt = Worktree {
                id: 0,
                repo_id: 0,
                name,
                branch,
                path: gw.path.to_string_lossy().to_string(),
                base_branch: None,
                managed: false,
                adopted_at: None,
                last_accessed: None,
                removed_at: None,
                created_at: 0,
            };
            return Ok((repo, wt));
        }
    }

    anyhow::bail!("worktree not found: {identifier}")
}

/// Resolve a worktree by identifier, adopting it if unmanaged.
///
/// Tries the DB first (exact match, then sanitized fallback). If not found,
/// falls back to git worktree discovery. If found via git, silently adopts
/// the worktree (inserts into DB with `adopted_at` set).
///
/// Returns the repo and worktree records.
pub fn resolve_or_adopt(
    identifier: &str,
    repo_info: &RepoInfo,
    db: &Database,
) -> Result<(Repo, Worktree)> {
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    // Try DB first
    if let Some(repo) = db.get_repo_by_path(repo_path_str)? {
        if let Some(wt) = db.find_worktree_by_identifier(repo.id, identifier)? {
            return Ok((repo, wt));
        }
        let sanitized = paths::sanitize_branch(identifier);
        if sanitized != identifier {
            if let Some(wt) = db.find_worktree_by_identifier(repo.id, &sanitized)? {
                return Ok((repo, wt));
            }
        }
    }

    // Fall back to git worktrees
    let git_worktrees = git::list_worktrees(&repo_info.path)?;
    let sanitized = paths::sanitize_branch(identifier);

    for gw in &git_worktrees {
        let branch_match = gw.branch.as_deref() == Some(identifier);
        let name_match = gw.name == identifier || gw.name == sanitized;
        let sanitized_branch_match = gw
            .branch
            .as_deref()
            .is_some_and(|b| paths::sanitize_branch(b) == sanitized);

        if branch_match || name_match || sanitized_branch_match {
            let repo = ensure_repo(db, repo_info)?;
            let branch = gw.branch.clone().unwrap_or_else(|| identifier.to_string());
            let name = paths::sanitize_branch(&branch);
            let path = gw.path.to_string_lossy();

            let wt = db.adopt_worktree(repo.id, &name, &branch, &path, None)?;
            return Ok((repo, wt));
        }
    }

    anyhow::bail!("worktree not found: {identifier}")
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
    fn resolve_or_adopt_returns_existing_managed_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        let inserted = db
            .insert_worktree(
                db_repo.id,
                "my-feature",
                "my-feature",
                "/wt/my-feature",
                Some("main"),
            )
            .unwrap();

        let repo_info = git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            resolve_or_adopt("my-feature", &repo_info, &db).expect("should resolve existing");

        assert_eq!(repo.id, db_repo.id);
        assert_eq!(wt.id, inserted.id);
        assert!(
            wt.adopted_at.is_none(),
            "existing worktree should not be adopted"
        );
    }

    #[test]
    fn resolve_or_adopt_adopts_unmanaged_git_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // Register repo in DB but NOT the worktree
        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        db.insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();

        // Create a git worktree manually (not via trench)
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("ext-feature");
        git_repo
            .branch(
                "ext-feature",
                &git_repo.head().unwrap().peel_to_commit().unwrap(),
                false,
            )
            .unwrap();
        let branch_ref = git_repo
            .find_branch("ext-feature", git2::BranchType::Local)
            .unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch_ref.get()));
        git_repo
            .worktree("ext-feature", &wt_path, Some(&opts))
            .unwrap();

        // resolve_or_adopt should find via git and adopt
        let repo_info = git::discover_repo(repo_dir.path()).unwrap();
        let (_, wt) =
            resolve_or_adopt("ext-feature", &repo_info, &db).expect("should adopt unmanaged");

        assert!(wt.adopted_at.is_some(), "should have adopted_at set");
        assert!(wt.managed, "should be marked as managed");
        assert_eq!(wt.branch, "ext-feature");

        // Verify it's now in the DB
        let db_repo = db.get_repo_by_path(repo_path_str).unwrap().unwrap();
        let found = db
            .find_worktree_by_identifier(db_repo.id, "ext-feature")
            .unwrap();
        assert!(found.is_some(), "adopted worktree should be findable in DB");
    }

    #[test]
    fn resolve_or_adopt_creates_repo_when_not_in_db() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // Do NOT insert repo into DB — repo is completely unknown to trench

        // Create a git worktree manually
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("new-feature");
        git_repo
            .branch(
                "new-feature",
                &git_repo.head().unwrap().peel_to_commit().unwrap(),
                false,
            )
            .unwrap();
        let branch_ref = git_repo
            .find_branch("new-feature", git2::BranchType::Local)
            .unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch_ref.get()));
        git_repo
            .worktree("new-feature", &wt_path, Some(&opts))
            .unwrap();

        let repo_info = git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            resolve_or_adopt("new-feature", &repo_info, &db).expect("should create repo and adopt");

        // Repo should now exist in DB
        assert!(repo.id > 0);
        let repo_path_str = repo_dir.path().canonicalize().unwrap();
        let found_repo = db
            .get_repo_by_path(repo_path_str.to_str().unwrap())
            .unwrap();
        assert!(found_repo.is_some(), "repo should be created in DB");

        // Worktree should be adopted
        assert!(wt.adopted_at.is_some());
        assert!(wt.managed);
        assert_eq!(wt.branch, "new-feature");
    }

    #[test]
    fn resolve_or_adopt_not_found_returns_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        db.insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();

        let repo_info = git::discover_repo(repo_dir.path()).unwrap();
        let result = resolve_or_adopt("nonexistent", &repo_info, &db);
        let err = result.expect_err("should error for nonexistent worktree");
        assert!(
            err.to_string().contains("not found"),
            "error should mention 'not found', got: {}",
            err
        );
    }

    #[test]
    fn resolve_only_does_not_write_for_unmanaged_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // Do NOT insert repo or worktree into DB — completely unmanaged

        // Create a git worktree manually
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("virtual-feat");
        git_repo
            .branch(
                "virtual-feat",
                &git_repo.head().unwrap().peel_to_commit().unwrap(),
                false,
            )
            .unwrap();
        let branch_ref = git_repo
            .find_branch("virtual-feat", git2::BranchType::Local)
            .unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch_ref.get()));
        git_repo
            .worktree("virtual-feat", &wt_path, Some(&opts))
            .unwrap();

        let repo_info = git::discover_repo(repo_dir.path()).unwrap();

        // resolve_only should return virtual structs without writing to DB
        let (repo, wt) =
            resolve_only("virtual-feat", &repo_info, Some(&db)).expect("should resolve");

        // Virtual structs should have placeholder id=0
        assert_eq!(repo.id, 0, "virtual repo should have id=0");
        assert_eq!(wt.id, 0, "virtual worktree should have id=0");

        // Should carry correct info from git
        assert_eq!(repo.name, repo_info.name);
        assert_eq!(wt.branch, "virtual-feat");
        assert_eq!(wt.name, "virtual-feat");

        // DB must have NO repo or worktree rows
        let repo_path_str = repo_dir.path().canonicalize().unwrap();
        let found_repo = db
            .get_repo_by_path(repo_path_str.to_str().unwrap())
            .unwrap();
        assert!(
            found_repo.is_none(),
            "resolve_only must not insert repo into DB"
        );
    }

    #[test]
    fn resolve_only_returns_managed_worktree_from_db() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        let inserted = db
            .insert_worktree(
                db_repo.id,
                "my-feature",
                "my-feature",
                "/wt/my-feature",
                Some("main"),
            )
            .unwrap();

        let repo_info = git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            resolve_only("my-feature", &repo_info, Some(&db)).expect("should resolve existing");

        assert_eq!(repo.id, db_repo.id);
        assert_eq!(wt.id, inserted.id);
    }

    #[test]
    fn resolve_only_without_db_resolves_from_git() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());

        // Create a git worktree manually
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("no-db-feat");
        git_repo
            .branch(
                "no-db-feat",
                &git_repo.head().unwrap().peel_to_commit().unwrap(),
                false,
            )
            .unwrap();
        let branch_ref = git_repo
            .find_branch("no-db-feat", git2::BranchType::Local)
            .unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch_ref.get()));
        git_repo
            .worktree("no-db-feat", &wt_path, Some(&opts))
            .unwrap();

        let repo_info = git::discover_repo(repo_dir.path()).unwrap();

        // Call with db=None
        let (repo, wt) =
            resolve_only("no-db-feat", &repo_info, None).expect("should resolve from git");

        assert_eq!(repo.id, 0);
        assert_eq!(wt.id, 0);
        assert_eq!(wt.branch, "no-db-feat");
    }

    #[test]
    fn resolve_or_adopt_idempotent_on_already_adopted() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        db.insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();

        // Create a git worktree manually
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("idempotent-wt");
        git_repo
            .branch(
                "idempotent-wt",
                &git_repo.head().unwrap().peel_to_commit().unwrap(),
                false,
            )
            .unwrap();
        let branch_ref = git_repo
            .find_branch("idempotent-wt", git2::BranchType::Local)
            .unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch_ref.get()));
        git_repo
            .worktree("idempotent-wt", &wt_path, Some(&opts))
            .unwrap();

        let repo_info = git::discover_repo(repo_dir.path()).unwrap();

        // First call: adopts
        let (_, wt1) = resolve_or_adopt("idempotent-wt", &repo_info, &db).unwrap();
        assert!(wt1.adopted_at.is_some());

        // Second call: returns same DB record (no re-adoption)
        let (_, wt2) = resolve_or_adopt("idempotent-wt", &repo_info, &db).unwrap();
        assert_eq!(wt2.id, wt1.id, "should return same worktree");
        assert_eq!(
            wt2.adopted_at, wt1.adopted_at,
            "adopted_at should not change"
        );
    }
}
