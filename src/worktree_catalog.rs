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

pub struct WorktreeCatalog {
    identities: Vec<WorktreeIdentity>,
    base: Option<String>,
}

impl WorktreeCatalog {
    pub fn discover(cwd: &Path) -> Result<Self, CatalogError> {
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
        let main = identities.iter().find(|identity| identity.is_main);
        let base = main
            .and_then(|identity| git::status::detected_base(&identity.path).ok().flatten())
            .or_else(|| main.and_then(|identity| identity.branch.clone()));
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
        let counts = git::status::counts(id)?;
        let comparison =
            git::status::ahead_behind(id, identity.head.as_deref(), self.base.as_deref())?;
        Ok(WorktreeStatus {
            base: self.base.clone(),
            staged: counts.staged,
            modified: counts.modified,
            untracked: counts.untracked,
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
