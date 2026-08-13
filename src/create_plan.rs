use std::path::{Path, PathBuf};

use crate::git;
use crate::ref_catalog::{DefaultBaseError, RefCatalog, RefCatalogError, RefSnapshot};
use crate::worktree_catalog::{CatalogError, WorktreeCatalog};
use crate::worktree_policy::{WorktreePolicy, WorktreePolicyError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPolicy {
    Run,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateAction {
    NewBranch(String),
    ExistingLocal,
    TrackRemote(String),
    Navigate(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreatePrecondition {
    ValidRef,
    ClassifiedRef,
    AvailableIdentity,
    AvailableAdminName,
    AvailableBranch,
    AvailablePath,
    ContainedByRoot,
    ExistingWorktreeResolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePlan {
    pub dry_run: bool,
    pub action: CreateAction,
    pub branch: String,
    pub worktree: String,
    pub path: PathBuf,
    pub base: Option<String>,
    pub tracking: Option<String>,
    pub hook_policy: HookPolicy,
    pub preconditions: Vec<CreatePrecondition>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreatePlanError {
    #[error(transparent)]
    Git(#[from] git::GitError),
    #[error(transparent)]
    RefCatalog(#[from] RefCatalogError),
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Policy(#[from] WorktreePolicyError),
    #[error(transparent)]
    DefaultBase(#[from] DefaultBaseError),
    #[error(transparent)]
    GitState(#[from] git2::Error),
    #[error("--from is only valid when creating a new branch; '{selection}' already exists")]
    FromRequiresNewBranch { selection: String },
    #[error("branch '{branch}' is checked out in multiple worktrees: {paths:?}")]
    CheckedOutBranchAmbiguous { branch: String, paths: Vec<PathBuf> },
    #[error("base ref not found: {base}")]
    BaseNotFound { base: String },
}

enum RefClass<'a> {
    New(&'a str),
    Local(&'a str),
    Remote { branch: &'a str, upstream: String },
}

impl RefClass<'_> {
    fn branch(&self) -> &str {
        match self {
            Self::New(branch) | Self::Local(branch) | Self::Remote { branch, .. } => branch,
        }
    }
}

pub struct CreatePlanner {
    repo_info: git::RepoInfo,
    refs: RefSnapshot,
    catalog: WorktreeCatalog,
    policy: WorktreePolicy,
    configured_base: Option<String>,
    hook_policy: HookPolicy,
}

impl CreatePlanner {
    pub fn discover(
        cwd: &Path,
        worktree_root: &Path,
        configured_base: Option<&str>,
        hook_policy: HookPolicy,
    ) -> Result<Self, CreatePlanError> {
        let repo_info = git::discover_repo(cwd)?;
        let refs = RefCatalog::discover(cwd)?;
        let catalog = WorktreeCatalog::discover(cwd)?;
        let policy = WorktreePolicy::from_catalog(
            worktree_root,
            &repo_info.name,
            &catalog,
            &repo_info.path,
        )?;
        Ok(Self {
            repo_info,
            refs,
            catalog,
            policy,
            configured_base: configured_base.map(ToOwned::to_owned),
            hook_policy,
        })
    }

    pub fn plan(&self, branch: &str, from: Option<&str>) -> Result<CreatePlan, CreatePlanError> {
        self.policy.validate(branch)?;
        let class = self.classify(branch);
        self.policy.validate(class.branch())?;
        if from.is_some() && !matches!(class, RefClass::New(_)) {
            return Err(CreatePlanError::FromRequiresNewBranch {
                selection: branch.to_string(),
            });
        }
        let checked_out = self
            .catalog
            .identities()
            .iter()
            .filter(|identity| identity.branch.as_deref() == Some(class.branch()))
            .collect::<Vec<_>>();
        if checked_out.len() > 1 {
            return Err(CreatePlanError::CheckedOutBranchAmbiguous {
                branch: class.branch().to_string(),
                paths: checked_out
                    .iter()
                    .map(|identity| identity.path.clone())
                    .collect(),
            });
        }
        if let Some(existing) = checked_out.first() {
            return Ok(CreatePlan {
                dry_run: true,
                action: CreateAction::Navigate(existing.worktree.clone()),
                branch: class.branch().to_string(),
                worktree: existing.worktree.clone(),
                path: existing.path.clone(),
                base: None,
                tracking: None,
                hook_policy: self.hook_policy,
                preconditions: vec![
                    CreatePrecondition::ValidRef,
                    CreatePrecondition::ClassifiedRef,
                    CreatePrecondition::ExistingWorktreeResolved,
                ],
            });
        }
        let location = self.policy.derive(class.branch())?;
        let (action, base, tracking) = match class {
            RefClass::New(_) => {
                let base =
                    match from {
                        Some(base) => self.refs.resolve(base).ok_or_else(|| {
                            CreatePlanError::BaseNotFound {
                                base: base.to_string(),
                            }
                        })?,
                        None => self.refs.default_base(self.configured_base.as_deref())?,
                    };
                (CreateAction::NewBranch(base.clone()), Some(base), None)
            }
            RefClass::Local(_) => (CreateAction::ExistingLocal, None, None),
            RefClass::Remote { upstream, .. } => (
                CreateAction::TrackRemote(upstream.clone()),
                None,
                Some(upstream),
            ),
        };
        Ok(CreatePlan {
            dry_run: true,
            action,
            branch: location.branch,
            worktree: location.worktree,
            path: location.path,
            base,
            tracking,
            hook_policy: self.hook_policy,
            preconditions: vec![
                CreatePrecondition::ValidRef,
                CreatePrecondition::ClassifiedRef,
                CreatePrecondition::AvailableIdentity,
                CreatePrecondition::AvailableAdminName,
                CreatePrecondition::AvailableBranch,
                CreatePrecondition::AvailablePath,
                CreatePrecondition::ContainedByRoot,
            ],
        })
    }

    fn classify<'a>(&self, selection: &'a str) -> RefClass<'a> {
        if let Some(branch) = selection.strip_prefix("origin/") {
            if self.refs.local.iter().any(|local| local == branch) {
                return RefClass::Local(branch);
            }
            if self.refs.origin.iter().any(|remote| remote == selection) {
                return RefClass::Remote {
                    branch,
                    upstream: selection.to_string(),
                };
            }
        }
        if self.refs.local.iter().any(|local| local == selection) {
            return RefClass::Local(selection);
        }
        let upstream = format!("origin/{selection}");
        if self.refs.origin.iter().any(|remote| remote == &upstream) {
            return RefClass::Remote {
                branch: selection,
                upstream,
            };
        }
        RefClass::New(selection)
    }

    pub fn repository(&self) -> &git::RepoInfo {
        &self.repo_info
    }

    pub fn refs(&self) -> &RefSnapshot {
        &self.refs
    }

    pub fn catalog(&self) -> &WorktreeCatalog {
        &self.catalog
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(path: &std::path::Path) {
        let repo = git2::Repository::init(path).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
            .unwrap();
    }

    #[test]
    fn new_branch_plan_is_repository_scoped_and_read_only() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = outside.path().join("worktrees");
        init_repo(repo.path());
        let repository = repo.path().file_name().unwrap().to_string_lossy();
        let planner = CreatePlanner::discover(repo.path(), &root, None, HookPolicy::Run).unwrap();

        let plan = planner.plan("feature/auth", None).unwrap();

        assert_eq!(plan.action, CreateAction::NewBranch("main".to_string()));
        assert_eq!(plan.branch, "feature/auth");
        assert_eq!(plan.worktree, "feature-auth");
        assert_eq!(
            plan.path,
            root.join(repository.as_ref()).join("feature-auth")
        );
        assert_eq!(plan.base.as_deref(), Some("main"));
        assert_eq!(plan.tracking, None);
        assert_eq!(plan.hook_policy, HookPolicy::Run);
        assert!(!root.exists(), "planning must not create the worktree root");
    }

    #[test]
    fn existing_local_branch_has_no_base_or_tracking() {
        let repo = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let repository = git2::Repository::open(repo.path()).unwrap();
        let head = repository.head().unwrap().peel_to_commit().unwrap();
        repository.branch("release", &head, false).unwrap();
        drop(head);
        drop(repository);
        let planner =
            CreatePlanner::discover(repo.path(), root.path(), None, HookPolicy::Run).unwrap();

        let plan = planner.plan("release", None).unwrap();

        assert_eq!(plan.action, CreateAction::ExistingLocal);
        assert_eq!(plan.branch, "release");
        assert_eq!(plan.base, None);
        assert_eq!(plan.tracking, None);
    }

    #[test]
    fn remote_only_selection_plans_a_local_tracking_branch() {
        let repo = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let repository = git2::Repository::open(repo.path()).unwrap();
        let head = repository.head().unwrap().target().unwrap();
        repository
            .reference(
                "refs/remotes/origin/release",
                head,
                false,
                "test remote ref",
            )
            .unwrap();
        drop(repository);
        let planner =
            CreatePlanner::discover(repo.path(), root.path(), None, HookPolicy::Run).unwrap();

        let plan = planner.plan("origin/release", None).unwrap();

        assert_eq!(
            plan.action,
            CreateAction::TrackRemote("origin/release".to_string())
        );
        assert_eq!(plan.branch, "release");
        assert_eq!(plan.worktree, "release");
        assert_eq!(plan.base, None);
        assert_eq!(plan.tracking.as_deref(), Some("origin/release"));
    }

    #[test]
    fn checked_out_branch_navigates_before_derived_path_collision() {
        let repo = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let repository = git2::Repository::open(repo.path()).unwrap();
        let head = repository.head().unwrap().peel_to_commit().unwrap();
        let branch = repository.branch("feature/auth", &head, false).unwrap();
        let repository_name = repo.path().file_name().unwrap().to_string_lossy();
        let target = root
            .path()
            .join(repository_name.as_ref())
            .join("feature-auth");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(branch.get()));
        repository
            .worktree("feature-auth", &target, Some(&options))
            .unwrap();
        drop(branch);
        drop(head);
        drop(repository);
        let planner =
            CreatePlanner::discover(repo.path(), root.path(), None, HookPolicy::Run).unwrap();

        let plan = planner.plan("feature/auth", None).unwrap();

        assert_eq!(
            plan.action,
            CreateAction::Navigate("feature-auth".to_string())
        );
        assert_eq!(plan.path, target.canonicalize().unwrap());
        assert_eq!(plan.base, None);
        assert_eq!(plan.tracking, None);
    }

    #[test]
    fn from_is_rejected_for_non_new_branches() {
        let repo = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let repository = git2::Repository::open(repo.path()).unwrap();
        let head = repository.head().unwrap().peel_to_commit().unwrap();
        repository.branch("release", &head, false).unwrap();
        drop(head);
        drop(repository);
        let planner =
            CreatePlanner::discover(repo.path(), root.path(), None, HookPolicy::Run).unwrap();

        assert!(matches!(
            planner.plan("release", Some("main")),
            Err(CreatePlanError::FromRequiresNewBranch { .. })
        ));
    }

    #[test]
    fn missing_explicit_base_is_reported_as_a_base_error() {
        let repo = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let planner =
            CreatePlanner::discover(repo.path(), root.path(), None, HookPolicy::Run).unwrap();

        assert!(matches!(
            planner.plan("feature", Some("missing")),
            Err(CreatePlanError::BaseNotFound { base }) if base == "missing"
        ));
    }
}
