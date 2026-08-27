use std::path::{Path, PathBuf};

use crate::{
    paths,
    ref_catalog::{RefKind, RefSnapshot},
    tui::app::WorktreeId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedOutBranch {
    pub branch: String,
    pub id: WorktreeId,
    pub path: PathBuf,
}

impl CheckedOutBranch {
    pub fn new(branch: impl Into<String>, id: WorktreeId, path: impl Into<PathBuf>) -> Self {
        Self {
            branch: branch.into(),
            id,
            path: path.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchKind {
    New,
    Local,
    Remote { upstream: String },
    CheckedOut { id: WorktreeId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePreview {
    pub branch: String,
    pub worktree: String,
    pub path: PathBuf,
    pub kind: BranchKind,
    pub base: Option<String>,
    pub base_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchSuggestionKind {
    Local,
    Remote,
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSuggestion {
    pub label: String,
    pub selection: String,
    pub kind: BranchSuggestionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseCandidate {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateMode {
    Form,
    BasePicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginRefresh {
    Idle,
    Loading,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDialog {
    repository: String,
    worktree_root: PathBuf,
    refs: RefSnapshot,
    checked_out: Vec<CheckedOutBranch>,
    branch: String,
    selected_base: Option<String>,
    mode: CreateMode,
    origin_refresh: OriginRefresh,
}

impl CreateDialog {
    pub fn new<I>(
        repository: impl Into<String>,
        worktree_root: &Path,
        refs: RefSnapshot,
        checked_out: I,
    ) -> Self
    where
        I: IntoIterator<Item = CheckedOutBranch>,
    {
        let selected_base = refs.default_base(None).ok();
        Self {
            repository: repository.into(),
            worktree_root: worktree_root.to_path_buf(),
            refs,
            checked_out: checked_out.into_iter().collect(),
            branch: String::new(),
            selected_base,
            mode: CreateMode::Form,
            origin_refresh: OriginRefresh::Idle,
        }
    }

    pub fn set_branch(&mut self, branch: impl Into<String>) {
        self.branch = branch.into();
    }

    pub fn preview(&self) -> Option<CreatePreview> {
        let selection = self.branch.trim();
        if selection.is_empty() {
            return None;
        }
        let (branch, mut kind) = self.classify(selection);
        let worktree = paths::sanitize_branch(&branch);
        let mut path = self.worktree_root.join(&self.repository).join(&worktree);
        if let Some(existing) = self
            .checked_out
            .iter()
            .find(|existing| existing.branch == branch)
        {
            kind = BranchKind::CheckedOut {
                id: existing.id.clone(),
            };
            path = existing.path.clone();
        }
        let base_visible = kind == BranchKind::New;
        Some(CreatePreview {
            branch,
            worktree,
            path,
            kind,
            base: base_visible.then(|| self.selected_base.clone()).flatten(),
            base_visible,
        })
    }

    pub fn branch_suggestions(&self) -> Vec<BranchSuggestion> {
        let query = self.branch.trim();
        let mut suggestions = self
            .refs
            .candidates()
            .into_iter()
            .filter(|candidate| fuzzy_matches(&candidate.name, query))
            .map(|candidate| BranchSuggestion {
                label: candidate.name.clone(),
                selection: candidate.name,
                kind: match candidate.kind {
                    RefKind::Local => BranchSuggestionKind::Local,
                    RefKind::Remote => BranchSuggestionKind::Remote,
                },
            })
            .collect::<Vec<_>>();
        if !query.is_empty() {
            suggestions.push(BranchSuggestion {
                label: format!("Create \"{query}\" as new branch"),
                selection: query.to_string(),
                kind: BranchSuggestionKind::New,
            });
        }
        suggestions
    }

    pub fn open_base_picker(&mut self) {
        self.mode = CreateMode::BasePicker;
    }

    pub fn close_base_picker(&mut self) {
        self.mode = CreateMode::Form;
    }

    pub fn mode(&self) -> CreateMode {
        self.mode
    }

    pub fn set_origin_refresh(&mut self, refresh: OriginRefresh) {
        self.origin_refresh = refresh;
    }

    pub fn origin_spinner_visible(&self) -> bool {
        self.origin_refresh == OriginRefresh::Loading
    }

    pub fn warning(&self) -> Option<&'static str> {
        (self.origin_refresh == OriginRefresh::Failed)
            .then_some("Could not update origin; showing local and stale refs")
    }

    pub fn base_candidates(&self) -> Vec<BaseCandidate> {
        self.refs
            .candidates()
            .into_iter()
            .map(|candidate| BaseCandidate {
                name: candidate.name,
            })
            .collect()
    }

    fn classify(&self, selection: &str) -> (String, BranchKind) {
        if let Some(branch) = selection.strip_prefix("origin/") {
            if self.refs.local.iter().any(|local| local == branch) {
                return (branch.to_string(), BranchKind::Local);
            }
            if self.refs.origin.iter().any(|remote| remote == selection) {
                return (
                    branch.to_string(),
                    BranchKind::Remote {
                        upstream: selection.to_string(),
                    },
                );
            }
        }
        if self.refs.local.iter().any(|local| local == selection) {
            return (selection.to_string(), BranchKind::Local);
        }
        let upstream = format!("origin/{selection}");
        if self.refs.origin.iter().any(|remote| remote == &upstream) {
            return (selection.to_string(), BranchKind::Remote { upstream });
        }
        (selection.to_string(), BranchKind::New)
    }
}

fn fuzzy_matches(candidate: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut query = query.chars().flat_map(char::to_lowercase);
    let mut next = query.next();
    for candidate in candidate.chars().flat_map(char::to_lowercase) {
        if next == Some(candidate) {
            next = query.next();
            if next.is_none() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::ref_catalog::RefSnapshot;

    fn refs() -> RefSnapshot {
        RefSnapshot::from_parts(
            ["main", "release", "topic/one"],
            ["origin/main", "origin/release", "origin/topic/two"],
            Some("origin/main"),
            Some("main"),
            true,
        )
    }

    #[test]
    fn new_branch_preview_keeps_git_branch_and_worktree_identity_distinct() {
        let refs = RefSnapshot::from_parts(["main"], [] as [&str; 0], None, Some("main"), false);
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);

        dialog.set_branch("feature/auth");

        let preview = dialog.preview().expect("valid create preview");
        assert_eq!(preview.branch, "feature/auth");
        assert_eq!(preview.worktree, "feature-auth");
        assert_eq!(preview.path, Path::new("/worktrees/trench/feature-auth"));
        assert_eq!(preview.kind, BranchKind::New);
        assert_eq!(preview.base.as_deref(), Some("main"));
        assert!(preview.base_visible);
    }

    #[test]
    fn compact_form_adapts_for_local_remote_and_checked_out_branches() {
        let checked_out = CheckedOutBranch::new(
            "release",
            WorktreeId::new("/worktrees/trench/release"),
            "/worktrees/trench/release",
        );
        let mut dialog = CreateDialog::new(
            "trench",
            Path::new("/worktrees"),
            refs(),
            [checked_out.clone()],
        );

        dialog.set_branch("topic/one");
        let local = dialog.preview().unwrap();
        assert_eq!(local.kind, BranchKind::Local);
        assert!(!local.base_visible);
        assert_eq!(local.path, Path::new("/worktrees/trench/topic-one"));

        dialog.set_branch("origin/topic/two");
        let remote = dialog.preview().unwrap();
        assert_eq!(
            remote.kind,
            BranchKind::Remote {
                upstream: "origin/topic/two".to_string(),
            }
        );
        assert_eq!(remote.branch, "topic/two");
        assert!(!remote.base_visible);

        dialog.set_branch("release");
        let existing = dialog.preview().unwrap();
        assert_eq!(existing.kind, BranchKind::CheckedOut { id: checked_out.id });
        assert_eq!(existing.path, checked_out.path);
        assert!(!existing.base_visible);
    }

    #[test]
    fn branch_suggestions_fuzzy_match_refs_and_end_with_explicit_new_branch() {
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs(), []);

        dialog.set_branch("ttwo");

        let suggestions = dialog.branch_suggestions();
        assert_eq!(
            suggestions
                .iter()
                .map(|suggestion| suggestion.label.as_str())
                .collect::<Vec<_>>(),
            ["origin/topic/two", "Create \"ttwo\" as new branch"]
        );
        assert_eq!(
            suggestions.last().map(|suggestion| &suggestion.kind),
            Some(&BranchSuggestionKind::New)
        );
    }

    #[test]
    fn base_picker_transforms_in_place_and_keeps_stale_refs_on_fetch_failure() {
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs(), []);
        dialog.set_branch("feature/auth");

        dialog.open_base_picker();
        dialog.set_origin_refresh(OriginRefresh::Loading);
        assert_eq!(dialog.mode(), CreateMode::BasePicker);
        assert!(dialog.origin_spinner_visible());
        assert_eq!(
            dialog
                .base_candidates()
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            ["main", "release", "topic/one", "origin/topic/two"]
        );

        dialog.set_origin_refresh(OriginRefresh::Failed);
        assert!(!dialog.origin_spinner_visible());
        assert_eq!(
            dialog
                .base_candidates()
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            ["main", "release", "topic/one", "origin/topic/two"]
        );
        assert_eq!(
            dialog.warning(),
            Some("Could not update origin; showing local and stale refs")
        );

        dialog.close_base_picker();
        assert_eq!(dialog.mode(), CreateMode::Form);
        assert_eq!(
            dialog.preview().unwrap().base.as_deref(),
            Some("origin/main")
        );
    }
}
