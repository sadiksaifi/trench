use std::collections::BTreeSet;
use std::path::Path;

use crate::{git, worktree_catalog};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefCandidate {
    pub name: String,
    pub kind: RefKind,
}

impl RefCandidate {
    pub fn local(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: RefKind::Local,
        }
    }

    pub fn remote(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: RefKind::Remote,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefSnapshot {
    pub local: Vec<String>,
    pub origin: Vec<String>,
    pub origin_head: Option<String>,
    pub main_branch: Option<String>,
    pub has_origin: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DefaultBaseError {
    #[error("configured default base not found: {base}")]
    ConfiguredNotFound { base: String },
    #[error(
        "could not detect a default base; configure git.default_base or attach the main worktree to a branch"
    )]
    NotDetected,
    #[error("origin/HEAD points to a missing ref: {base}")]
    OriginHeadNotFound { base: String },
}

impl RefSnapshot {
    pub(crate) fn from_parts<L, R, LS, RS>(
        local: L,
        origin: R,
        origin_head: Option<&str>,
        main_branch: Option<&str>,
        has_origin: bool,
    ) -> Self
    where
        L: IntoIterator<Item = LS>,
        R: IntoIterator<Item = RS>,
        LS: Into<String>,
        RS: Into<String>,
    {
        let mut local = local.into_iter().map(Into::into).collect::<Vec<_>>();
        let mut origin = origin.into_iter().map(Into::into).collect::<Vec<_>>();
        local.sort();
        local.dedup();
        origin.sort();
        origin.dedup();
        Self {
            local,
            origin,
            origin_head: origin_head.map(ToOwned::to_owned),
            main_branch: main_branch.map(ToOwned::to_owned),
            has_origin,
        }
    }

    pub fn candidates(&self) -> Vec<RefCandidate> {
        let local = self.local.iter().cloned().collect::<BTreeSet<_>>();
        let mut candidates = local
            .iter()
            .cloned()
            .map(RefCandidate::local)
            .collect::<Vec<_>>();
        candidates.extend(
            self.origin
                .iter()
                .filter(|name| name.as_str() != "origin/HEAD")
                .filter(|name| {
                    name.strip_prefix("origin/")
                        .is_some_and(|short| !local.contains(short))
                })
                .cloned()
                .map(RefCandidate::remote),
        );
        candidates
    }

    pub fn default_base(&self, configured: Option<&str>) -> Result<String, DefaultBaseError> {
        if let Some(base) = configured {
            return self
                .resolve(base)
                .ok_or_else(|| DefaultBaseError::ConfiguredNotFound {
                    base: base.to_string(),
                });
        }
        if let Some(base) = self.origin_head.as_deref() {
            return self
                .resolve(base)
                .ok_or_else(|| DefaultBaseError::OriginHeadNotFound {
                    base: base.to_string(),
                });
        }
        self.main_branch
            .as_deref()
            .and_then(|base| self.resolve(base))
            .ok_or(DefaultBaseError::NotDetected)
    }

    pub fn resolve(&self, candidate: &str) -> Option<String> {
        if candidate.starts_with("origin/") {
            return self
                .origin
                .iter()
                .any(|name| name == candidate)
                .then(|| candidate.to_string());
        }
        if self.local.iter().any(|name| name == candidate) {
            return Some(candidate.to_string());
        }
        let remote = format!("origin/{candidate}");
        self.origin
            .iter()
            .any(|name| name == &remote)
            .then_some(remote)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RefCatalogError {
    #[error(transparent)]
    Discovery(#[from] git::GitError),
    #[error(transparent)]
    Catalog(#[from] worktree_catalog::CatalogError),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

pub struct RefCatalog;

impl RefCatalog {
    /// Snapshot refs already present in the repository. This never fetches.
    pub fn discover(cwd: &Path) -> Result<RefSnapshot, RefCatalogError> {
        let repo_info = git::discover_repo(cwd)?;
        let repo = git2::Repository::open(&repo_info.path)?;
        let local = branch_names(&repo, git2::BranchType::Local)?;
        let origin = branch_names(&repo, git2::BranchType::Remote)?
            .into_iter()
            .filter(|name| name.starts_with("origin/"))
            .collect::<Vec<_>>();
        let origin_head = match repo.find_reference("refs/remotes/origin/HEAD") {
            Ok(reference) => reference
                .symbolic_target()
                .and_then(|target| target.strip_prefix("refs/remotes/"))
                .map(ToOwned::to_owned),
            Err(error) if error.code() == git2::ErrorCode::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let main_branch = worktree_catalog::WorktreeCatalog::discover(cwd)?
            .identities()
            .iter()
            .find(|identity| identity.is_main && !identity.detached)
            .and_then(|identity| identity.branch.clone());
        let has_origin = repo.find_remote("origin").is_ok();
        Ok(RefSnapshot::from_parts(
            local,
            origin,
            origin_head.as_deref(),
            main_branch.as_deref(),
            has_origin,
        ))
    }

    /// Refresh origin only when the caller explicitly requests network access.
    pub fn fetch_origin(repo_path: &Path) -> Result<(), git::GitError> {
        git::fetch_remote(repo_path)
    }
}

fn branch_names(
    repo: &git2::Repository,
    kind: git2::BranchType,
) -> Result<Vec<String>, git2::Error> {
    let mut names = Vec::new();
    for branch in repo.branches(Some(kind))? {
        let (branch, _) = branch?;
        if let Some(name) = branch.name()? {
            names.push(name.to_string());
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(path: &std::path::Path) -> git2::Repository {
        let repo = git2::Repository::init(path).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        {
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .unwrap();
        }
        repo
    }

    #[test]
    fn candidates_deduplicate_origin_refs_with_local_preference() {
        let snapshot = RefSnapshot::from_parts(
            ["main", "release"],
            [
                "origin/HEAD",
                "origin/main",
                "origin/release",
                "origin/topic",
            ],
            Some("origin/main"),
            Some("main"),
            true,
        );

        assert_eq!(
            snapshot.candidates(),
            [
                RefCandidate::local("main"),
                RefCandidate::local("release"),
                RefCandidate::remote("origin/topic"),
            ]
        );
    }

    #[test]
    fn default_base_prefers_config_then_origin_head_then_main_branch() {
        let snapshot = RefSnapshot::from_parts(
            ["configured", "main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );

        assert_eq!(
            snapshot.default_base(Some("configured")).unwrap(),
            "configured"
        );
        assert_eq!(snapshot.default_base(None).unwrap(), "origin/main");

        let local_only =
            RefSnapshot::from_parts(["trunk"], [] as [&str; 0], None, Some("trunk"), false);
        assert_eq!(local_only.default_base(None).unwrap(), "trunk");
    }

    #[test]
    fn missing_configured_or_detected_base_is_actionable() {
        let snapshot = RefSnapshot::from_parts(["main"], [] as [&str; 0], None, None, false);

        assert_eq!(
            snapshot.default_base(Some("missing")),
            Err(DefaultBaseError::ConfiguredNotFound {
                base: "missing".to_string()
            })
        );
        assert_eq!(
            RefSnapshot::from_parts([] as [&str; 0], [] as [&str; 0], None, None, false)
                .default_base(None),
            Err(DefaultBaseError::NotDetected)
        );
    }

    #[test]
    fn discover_snapshots_live_local_and_origin_refs_without_fetching() {
        let root = tempfile::tempdir().unwrap();
        let repo = init_repo(root.path());
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("release", &head, false).unwrap();
        repo.remote("origin", "https://invalid.example/trench.git")
            .unwrap();
        repo.reference("refs/remotes/origin/main", head.id(), false, "test remote")
            .unwrap();
        repo.reference("refs/remotes/origin/topic", head.id(), false, "test remote")
            .unwrap();
        repo.reference_symbolic(
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
            false,
            "test remote head",
        )
        .unwrap();
        drop(head);
        drop(repo);

        let snapshot = RefCatalog::discover(root.path()).unwrap();

        assert_eq!(snapshot.local, ["main", "release"]);
        assert_eq!(
            snapshot.origin,
            ["origin/HEAD", "origin/main", "origin/topic"]
        );
        assert_eq!(snapshot.origin_head.as_deref(), Some("origin/main"));
        assert_eq!(snapshot.main_branch.as_deref(), Some("main"));
        assert!(snapshot.has_origin);
    }
}
