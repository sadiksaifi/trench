//! The remove confirmation: one screen that shows every fact up front.
//!
//! The assessment already knows whether the worktree is dirty and whether its
//! branch is merged when the dialog opens, so the dialog shows those risks
//! immediately instead of revealing them one confirmation at a time. `y` is
//! the only key that removes. Enter never destroys anything.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{
    cli::commands::remove::stateless::{
        RemovalAssessment, RemovalAssessmentError, RemovalAuthorizationError, RemoveOptions,
    },
    config::HooksConfig,
    operation::{OperationRequest, RemoveRequest},
    tui::app::{AppState, WorktreeId},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveKey {
    /// `y`: remove, accepting every risk shown.
    Confirm,
    Escape,
    /// Toggle deletion of the local branch.
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveEffect {
    Close,
    Submit,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveFlowError {
    #[error("no worktree is selected")]
    NoSelection,
    #[error(transparent)]
    Assessment(#[from] RemovalAssessmentError),
    #[error("the main worktree cannot be removed")]
    MainWorktree,
    #[error(
        "Worktree is locked. Run `git worktree unlock {path}` first.",
        path = crate::output::text::home_relative(path, crate::paths::home_dir().as_deref())
    )]
    Locked { path: PathBuf },
    #[error("The worktree changed. Review it and press y again.")]
    FactsChanged,
    #[error(transparent)]
    Authorization(#[from] RemovalAuthorizationError),
}

/// Something the removal destroys that cannot be recovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// The worktree has changes that are not committed.
    UncommittedChanges,
    /// The local branch is gone, so Git cannot prove the worktree is clean.
    UnverifiableChanges,
    /// The branch being deleted has commits its base does not.
    UnmergedCommits,
    /// The base was not found, so the branch may have unmerged commits.
    UnknownMergeStatus,
    /// The worktree is detached at commits that no branch or tag contains.
    UnreachableCommits,
}

/// How long `y` is ignored after the facts on screen change.
///
/// A double-press arrives within a few hundred milliseconds. Without this
/// pause its second press would accept a risk that was drawn one frame ago.
pub const CONFIRM_DELAY_AFTER_CHANGE: Duration = Duration::from_millis(800);

/// Whether the local branch can be deleted along with the worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchChoice {
    /// The checkbox is shown. `merged` is `None` when the base was not found.
    Offered { merged: Option<bool> },
    /// The worktree is detached: there is no branch.
    Detached,
    /// The branch was already deleted.
    AlreadyGone,
    /// Another worktree has the branch checked out, so it must stay.
    CheckedOutElsewhere(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveTarget {
    pub id: WorktreeId,
    pub worktree: String,
    pub branch: Option<String>,
    pub path: PathBuf,
    pub detached: bool,
    pub dirty: bool,
    pub merged: Option<bool>,
    /// The local branch still exists. When it does not, Git cannot compare
    /// the worktree against a commit, so `dirty` is a precaution.
    pub branch_present: bool,
    /// The worktree directory is gone; only Git metadata will be removed.
    pub directory_missing: bool,
    /// Another worktree that has the same branch checked out.
    pub branch_checked_out_elsewhere: Option<PathBuf>,
    /// The detached HEAD has commits that no branch or tag contains.
    pub head_unreachable: bool,
}

impl RemoveTarget {
    fn new(id: WorktreeId, assessment: &RemovalAssessment) -> Self {
        Self {
            id,
            worktree: assessment.worktree().to_string(),
            branch: assessment.branch().map(ToOwned::to_owned),
            path: assessment.path().to_path_buf(),
            detached: assessment.detached(),
            dirty: assessment.dirty(),
            merged: assessment.merged(),
            branch_present: assessment.branch_present(),
            directory_missing: assessment.directory_missing(),
            branch_checked_out_elsewhere: assessment
                .branch_checked_out_elsewhere()
                .map(Path::to_path_buf),
            head_unreachable: assessment.head_unreachable(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveDialog {
    target: RemoveTarget,
    assessment: RemovalAssessment,
    delete_branch: bool,
    validation_error: Option<String>,
    /// `y` does nothing before this moment.
    confirm_after: Option<Instant>,
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
        if assessment.locked() {
            return Err(RemoveFlowError::Locked {
                path: assessment.path().to_path_buf(),
            });
        }
        let target = RemoveTarget::new(id, &assessment);
        Ok(Self {
            target,
            assessment,
            delete_branch: false,
            validation_error: None,
            confirm_after: None,
        })
    }

    pub fn target(&self) -> &RemoveTarget {
        &self.target
    }

    pub fn delete_branch(&self) -> bool {
        self.delete_branch
    }

    pub fn validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub fn set_validation_error(&mut self, error: Option<String>) {
        self.validation_error = error;
    }

    pub fn branch_choice(&self) -> BranchChoice {
        if self.target.detached || self.target.branch.is_none() {
            BranchChoice::Detached
        } else if !self.target.branch_present {
            BranchChoice::AlreadyGone
        } else if let Some(path) = &self.target.branch_checked_out_elsewhere {
            BranchChoice::CheckedOutElsewhere(path.clone())
        } else {
            BranchChoice::Offered {
                merged: self.target.merged,
            }
        }
    }

    pub fn can_delete_branch(&self) -> bool {
        matches!(self.branch_choice(), BranchChoice::Offered { .. })
    }

    /// Every unrecoverable loss the removal would cause as currently chosen.
    pub fn risks(&self) -> Vec<Risk> {
        let mut risks = Vec::new();
        if self.target.dirty {
            // Only a branch that was deleted makes the comparison impossible.
            // A detached worktree never had one, and its HEAD still resolves.
            let branch_deleted = self.target.branch.is_some() && !self.target.branch_present;
            risks.push(if branch_deleted {
                Risk::UnverifiableChanges
            } else {
                Risk::UncommittedChanges
            });
        }
        if self.target.head_unreachable {
            risks.push(Risk::UnreachableCommits);
        }
        if self.delete_branch {
            match self.target.merged {
                Some(true) => {}
                Some(false) => risks.push(Risk::UnmergedCommits),
                None => risks.push(Risk::UnknownMergeStatus),
            }
        }
        risks
    }

    pub fn toggle_delete_branch(&mut self) {
        if self.can_delete_branch() {
            self.delete_branch = !self.delete_branch;
        }
    }

    pub fn handle_key(&mut self, key: RemoveKey, now: Instant) -> Option<RemoveEffect> {
        match key {
            RemoveKey::Escape => Some(RemoveEffect::Close),
            RemoveKey::Space => {
                self.toggle_delete_branch();
                None
            }
            RemoveKey::Confirm if self.confirm_after.is_some_and(|after| now < after) => None,
            RemoveKey::Confirm => Some(RemoveEffect::Submit),
        }
    }

    /// Re-read the target from live Git. Returns whether anything changed.
    ///
    /// The choice to delete the branch survives when it is still possible.
    pub fn refresh(
        &mut self,
        cwd: &Path,
        configured_base: Option<&str>,
    ) -> Result<bool, RemoveFlowError> {
        let assessment = RemovalAssessment::discover(
            cwd,
            &self.target.id.as_path().to_string_lossy(),
            configured_base,
        )?;
        if assessment == self.assessment {
            return Ok(false);
        }
        self.target = RemoveTarget::new(self.target.id.clone(), &assessment);
        self.assessment = assessment;
        self.delete_branch = self.delete_branch && self.can_delete_branch();
        Ok(true)
    }

    /// Build the removal request after `y`, against live Git.
    ///
    /// `y` accepts the risks on screen and nothing else. When the worktree
    /// changed since the dialog was drawn, the new facts are shown and the
    /// user must press `y` again.
    pub fn revalidate_request(
        &mut self,
        cwd: &Path,
        configured_base: Option<&str>,
        hooks: Option<HooksConfig>,
        now: Instant,
    ) -> Result<OperationRequest, RemoveFlowError> {
        if self.refresh(cwd, configured_base)? {
            self.confirm_after = Some(now + CONFIRM_DELAY_AFTER_CHANGE);
            return Err(RemoveFlowError::FactsChanged);
        }
        let options = RemoveOptions {
            yes: false,
            force_worktree: self.target.dirty,
            delete_branch: self.delete_branch,
            force_branch: self.delete_branch && self.target.merged != Some(true),
            no_hooks: false,
            dry_run: false,
        };
        let plan = self.assessment.clone().authorize_cockpit(options)?;
        Ok(OperationRequest::Remove(RemoveRequest { plan, hooks }))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        process::Command,
        time::{Duration, Instant},
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
                missing: false,
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
            missing: false,
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
        assert_eq!(dialog.branch_choice(), BranchChoice::Detached);
        dialog.toggle_delete_branch();
        assert!(!dialog.delete_branch());
    }

    #[test]
    fn missing_local_branch_has_no_branch_deletion_control() {
        let fixture = Fixture::new("missing-local");
        git(
            fixture.root.path(),
            &["update-ref", "-d", "refs/heads/missing-local"],
        );
        let assessment = fixture.assessment();
        let mut dialog =
            RemoveDialog::new(WorktreeId::new(&fixture.worktree_path), assessment).unwrap();
        assert_eq!(dialog.target().branch.as_deref(), Some("missing-local"));
        assert!(!dialog.can_delete_branch());
        dialog.toggle_delete_branch();
        assert!(!dialog.delete_branch());
    }

    #[test]
    fn locked_worktree_is_rejected_with_the_unlock_command() {
        let fixture = Fixture::new("locked");
        git(
            fixture.root.path(),
            &["worktree", "lock", fixture.worktree_path.to_str().unwrap()],
        );

        let error = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap_err();

        assert!(matches!(error, RemoveFlowError::Locked { .. }));
        assert!(error.to_string().contains("git worktree unlock"));
    }

    #[test]
    fn branch_checked_out_in_another_worktree_has_no_branch_deletion_control() {
        let fixture = Fixture::new("shared");
        let second = fixture.root.path().join("worktrees").join("second");
        git(
            fixture.root.path(),
            &[
                "worktree",
                "add",
                "--force",
                second.to_str().unwrap(),
                "shared",
            ],
        );
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();

        assert_eq!(
            dialog.branch_choice(),
            BranchChoice::CheckedOutElsewhere(second.canonicalize().unwrap())
        );
        dialog.toggle_delete_branch();
        assert!(!dialog.delete_branch());
    }

    #[test]
    fn a_clean_merged_worktree_shows_no_risk_and_y_builds_the_removal_request() {
        let fixture = Fixture::new("merged");
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();

        assert_eq!(
            dialog.branch_choice(),
            BranchChoice::Offered { merged: Some(true) }
        );
        assert!(dialog.risks().is_empty());
        assert_eq!(dialog.handle_key(RemoveKey::Space, Instant::now()), None);
        assert!(dialog.delete_branch());
        assert!(dialog.risks().is_empty());
        assert_eq!(
            dialog.handle_key(RemoveKey::Confirm, Instant::now()),
            Some(RemoveEffect::Submit)
        );

        let OperationRequest::Remove(request) = dialog
            .revalidate_request(fixture.root.path(), Some("main"), None, Instant::now())
            .unwrap()
        else {
            panic!("remove flow must construct the shared remove request")
        };
        let value = serde_json::to_value(&request.plan).unwrap();
        assert_eq!(value["delete_branch"], true);
        assert_eq!(value["force_worktree"], false);
        assert_eq!(value["force_branch"], false);
        assert_eq!(value["hook_policy"], "run");
        assert_eq!(value["confirmation"], "interactive");
    }

    #[test]
    fn every_risk_is_visible_before_any_key_and_y_accepts_exactly_those() {
        let fixture = Fixture::new("risky");
        std::fs::write(fixture.worktree_path.join("feature.txt"), "feature\n").unwrap();
        git(&fixture.worktree_path, &["add", "feature.txt"]);
        git(&fixture.worktree_path, &["commit", "-m", "feature"]);
        std::fs::write(fixture.worktree_path.join("dirty.txt"), "dirty\n").unwrap();
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();

        assert_eq!(dialog.risks(), [Risk::UncommittedChanges]);
        dialog.toggle_delete_branch();
        assert_eq!(
            dialog.risks(),
            [Risk::UncommittedChanges, Risk::UnmergedCommits]
        );

        let OperationRequest::Remove(request) = dialog
            .revalidate_request(fixture.root.path(), Some("main"), None, Instant::now())
            .unwrap()
        else {
            panic!("expected remove request")
        };
        let plan = serde_json::to_value(request.plan).unwrap();
        assert_eq!(plan["force_worktree_applied"], true);
        assert_eq!(plan["force_branch_applied"], true);
    }

    #[test]
    fn keeping_the_branch_never_forces_its_deletion() {
        let fixture = Fixture::new("keep-branch");
        std::fs::write(fixture.worktree_path.join("feature.txt"), "feature\n").unwrap();
        git(&fixture.worktree_path, &["add", "feature.txt"]);
        git(&fixture.worktree_path, &["commit", "-m", "feature"]);
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();

        assert!(dialog.risks().is_empty());
        let OperationRequest::Remove(request) = dialog
            .revalidate_request(fixture.root.path(), Some("main"), None, Instant::now())
            .unwrap()
        else {
            panic!("expected remove request")
        };
        let plan = serde_json::to_value(request.plan).unwrap();
        assert_eq!(plan["delete_branch"], false);
        assert_eq!(plan["force_branch"], false);
    }

    #[test]
    fn an_unknown_merge_status_is_a_risk_only_when_the_branch_is_deleted() {
        let fixture = Fixture::new("missing-base");
        let assessment = RemovalAssessment::discover(
            fixture.root.path(),
            fixture.worktree_path.to_str().unwrap(),
            Some("origin/main"),
        )
        .unwrap();
        let mut dialog =
            RemoveDialog::new(WorktreeId::new(&fixture.worktree_path), assessment).unwrap();

        assert_eq!(
            dialog.branch_choice(),
            BranchChoice::Offered { merged: None }
        );
        assert!(dialog.risks().is_empty());
        dialog.toggle_delete_branch();
        assert_eq!(dialog.risks(), [Risk::UnknownMergeStatus]);

        let OperationRequest::Remove(request) = dialog
            .revalidate_request(
                fixture.root.path(),
                Some("origin/main"),
                None,
                Instant::now(),
            )
            .unwrap()
        else {
            panic!("expected remove request")
        };
        let plan = serde_json::to_value(request.plan).unwrap();
        assert_eq!(plan["merged"], serde_json::Value::Null);
        assert_eq!(plan["force_branch_applied"], true);
    }

    #[test]
    fn a_worktree_that_changes_after_y_is_shown_again_instead_of_removed() {
        let fixture = Fixture::new("changes-late");
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();
        dialog.toggle_delete_branch();
        assert!(dialog.risks().is_empty());
        std::fs::write(fixture.worktree_path.join("late.txt"), "late\n").unwrap();

        let error = dialog
            .revalidate_request(fixture.root.path(), Some("main"), None, Instant::now())
            .unwrap_err();

        assert!(matches!(error, RemoveFlowError::FactsChanged));
        assert_eq!(
            error.to_string(),
            "The worktree changed. Review it and press y again."
        );
        assert_eq!(dialog.risks(), [Risk::UncommittedChanges]);
        assert!(dialog.delete_branch(), "the user's choice survives");
        assert!(fixture.worktree_path.join("late.txt").exists());

        let OperationRequest::Remove(request) = dialog
            .revalidate_request(fixture.root.path(), Some("main"), None, Instant::now())
            .unwrap()
        else {
            panic!("expected remove request")
        };
        let value = serde_json::to_value(request.plan).unwrap();
        assert_eq!(value["force_worktree"], true);
        assert_eq!(value["confirmation"], "interactive");
    }

    #[test]
    fn a_missing_directory_has_no_change_risk_and_keeps_the_branch_choice() {
        let fixture = Fixture::new("deleted-by-hand");
        std::fs::remove_dir_all(&fixture.worktree_path).unwrap();
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();

        assert!(dialog.target().directory_missing);
        assert!(dialog.risks().is_empty());
        assert!(dialog.can_delete_branch());
        assert!(matches!(
            dialog.revalidate_request(fixture.root.path(), Some("main"), None, Instant::now()),
            Ok(OperationRequest::Remove(_))
        ));
    }

    #[test]
    fn a_deleted_branch_makes_the_change_risk_unverifiable_and_offers_no_checkbox() {
        let fixture = Fixture::new("branch-gone");
        git(
            fixture.root.path(),
            &["update-ref", "-d", "refs/heads/branch-gone"],
        );
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();

        assert_eq!(dialog.branch_choice(), BranchChoice::AlreadyGone);
        assert_eq!(dialog.risks(), [Risk::UnverifiableChanges]);
        dialog.toggle_delete_branch();
        assert!(!dialog.delete_branch());
    }

    #[test]
    fn a_dirty_detached_worktree_reports_uncommitted_changes_not_a_missing_branch() {
        let fixture = Fixture::new("detached-dirty");
        git(&fixture.worktree_path, &["checkout", "--detach"]);
        std::fs::write(fixture.worktree_path.join("dirty.txt"), "dirty\n").unwrap();
        let dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();

        assert_eq!(dialog.risks(), [Risk::UncommittedChanges]);
    }

    #[test]
    fn commits_on_a_detached_head_that_no_branch_contains_are_a_risk() {
        let fixture = Fixture::new("detached-commits");
        git(&fixture.worktree_path, &["checkout", "--detach"]);
        let reachable = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();
        assert!(
            reachable.risks().is_empty(),
            "HEAD is still on a branch tip"
        );

        std::fs::write(fixture.worktree_path.join("only-here.txt"), "x\n").unwrap();
        git(&fixture.worktree_path, &["add", "only-here.txt"]);
        git(&fixture.worktree_path, &["commit", "-m", "only here"]);
        let unreachable = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();

        assert_eq!(unreachable.risks(), [Risk::UnreachableCommits]);
    }

    #[test]
    fn y_is_ignored_briefly_after_the_risks_change_so_a_double_press_cannot_accept_them() {
        let fixture = Fixture::new("double-press");
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();
        let pressed = Instant::now();
        assert_eq!(
            dialog.handle_key(RemoveKey::Confirm, pressed),
            Some(RemoveEffect::Submit)
        );
        std::fs::write(fixture.worktree_path.join("late.txt"), "late\n").unwrap();
        assert!(matches!(
            dialog.revalidate_request(fixture.root.path(), Some("main"), None, pressed),
            Err(RemoveFlowError::FactsChanged)
        ));

        // The second press of a double-tap lands before anyone could read.
        assert_eq!(
            dialog.handle_key(RemoveKey::Confirm, pressed + Duration::from_millis(150)),
            None
        );
        assert_eq!(
            dialog.handle_key(RemoveKey::Escape, pressed + Duration::from_millis(150)),
            Some(RemoveEffect::Close),
            "leaving is never delayed"
        );
        assert_eq!(
            dialog.handle_key(RemoveKey::Confirm, pressed + CONFIRM_DELAY_AFTER_CHANGE),
            Some(RemoveEffect::Submit)
        );
    }

    #[test]
    fn escape_closes_and_space_only_toggles_branch_deletion() {
        let fixture = Fixture::new("keys");
        let mut dialog = RemoveDialog::new(
            WorktreeId::new(&fixture.worktree_path),
            fixture.assessment(),
        )
        .unwrap();

        assert_eq!(dialog.handle_key(RemoveKey::Space, Instant::now()), None);
        assert!(dialog.delete_branch());
        assert_eq!(dialog.handle_key(RemoveKey::Space, Instant::now()), None);
        assert!(!dialog.delete_branch());
        assert_eq!(
            dialog.handle_key(RemoveKey::Escape, Instant::now()),
            Some(RemoveEffect::Close)
        );
    }
}
