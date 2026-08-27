use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::git;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeIdentity {
    pub worktree: String,
    pub branch: Option<String>,
    pub path: PathBuf,
    pub is_main: bool,
    pub is_current: bool,
    pub detached: bool,
    pub(crate) head: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeStatus {
    pub base: Option<String>,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub conflicted: u32,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeRecord {
    pub worktree: String,
    pub branch: Option<String>,
    pub path: String,
    pub is_main: bool,
    pub is_current: bool,
    pub detached: bool,
    pub base: Option<String>,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error(transparent)]
    Git(#[from] git::GitError),
    #[error("worktree not found: {selector}")]
    NotFound { selector: String },
    #[error("worktree selector is ambiguous: {selector}; matches: {matches:?}")]
    Ambiguous {
        selector: String,
        matches: Vec<PathBuf>,
    },
}

pub trait BaseRefResolver {
    fn base_for(&self, main: &WorktreeIdentity) -> Result<Option<String>, git::GitError>;
}

struct DetectedBaseRef;

impl BaseRefResolver for DetectedBaseRef {
    fn base_for(&self, main: &WorktreeIdentity) -> Result<Option<String>, git::GitError> {
        Ok(git::status::detected_base(&main.path)?.or_else(|| main.branch.clone()))
    }
}

pub struct WorktreeCatalog {
    identities: Vec<WorktreeIdentity>,
    base: Option<String>,
}

impl WorktreeCatalog {
    pub fn discover(cwd: &Path) -> Result<Self, CatalogError> {
        Self::discover_with_base(cwd, &DetectedBaseRef)
    }

    fn discover_with_base(
        cwd: &Path,
        base_resolver: &impl BaseRefResolver,
    ) -> Result<Self, CatalogError> {
        let mut identities: Vec<_> = git::worktrees::discover(cwd)?
            .into_iter()
            .map(|entry| {
                let worktree = if entry.detached {
                    format!(
                        "detached@{}",
                        entry
                            .head
                            .as_deref()
                            .unwrap_or("unknown")
                            .chars()
                            .take(7)
                            .collect::<String>()
                    )
                } else {
                    entry
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| entry.path.to_string_lossy().into_owned())
                };
                WorktreeIdentity {
                    worktree,
                    branch: entry.branch,
                    path: entry.path,
                    is_main: entry.is_main,
                    is_current: entry.is_current,
                    detached: entry.detached,
                    head: entry.head,
                }
            })
            .collect();

        identities.sort_by(|left, right| {
            right
                .is_current
                .cmp(&left.is_current)
                .then_with(|| left.worktree.cmp(&right.worktree))
                .then_with(|| left.path.cmp(&right.path))
        });
        let base = identities
            .iter()
            .find(|identity| identity.is_main)
            .map(|identity| base_resolver.base_for(identity))
            .transpose()?
            .flatten();
        Ok(Self { identities, base })
    }

    pub fn with_base(mut self, base: Option<&str>) -> Self {
        if let Some(base) = base {
            self.base = Some(base.to_string());
        }
        self
    }

    pub fn identities(&self) -> &[WorktreeIdentity] {
        &self.identities
    }

    pub fn status(&self, id: &Path) -> Result<WorktreeStatus, CatalogError> {
        let Some(identity) = self.identities.iter().find(|identity| identity.path == id) else {
            return Err(CatalogError::NotFound {
                selector: id.to_string_lossy().into_owned(),
            });
        };
        if !id.exists() {
            return Ok(WorktreeStatus {
                base: self.base.clone(),
                ..WorktreeStatus::default()
            });
        }
        let counts = git::status::counts(id)?;
        let comparison =
            git::status::ahead_behind(id, identity.head.as_deref(), self.base.as_deref())?;
        Ok(WorktreeStatus {
            base: self.base.clone(),
            staged: counts.staged,
            modified: counts.modified,
            untracked: counts.untracked,
            conflicted: counts.conflicted,
            ahead: comparison.map(|(ahead, _)| ahead),
            behind: comparison.map(|(_, behind)| behind),
        })
    }

    pub fn records(&self) -> Result<Vec<WorktreeRecord>, CatalogError> {
        self.identities
            .iter()
            .map(|identity| {
                let status = self.status(&identity.path)?;
                Ok(WorktreeRecord {
                    worktree: identity.worktree.clone(),
                    branch: identity.branch.clone(),
                    path: identity.path.to_string_lossy().into_owned(),
                    is_main: identity.is_main,
                    is_current: identity.is_current,
                    detached: identity.detached,
                    base: status.base,
                    staged: status.staged,
                    modified: status.modified,
                    untracked: status.untracked,
                    ahead: status.ahead,
                    behind: status.behind,
                })
            })
            .collect()
    }

    pub fn resolve(&self, selector: &str) -> Result<&WorktreeIdentity, CatalogError> {
        let selector_path = Path::new(selector).is_absolute().then(|| {
            Path::new(selector)
                .canonicalize()
                .unwrap_or_else(|_| selector.into())
        });
        let mut matches = BTreeMap::new();
        for identity in &self.identities {
            if identity.worktree == selector
                || identity.branch.as_deref() == Some(selector)
                || selector_path.as_ref() == Some(&identity.path)
            {
                matches.insert(identity.path.clone(), identity);
            }
        }
        match matches.len() {
            0 => Err(CatalogError::NotFound {
                selector: selector.to_string(),
            }),
            1 => Ok(*matches.values().next().unwrap()),
            _ => Err(CatalogError::Ambiguous {
                selector: selector.to_string(),
                matches: matches.into_keys().collect(),
            }),
        }
    }
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

    fn add_worktree(repo: &git2::Repository, name: &str, branch: &str, path: &Path) {
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        let local = repo.branch(branch, &commit, false).unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(local.get()));
        repo.worktree(name, path, Some(&options)).unwrap();
    }

    #[test]
    fn resolve_accepts_exact_identity_branch_and_canonical_path() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let linked = root.path().join("feature-auth");
        std::fs::create_dir(&main).unwrap();
        let repo = init_repo(&main);
        add_worktree(&repo, "feature-auth", "feature/auth", &linked);

        let catalog = WorktreeCatalog::discover(&linked).unwrap();
        let by_identity = catalog.resolve("feature-auth").unwrap();
        let by_branch = catalog.resolve("feature/auth").unwrap();
        let canonical = linked.canonicalize().unwrap();
        let by_path = catalog.resolve(canonical.to_str().unwrap()).unwrap();

        assert_eq!(by_identity.path, canonical);
        assert_eq!(by_branch.path, canonical);
        assert_eq!(by_path.path, canonical);
        assert!(matches!(
            catalog.resolve("missing"),
            Err(CatalogError::NotFound { .. })
        ));
    }

    #[test]
    fn duplicate_worktree_identities_are_ambiguous() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let first = root.path().join("one").join("shared");
        let second = root.path().join("two").join("shared");
        std::fs::create_dir(&main).unwrap();
        std::fs::create_dir(first.parent().unwrap()).unwrap();
        std::fs::create_dir(second.parent().unwrap()).unwrap();
        let repo = init_repo(&main);
        add_worktree(&repo, "first", "feature/one", &first);
        add_worktree(&repo, "second", "feature/two", &second);

        let catalog = WorktreeCatalog::discover(&main).unwrap();
        assert!(matches!(
            catalog.resolve("shared"),
            Err(CatalogError::Ambiguous { .. })
        ));
    }

    #[test]
    fn current_is_first_then_identities_are_alphabetical() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let alpha = root.path().join("alpha");
        let zebra = root.path().join("zebra");
        std::fs::create_dir(&main).unwrap();
        let repo = init_repo(&main);
        add_worktree(&repo, "zebra", "feature/zebra", &zebra);
        add_worktree(&repo, "alpha", "feature/alpha", &alpha);

        let catalog = WorktreeCatalog::discover(&zebra).unwrap();
        let names: Vec<_> = catalog
            .identities()
            .iter()
            .map(|identity| identity.worktree.as_str())
            .collect();

        assert_eq!(names, ["zebra", "alpha", "main"]);
    }
}
