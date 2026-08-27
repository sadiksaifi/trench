use crate::{
    cli::commands::sync::stateless::SyncStrategy,
    ref_catalog::RefSnapshot,
    tui::{
        app::{WorktreeId, WorktreeIdentity, WorktreeStatus},
        ref_picker::RefPicker,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSubmission {
    pub target: WorktreeId,
    pub base: String,
    pub strategy: SyncStrategy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncDialog {
    target: WorktreeId,
    base_picker: RefPicker,
    strategy: SyncStrategy,
}

impl SyncDialog {
    pub fn new(
        target: &WorktreeIdentity,
        refs: RefSnapshot,
        configured_base: Option<&str>,
    ) -> Self {
        Self {
            target: target.id.clone(),
            base_picker: RefPicker::new(refs, configured_base),
            strategy: SyncStrategy::Rebase,
        }
    }

    pub fn base(&self) -> Option<&str> {
        self.base_picker.selected()
    }

    pub fn strategy(&self) -> SyncStrategy {
        self.strategy
    }

    pub fn set_strategy(&mut self, strategy: SyncStrategy) {
        self.strategy = strategy;
    }

    pub fn submission(&self) -> Option<SyncSubmission> {
        Some(SyncSubmission {
            target: self.target.clone(),
            base: self.base()?.to_string(),
            strategy: self.strategy,
        })
    }
}

pub fn unavailable_reason(
    identity: &WorktreeIdentity,
    status: Option<&WorktreeStatus>,
) -> Option<&'static str> {
    if identity.detached {
        return Some("Detached worktrees cannot be synced");
    }
    let Some(status) = status else {
        return Some("Git status is still loading");
    };
    (status.staged + status.modified + status.untracked > 0)
        .then_some("Dirty worktrees cannot be synced")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{
        cli::commands::sync::stateless::SyncStrategy,
        ref_catalog::RefSnapshot,
        tui::app::{WorktreeIdentity, WorktreeStatus},
    };

    fn identity(branch: Option<&str>, is_main: bool) -> WorktreeIdentity {
        WorktreeIdentity {
            id: WorktreeId::new("/worktrees/feature-auth"),
            worktree: "feature-auth".to_string(),
            branch: branch.map(str::to_string),
            path: PathBuf::from("/worktrees/feature-auth"),
            head: Some("1234567890abcdef".to_string()),
            is_main,
            is_current: false,
            detached: branch.is_none(),
        }
    }

    fn refs() -> RefSnapshot {
        RefSnapshot::from_parts(
            ["main", "release"],
            ["origin/main", "origin/topic/two"],
            Some("origin/main"),
            Some("main"),
            true,
        )
    }

    #[test]
    fn dialog_shows_configured_base_and_strategy_and_submits_the_exact_target() {
        let target = identity(Some("feature/auth"), false);
        let mut dialog = SyncDialog::new(&target, refs(), Some("release"));

        assert_eq!(dialog.base(), Some("release"));
        assert_eq!(dialog.strategy(), SyncStrategy::Rebase);
        dialog.set_strategy(SyncStrategy::Merge);
        assert_eq!(
            dialog.submission(),
            Some(SyncSubmission {
                target: target.id,
                base: "release".to_string(),
                strategy: SyncStrategy::Merge,
            })
        );
    }

    #[test]
    fn eligibility_rejects_loading_dirty_and_detached_rows_but_allows_main() {
        let clean = WorktreeStatus::default();
        let dirty = WorktreeStatus {
            modified: 1,
            ..WorktreeStatus::default()
        };

        assert_eq!(
            unavailable_reason(&identity(Some("main"), true), Some(&clean)),
            None
        );
        assert_eq!(
            unavailable_reason(&identity(Some("feature/auth"), false), None),
            Some("Git status is still loading")
        );
        assert_eq!(
            unavailable_reason(&identity(Some("feature/auth"), false), Some(&dirty)),
            Some("Dirty worktrees cannot be synced")
        );
        assert_eq!(
            unavailable_reason(&identity(None, false), Some(&clean)),
            Some("Detached worktrees cannot be synced")
        );
    }
}
