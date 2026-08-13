use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeLocation {
    pub branch: String,
    pub worktree: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingWorktree {
    pub worktree: String,
    pub branch: Option<String>,
    pub path: PathBuf,
}

impl ExistingWorktree {
    #[cfg(test)]
    pub fn new(
        worktree: impl Into<String>,
        branch: Option<impl Into<String>>,
        path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            worktree: worktree.into(),
            branch: branch.map(Into::into),
            path: path.into(),
        }
    }
}

impl From<&crate::worktree_catalog::WorktreeIdentity> for ExistingWorktree {
    fn from(identity: &crate::worktree_catalog::WorktreeIdentity) -> Self {
        Self {
            worktree: identity.worktree.clone(),
            branch: identity.branch.clone(),
            path: identity.path.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorktreePolicyError {
    #[error("invalid branch '{branch}': {reason}")]
    InvalidBranch { branch: String, reason: String },
    #[error("derived worktree path escapes its configured root: {path}")]
    RootEscape { path: PathBuf },
    #[error("branch '{branch}' does not produce a usable worktree identity")]
    InvalidIdentity { branch: String },
    #[error("worktree identity '{worktree}' is already used by {path}")]
    IdentityConflict { worktree: String, path: PathBuf },
    #[error("Git worktree admin name already exists: {worktree}")]
    AdminNameConflict { worktree: String },
    #[error("branch '{branch}' is already checked out at {path}")]
    BranchCheckedOut { branch: String, path: PathBuf },
    #[error("worktree path already exists: {path}")]
    PathExists { path: PathBuf },
}

pub struct WorktreePolicy {
    root: PathBuf,
    repository: String,
    existing: Vec<ExistingWorktree>,
    admin_names: BTreeSet<String>,
}

impl WorktreePolicy {
    pub fn new(root: &Path, repository: impl Into<String>) -> Self {
        Self {
            root: absolute_lexical(root),
            repository: repository.into(),
            existing: Vec::new(),
            admin_names: BTreeSet::new(),
        }
    }

    pub fn from_catalog(
        root: &Path,
        repository: impl Into<String>,
        catalog: &crate::worktree_catalog::WorktreeCatalog,
        repository_path: &Path,
    ) -> Result<Self, git2::Error> {
        let repo = git2::Repository::open(repository_path)?;
        let admin_names = repo
            .worktrees()?
            .iter()
            .flatten()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        Ok(Self::new(root, repository)
            .with_existing(catalog.identities().iter().map(ExistingWorktree::from))
            .with_admin_names(admin_names))
    }

    pub fn with_existing(mut self, existing: impl IntoIterator<Item = ExistingWorktree>) -> Self {
        self.existing = existing.into_iter().collect();
        self
    }

    pub fn with_admin_names<S: Into<String>>(mut self, names: impl IntoIterator<Item = S>) -> Self {
        self.admin_names = names.into_iter().map(Into::into).collect();
        self
    }

    pub fn derive(&self, branch: &str) -> Result<WorktreeLocation, WorktreePolicyError> {
        let location = self.location(branch)?;
        let WorktreeLocation {
            branch,
            worktree,
            path,
        } = location;
        if let Some(existing) = self
            .existing
            .iter()
            .find(|existing| existing.worktree == worktree)
        {
            return Err(WorktreePolicyError::IdentityConflict {
                worktree,
                path: existing.path.clone(),
            });
        }
        if self.admin_names.contains(&worktree) {
            return Err(WorktreePolicyError::AdminNameConflict { worktree });
        }
        if let Some(existing) = self
            .existing
            .iter()
            .find(|existing| existing.branch.as_deref() == Some(&branch))
        {
            return Err(WorktreePolicyError::BranchCheckedOut {
                branch,
                path: existing.path.clone(),
            });
        }
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(WorktreePolicyError::PathExists { path });
        }
        Ok(WorktreeLocation {
            branch,
            worktree,
            path,
        })
    }

    pub fn validate(&self, branch: &str) -> Result<(), WorktreePolicyError> {
        self.location(branch).map(|_| ())
    }

    fn location(&self, branch: &str) -> Result<WorktreeLocation, WorktreePolicyError> {
        validate_branch(branch)?;
        let worktree = paths::sanitize_branch(branch);
        if worktree.is_empty() || worktree == "." || worktree == ".." {
            return Err(WorktreePolicyError::InvalidIdentity {
                branch: branch.to_string(),
            });
        }
        let path = absolute_lexical(&self.root.join(&self.repository).join(&worktree));
        if !is_single_segment(&self.repository) || !path.starts_with(&self.root) {
            return Err(WorktreePolicyError::RootEscape { path });
        }
        Ok(WorktreeLocation {
            branch: branch.to_string(),
            worktree,
            path,
        })
    }
}

fn validate_branch(branch: &str) -> Result<(), WorktreePolicyError> {
    paths::validate_branch_name(branch).map_err(|reason| WorktreePolicyError::InvalidBranch {
        branch: branch.to_string(),
        reason,
    })?;
    if !git2::Reference::is_valid_name(&format!("refs/heads/{branch}")) {
        return Err(WorktreePolicyError::InvalidBranch {
            branch: branch.to_string(),
            reason: "not a valid Git ref".to_string(),
        });
    }
    Ok(())
}

fn is_single_segment(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn absolute_lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_branch_keeps_git_ref_and_derives_repository_scoped_identity() {
        let root = tempfile::tempdir().unwrap();
        let policy = WorktreePolicy::new(root.path(), "trench");

        let location = policy.derive("feature/auth").unwrap();

        assert_eq!(location.branch, "feature/auth");
        assert_eq!(location.worktree, "feature-auth");
        assert_eq!(
            location.path,
            root.path().join("trench").join("feature-auth")
        );
    }

    #[test]
    fn sanitized_identity_collision_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let policy =
            WorktreePolicy::new(root.path(), "trench").with_existing([ExistingWorktree::new(
                "feature-auth",
                Some("different/branch"),
                "/existing",
            )]);

        assert_eq!(
            policy.derive("feature/auth"),
            Err(WorktreePolicyError::IdentityConflict {
                worktree: "feature-auth".to_string(),
                path: PathBuf::from("/existing")
            })
        );
    }

    #[test]
    fn git_admin_name_collision_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let policy = WorktreePolicy::new(root.path(), "trench").with_admin_names(["feature-auth"]);

        assert_eq!(
            policy.derive("feature/auth"),
            Err(WorktreePolicyError::AdminNameConflict {
                worktree: "feature-auth".to_string()
            })
        );
    }

    #[test]
    fn checked_out_branch_collision_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let policy =
            WorktreePolicy::new(root.path(), "trench").with_existing([ExistingWorktree::new(
                "custom-name",
                Some("feature/auth"),
                "/checked-out",
            )]);

        assert_eq!(
            policy.derive("feature/auth"),
            Err(WorktreePolicyError::BranchCheckedOut {
                branch: "feature/auth".to_string(),
                path: PathBuf::from("/checked-out")
            })
        );
    }

    #[test]
    fn existing_derived_path_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trench").join("feature-auth");
        std::fs::create_dir_all(&path).unwrap();
        let policy = WorktreePolicy::new(root.path(), "trench");

        assert_eq!(
            policy.derive("feature/auth"),
            Err(WorktreePolicyError::PathExists { path })
        );
    }

    #[test]
    fn invalid_ref_and_traversal_fail_before_path_collisions() {
        let root = tempfile::tempdir().unwrap();
        let collision = root.path().join("trench").join("feature-secret");
        std::fs::create_dir_all(collision).unwrap();
        let policy = WorktreePolicy::new(root.path(), "trench");

        assert!(matches!(
            policy.derive("feature/../secret"),
            Err(WorktreePolicyError::InvalidBranch { .. })
        ));
    }

    #[test]
    fn repository_traversal_cannot_escape_the_configured_root() {
        let root = tempfile::tempdir().unwrap();
        let policy = WorktreePolicy::new(root.path(), "../outside");

        assert!(matches!(
            policy.derive("feature/auth"),
            Err(WorktreePolicyError::RootEscape { .. })
        ));
    }

    #[test]
    fn empty_sanitized_identity_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let policy = WorktreePolicy::new(root.path(), "trench");

        assert_eq!(
            policy.derive("@"),
            Err(WorktreePolicyError::InvalidIdentity {
                branch: "@".to_string()
            })
        );
    }
}
