use std::fmt;
use std::path::{Path, PathBuf};

use serde::ser::{Serialize, SerializeStruct, Serializer};

use crate::git;
use crate::ref_catalog::{DefaultBaseError, RefCatalog, RefCatalogError, RefKind, RefSnapshot};
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

impl CreateAction {
    pub fn name(&self) -> &'static str {
        match self {
            Self::NewBranch(_) => "new_branch",
            Self::ExistingLocal => "existing_local",
            Self::TrackRemote(_) => "track_remote",
            Self::Navigate(_) => "navigate",
        }
    }
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
    /// The exact commit selected by planning. This is intentionally omitted
    /// from the stable human/JSON surface but participates in revalidation.
    pub source_oid: Option<git2::Oid>,
}

impl Serialize for CreatePlan {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut output = serializer.serialize_struct("CreatePlan", 8)?;
        output.serialize_field("dry_run", &self.dry_run)?;
        output.serialize_field("action", self.action.name())?;
        output.serialize_field("branch", &self.branch)?;
        output.serialize_field("worktree", &self.worktree)?;
        output.serialize_field("path", &self.path)?;
        output.serialize_field("base", &self.base)?;
        output.serialize_field("tracking", &self.tracking)?;
        output.serialize_field("hook_policy", &self.hook_policy)?;
        output.end()
    }
}

impl fmt::Display for CreatePlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let base = self.base.as_deref().unwrap_or("(none)");
        let tracking = self.tracking.as_deref().unwrap_or("(none)");
        let hook_policy = match self.hook_policy {
            HookPolicy::Run => "run",
            HookPolicy::Skip => "skip",
        };
        writeln!(f, "Dry run — no changes will be made\n")?;
        writeln!(f, "  Action:       {}", self.action.name())?;
        writeln!(f, "  Branch:       {}", self.branch)?;
        writeln!(f, "  Worktree:     {}", self.worktree)?;
        writeln!(f, "  Path:         {}", self.path.display())?;
        writeln!(f, "  Base:         {base}")?;
        writeln!(f, "  Tracking:     {tracking}")?;
        writeln!(f, "  Hook policy:  {hook_policy}")
    }
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
    repo_path: PathBuf,
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
            repo_path: repo_info.path,
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
                source_oid: None,
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
        let source_name = match &action {
            CreateAction::NewBranch(base) => Some(base.as_str()),
            CreateAction::ExistingLocal => Some(location.branch.as_str()),
            CreateAction::TrackRemote(upstream) => Some(upstream.as_str()),
            CreateAction::Navigate(_) => None,
        };
        let source_oid = source_name
            .map(|name| self.resolve_source_oid(name))
            .transpose()?;
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
            source_oid,
        })
    }

    fn resolve_source_oid(&self, name: &str) -> Result<git2::Oid, CreatePlanError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        let reference = if name.starts_with("origin/") {
            format!("refs/remotes/{name}")
        } else {
            format!("refs/heads/{name}")
        };
        let object = repo.revparse_single(&reference)?;
        let oid = object.peel_to_commit()?.id();
        Ok(oid)
    }

    fn classify<'a>(&self, selection: &'a str) -> RefClass<'a> {
        let candidates = self.refs.candidates();
        if let Some(branch) = selection.strip_prefix("origin/") {
            if candidates
                .iter()
                .any(|candidate| candidate.kind == RefKind::Local && candidate.name == branch)
            {
                return RefClass::Local(branch);
            }
            if candidates
                .iter()
                .any(|candidate| candidate.kind == RefKind::Remote && candidate.name == selection)
            {
                return RefClass::Remote {
                    branch,
                    upstream: selection.to_string(),
                };
            }
        }
        if candidates
            .iter()
            .any(|candidate| candidate.kind == RefKind::Local && candidate.name == selection)
        {
            return RefClass::Local(selection);
        }
        let upstream = format!("origin/{selection}");
        if candidates
            .iter()
            .any(|candidate| candidate.kind == RefKind::Remote && candidate.name == upstream)
        {
            return RefClass::Remote {
                branch: selection,
                upstream,
            };
        }
        RefClass::New(selection)
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
        repository
            .reference(
                "refs/remotes/origin/release",
                head.id(),
                false,
                "test remote ref",
            )
            .unwrap();
        drop(head);
        drop(repository);
        let planner =
            CreatePlanner::discover(repo.path(), root.path(), None, HookPolicy::Run).unwrap();

        let plan = planner.plan("release", None).unwrap();

        assert_eq!(plan.action, CreateAction::ExistingLocal);
        assert_eq!(plan.branch, "release");
        assert_eq!(plan.base, None);
        assert_eq!(plan.tracking, None);

        let explicit_origin = planner.plan("origin/release", None).unwrap();
        assert_eq!(explicit_origin.action, CreateAction::ExistingLocal);
        assert_eq!(explicit_origin.branch, "release");
        assert_eq!(explicit_origin.tracking, None);
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

    #[cfg(unix)]
    #[test]
    fn checked_out_branch_navigates_before_unused_symlinked_path_validation() {
        let repo = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let escape = tempfile::tempdir().unwrap();
        let linked = tempfile::tempdir().unwrap();
        let target = linked.path().join("feature-auth");
        init_repo(repo.path());
        let repository = git2::Repository::open(repo.path()).unwrap();
        let head = repository.head().unwrap().peel_to_commit().unwrap();
        let branch = repository.branch("feature/auth", &head, false).unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(branch.get()));
        repository
            .worktree("feature-auth", &target, Some(&options))
            .unwrap();
        drop(branch);
        drop(head);
        drop(repository);
        let repository_name = repo.path().file_name().unwrap().to_string_lossy();
        std::os::unix::fs::symlink(escape.path(), root.path().join(repository_name.as_ref()))
            .unwrap();
        let planner =
            CreatePlanner::discover(repo.path(), root.path(), None, HookPolicy::Run).unwrap();

        let plan = planner.plan("feature/auth", None).unwrap();

        assert_eq!(
            plan.action,
            CreateAction::Navigate("feature-auth".to_string())
        );
        assert_eq!(plan.path, target.canonicalize().unwrap());
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

    #[test]
    fn json_preview_has_only_the_frozen_flat_contract() {
        let plan = CreatePlan {
            dry_run: true,
            action: CreateAction::NewBranch("main".to_string()),
            branch: "feature/auth".to_string(),
            worktree: "feature-auth".to_string(),
            path: PathBuf::from("/worktrees/trench/feature-auth"),
            base: Some("main".to_string()),
            tracking: None,
            hook_policy: HookPolicy::Run,
            preconditions: vec![CreatePrecondition::ValidRef],
            source_oid: None,
        };

        assert_eq!(
            serde_json::to_value(&plan).unwrap(),
            serde_json::json!({
                "dry_run": true,
                "action": "new_branch",
                "branch": "feature/auth",
                "worktree": "feature-auth",
                "path": "/worktrees/trench/feature-auth",
                "base": "main",
                "tracking": null,
                "hook_policy": "run"
            })
        );
    }

    #[test]
    fn human_preview_reports_raw_plan_fields() {
        let plan = CreatePlan {
            dry_run: true,
            action: CreateAction::TrackRemote("origin/release".to_string()),
            branch: "release".to_string(),
            worktree: "release".to_string(),
            path: PathBuf::from("/worktrees/trench/release"),
            base: None,
            tracking: Some("origin/release".to_string()),
            hook_policy: HookPolicy::Skip,
            preconditions: Vec::new(),
            source_oid: None,
        };

        assert_eq!(
            plan.to_string(),
            "Dry run — no changes will be made\n\n  Action:       track_remote\n  Branch:       release\n  Worktree:     release\n  Path:         /worktrees/trench/release\n  Base:         (none)\n  Tracking:     origin/release\n  Hook policy:  skip\n"
        );
    }
}
