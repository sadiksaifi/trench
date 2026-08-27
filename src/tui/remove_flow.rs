use std::path::Path;

use crate::{
    cli::commands::remove::stateless::{
        RemovalAssessment, RemovalAssessmentError, RemovalAuthorizationError, RemoveOptions,
    },
    config::HooksConfig,
    operation::{OperationRequest, RemoveRequest},
    tui::app::{AppState, WorktreeId},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveMode {
    Review,
    ConfirmDirtyWorktree,
    ConfirmUnmergedBranch,
    Ready,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveFlowError {
    #[error("no worktree is selected")]
    NoSelection,
    #[error(transparent)]
    Assessment(#[from] RemovalAssessmentError),
    #[error("the main worktree cannot be removed")]
    MainWorktree,
    #[error("the current removal risk has not been confirmed")]
    ConfirmationRequired,
    #[error(transparent)]
    Authorization(#[from] RemovalAuthorizationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveTarget {
    pub id: WorktreeId,
    pub worktree: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub dirty: bool,
    pub merged: Option<bool>,
}

#[derive(Debug)]
pub struct RemoveDialog {
    target: RemoveTarget,
    assessment: RemovalAssessment,
    delete_branch: bool,
    dirty_confirmed: bool,
    unmerged_confirmed: bool,
    mode: RemoveMode,
}

impl RemoveDialog {
    pub fn discover_selected(
        state: &AppState,
        cwd: &Path,
        configured_base: Option<&str>,
    ) -> Result<Self, RemoveFlowError> {
        let selected = state
            .selected_identity()
            .ok_or(RemoveFlowError::NoSelection)?;
        let selector = selected.path.to_string_lossy();
        let assessment = RemovalAssessment::discover(cwd, &selector, configured_base)?;
        Self::new(selected.id.clone(), assessment)
    }

    pub fn new(id: WorktreeId, assessment: RemovalAssessment) -> Result<Self, RemoveFlowError> {
        if assessment.is_main() {
            return Err(RemoveFlowError::MainWorktree);
        }
        let target = RemoveTarget {
            id,
            worktree: assessment.worktree().to_string(),
            branch: assessment.branch().map(ToOwned::to_owned),
            detached: assessment.detached(),
            dirty: assessment.dirty(),
            merged: assessment.merged(),
        };
        Ok(Self {
            target,
            assessment,
            delete_branch: false,
            dirty_confirmed: false,
            unmerged_confirmed: false,
            mode: RemoveMode::Review,
        })
    }

    pub fn target(&self) -> &RemoveTarget {
        &self.target
    }

    pub fn mode(&self) -> RemoveMode {
        self.mode
    }

    pub fn delete_branch(&self) -> bool {
        self.delete_branch
    }

    pub fn can_delete_branch(&self) -> bool {
        !self.target.detached && self.target.branch.is_some()
    }

    pub fn toggle_delete_branch(&mut self) {
        if self.can_delete_branch() {
            self.delete_branch = !self.delete_branch;
            if !self.delete_branch {
                self.unmerged_confirmed = false;
            }
        }
    }

    pub fn begin_submit(&mut self) -> RemoveMode {
        self.mode = if self.target.dirty && !self.dirty_confirmed {
            RemoveMode::ConfirmDirtyWorktree
        } else if self.delete_branch
            && self.target.merged == Some(false)
            && !self.unmerged_confirmed
        {
            RemoveMode::ConfirmUnmergedBranch
        } else {
            RemoveMode::Ready
        };
        self.mode
    }

    pub fn confirm_current_risk(&mut self) -> RemoveMode {
        match self.mode {
            RemoveMode::ConfirmDirtyWorktree => self.dirty_confirmed = true,
            RemoveMode::ConfirmUnmergedBranch => self.unmerged_confirmed = true,
            RemoveMode::Review | RemoveMode::Ready => return self.mode,
        }
        self.begin_submit()
    }

    pub fn cancel_confirmation(&mut self) {
        self.mode = RemoveMode::Review;
    }

    pub fn into_request(
        mut self,
        hooks: Option<HooksConfig>,
    ) -> Result<OperationRequest, RemoveFlowError> {
        if self.begin_submit() != RemoveMode::Ready {
            return Err(RemoveFlowError::ConfirmationRequired);
        }
        let options = RemoveOptions {
            yes: true,
            force_worktree: self.target.dirty && self.dirty_confirmed,
            delete_branch: self.delete_branch,
            force_branch: self.delete_branch
                && self.target.merged == Some(false)
                && self.unmerged_confirmed,
            no_hooks: false,
            dry_run: false,
        };
        let plan = self.assessment.authorize(options)?;
        Ok(OperationRequest::Remove(RemoveRequest { plan, hooks }))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        process::Command,
    };

    use super::*;
    use crate::tui::{app::WorktreeIdentity, search::QueryBuffer};

    struct Fixture {
        root: tempfile::TempDir,
        worktree_path: PathBuf,
    }

    impl Fixture {
        fn new(branch: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            git(root.path(), &["init", "-b", "main"]);
            git(root.path(), &["config", "user.email", "test@example.com"]);
            git(root.path(), &["config", "user.name", "Test"]);
            std::fs::write(root.path().join("README.md"), "base\n").unwrap();
            git(root.path(), &["add", "README.md"]);
            git(root.path(), &["commit", "-m", "base"]);
            let worktree_path = root.path().join("worktrees").join(branch);
            git(
                root.path(),
                &[
                    "worktree",
                    "add",
                    "-b",
                    branch,
                    worktree_path.to_str().unwrap(),
                    "main",
                ],
            );
            Self {
                root,
                worktree_path,
            }
        }

        fn assessment(&self) -> RemovalAssessment {
            RemovalAssessment::discover(
                self.root.path(),
                self.worktree_path.to_str().unwrap(),
                Some("main"),
            )
            .unwrap()
        }

        fn identity(&self, branch: Option<&str>, is_main: bool) -> WorktreeIdentity {
            WorktreeIdentity {
                id: WorktreeId::new(&self.worktree_path),
                worktree: self
                    .worktree_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                branch: branch.map(ToOwned::to_owned),
                path: self.worktree_path.clone(),
                head: None,
                is_main,
                is_current: false,
                detached: branch.is_none(),
            }
        }
    }

    fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn filtered_selection_is_the_only_target_discovered() {
        let alpha = Fixture::new("alpha");
        let beta_path = alpha.root.path().join("worktrees").join("beta");
        git(
            alpha.root.path(),
            &[
                "worktree",
                "add",
                "-b",
                "beta",
                beta_path.to_str().unwrap(),
                "main",
            ],
        );
        let alpha_identity = alpha.identity(Some("alpha"), false);
        let beta_identity = WorktreeIdentity {
            id: WorktreeId::new(&beta_path),
            worktree: "beta".to_string(),
            branch: Some("beta".to_string()),
            path: beta_path,
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
        };
        let mut state = AppState::new(vec![alpha_identity, beta_identity.clone()]);
        let mut query = QueryBuffer::default();
        for character in "bet".chars() {
            query.insert(character);
        }
        state.search = Some(query);
        state.selected = Some(beta_identity.id.clone());

        let dialog =
            RemoveDialog::discover_selected(&state, alpha.root.path(), Some("main")).unwrap();

        assert_eq!(dialog.target().id, beta_identity.id);
        assert_eq!(dialog.target().branch.as_deref(), Some("beta"));
    }

    #[test]
    fn main_is_rejected_and_detached_has_no_branch_deletion_control() {
        let fixture = Fixture::new("feature");
        let main = RemovalAssessment::discover(fixture.root.path(), "main", Some("main")).unwrap();
        assert!(matches!(
            RemoveDialog::new(WorktreeId::new(fixture.root.path()), main),
            Err(RemoveFlowError::MainWorktree)
        ));

        git(&fixture.worktree_path, &["checkout", "--detach"]);
        let detached = RemovalAssessment::discover(
            fixture.root.path(),
            fixture.worktree_path.to_str().unwrap(),
            Some("main"),
        )
        .unwrap();
        let mut dialog =
            RemoveDialog::new(WorktreeId::new(&fixture.worktree_path), detached).unwrap();
        assert!(dialog.target().detached);
        assert!(!dialog.can_delete_branch());
        dialog.toggle_delete_branch();
        assert!(!dialog.delete_branch());
    }

    #[test]
    fn dirty_and_unmerged_branch_risks_require_independent_confirmations() {
        let fixture = Fixture::new("risky");
        std::fs::write(fixture.worktree_path.join("dirty.txt"), "dirty\n").unwrap();
        std::fs::write(fixture.worktree_path.join("feature.txt"), "feature\n").unwrap();
        git(&fixture.worktree_path, &["add", "feature.txt"]);
        git(&fixture.worktree_path, &["commit", "-m", "feature"]);
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();
        dialog.toggle_delete_branch();

        assert_eq!(dialog.begin_submit(), RemoveMode::ConfirmDirtyWorktree);
        assert_eq!(
            dialog.confirm_current_risk(),
            RemoveMode::ConfirmUnmergedBranch
        );
        assert!(matches!(
            dialog.into_request(None),
            Err(RemoveFlowError::ConfirmationRequired)
        ));

        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();
        dialog.toggle_delete_branch();
        dialog.begin_submit();
        dialog.confirm_current_risk();
        assert_eq!(dialog.confirm_current_risk(), RemoveMode::Ready);
        let request = dialog.into_request(None).unwrap();
        assert!(matches!(request, OperationRequest::Remove(_)));
    }

    #[test]
    fn clean_merged_submission_builds_the_shared_removal_request() {
        let fixture = Fixture::new("merged");
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();
        assert!(!dialog.delete_branch());
        dialog.toggle_delete_branch();
        assert_eq!(dialog.begin_submit(), RemoveMode::Ready);

        let request = dialog.into_request(None).unwrap();
        let OperationRequest::Remove(request) = request else {
            panic!("remove flow must construct the shared remove request")
        };
        let value = serde_json::to_value(&request.plan).unwrap();
        assert_eq!(value["delete_branch"], true);
        assert_eq!(value["force_worktree"], false);
        assert_eq!(value["force_branch"], false);
        assert_eq!(value["hook_policy"], "run");
    }
}
