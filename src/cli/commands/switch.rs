use std::path::Path;

use crate::worktree_catalog::{CatalogError, WorktreeCatalog};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchResult {
    pub path: String,
    pub name: String,
}

pub fn execute(identifier: &str, cwd: &Path) -> Result<SwitchResult, CatalogError> {
    let catalog = WorktreeCatalog::discover(cwd)?;
    let identity = catalog.resolve(identifier)?;
    Ok(SwitchResult {
        path: identity.path.to_string_lossy().into_owned(),
        name: identity.worktree.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(path: &Path) -> git2::Repository {
        let repo = git2::Repository::init(path).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        {
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .unwrap();
        }
        repo
    }

    fn add_worktree(repo: &git2::Repository, branch: &str, path: &Path) {
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        let local = repo.branch(branch, &commit, false).unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(local.get()));
        repo.worktree("feature-auth", path, Some(&options)).unwrap();
    }

    #[test]
    fn switch_returns_the_live_path_without_product_state() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let linked = root.path().join("feature-auth");
        std::fs::create_dir(&main).unwrap();
        let repo = init_repo(&main);
        add_worktree(&repo, "feature/auth", &linked);

        let result = execute("feature/auth", &main).unwrap();

        assert_eq!(
            result.path,
            linked.canonicalize().unwrap().to_string_lossy()
        );
        assert_eq!(result.name, "feature-auth");
    }

    #[test]
    fn switch_reports_a_missing_live_worktree() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());

        assert!(matches!(
            execute("missing", root.path()),
            Err(CatalogError::NotFound { .. })
        ));
    }
}
