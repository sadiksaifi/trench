use std::path::{Path, PathBuf};

use crate::{
    paths,
    ref_catalog::{RefKind, RefSnapshot},
    tui::{
        app::WorktreeId,
        line_input::{LineEdit, LineInput},
        ref_picker::{RefPicker, RefPickerEffect, RefPickerKey},
    },
};

pub use crate::tui::ref_picker::OriginRefresh;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateMode {
    Form,
    BasePicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateKey {
    Character(char),
    Backspace,
    Edit(LineEdit),
    Tab,
    Enter,
    Escape,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSubmission {
    pub branch: String,
    pub from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateEffect {
    Close,
    RefreshOrigin,
    Navigate(WorktreeId),
    Submit(CreateSubmission),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDialog {
    repository: String,
    worktree_root: PathBuf,
    refs: RefSnapshot,
    checked_out: Vec<CheckedOutBranch>,
    branch: LineInput,
    branch_selection: usize,
    base_picker: RefPicker,
    mode: CreateMode,
    validation_error: Option<String>,
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
        Self::new_with_configured_base(repository, worktree_root, refs, None, checked_out)
    }

    pub fn new_with_configured_base<I>(
        repository: impl Into<String>,
        worktree_root: &Path,
        refs: RefSnapshot,
        configured_base: Option<&str>,
        checked_out: I,
    ) -> Self
    where
        I: IntoIterator<Item = CheckedOutBranch>,
    {
        Self {
            repository: repository.into(),
            worktree_root: worktree_root.to_path_buf(),
            base_picker: RefPicker::new(refs.clone(), configured_base),
            refs,
            checked_out: checked_out.into_iter().collect(),
            branch: LineInput::default(),
            branch_selection: 0,
            mode: CreateMode::Form,
            validation_error: None,
        }
    }

    pub fn set_branch(&mut self, branch: impl Into<String>) {
        let branch = branch.into();
        self.branch = LineInput::from(branch.as_str());
        self.branch_selection = 0;
        self.validation_error = None;
    }

    pub fn preview(&self) -> Option<CreatePreview> {
        let selection = self.branch.value().trim();
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
            base: base_visible
                .then(|| self.base_picker.selected().map(ToOwned::to_owned))
                .flatten(),
            base_visible,
        })
    }

    pub fn branch_suggestions(&self) -> Vec<BranchSuggestion> {
        let query = self.branch.value().trim();
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
        self.base_picker.open();
        self.mode = CreateMode::BasePicker;
    }

    pub fn close_base_picker(&mut self) {
        self.mode = CreateMode::Form;
    }

    pub fn mode(&self) -> CreateMode {
        self.mode
    }

    pub fn branch(&self) -> &str {
        self.branch.value()
    }

    pub fn branch_input(&self) -> &LineInput {
        &self.branch
    }

    pub fn branch_selection(&self) -> usize {
        self.branch_selection
    }

    pub fn base_query(&self) -> &str {
        self.base_picker.query()
    }

    pub fn base_input(&self) -> &LineInput {
        self.base_picker.query_input()
    }

    pub fn base_selection(&self) -> usize {
        self.base_picker.selection()
    }

    pub fn set_origin_refresh(&mut self, refresh: OriginRefresh) {
        self.base_picker.set_origin_refresh(refresh);
    }

    pub fn update_refs(&mut self, refs: RefSnapshot) {
        self.base_picker.update_refs(refs.clone());
        self.refs = refs;
        self.branch_selection = self
            .branch_selection
            .min(self.branch_suggestions().len().saturating_sub(1));
    }

    pub fn origin_spinner_visible(&self) -> bool {
        self.base_picker.origin_spinner_visible()
    }

    pub fn warning(&self) -> Option<&'static str> {
        self.base_picker.warning()
    }

    pub fn validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub fn set_validation_error(&mut self, error: Option<String>) {
        self.validation_error = error;
    }

    pub fn submission(&self) -> Option<CreateSubmission> {
        let preview = self.preview()?;
        match preview.kind {
            BranchKind::CheckedOut { .. } => Some(CreateSubmission {
                branch: preview.branch,
                from: None,
            }),
            BranchKind::New => Some(CreateSubmission {
                branch: preview.branch,
                from: preview.base,
            }),
            BranchKind::Local | BranchKind::Remote { .. } => Some(CreateSubmission {
                branch: preview.branch,
                from: None,
            }),
        }
    }

    pub fn base_candidates(&self) -> Vec<crate::ref_catalog::RefCandidate> {
        self.base_picker.candidates()
    }

    pub fn handle_key(&mut self, key: CreateKey) -> Option<CreateEffect> {
        match self.mode {
            CreateMode::Form => self.handle_form_key(key),
            CreateMode::BasePicker => self.handle_base_picker_key(key),
        }
    }

    fn handle_form_key(&mut self, key: CreateKey) -> Option<CreateEffect> {
        match key {
            CreateKey::Character(character) => {
                self.branch.edit(LineEdit::Insert(character));
                self.branch_selection = 0;
                self.validation_error = None;
            }
            CreateKey::Backspace => {
                self.branch.edit(LineEdit::DeletePreviousCharacter);
                self.branch_selection = 0;
                self.validation_error = None;
            }
            CreateKey::Edit(edit) => {
                self.branch.edit(edit);
                self.branch_selection = 0;
                self.validation_error = None;
            }
            CreateKey::Tab if self.preview().is_some_and(|preview| preview.base_visible) => {
                self.open_base_picker();
                return Some(CreateEffect::RefreshOrigin);
            }
            CreateKey::Enter => {
                if let Some(selection) = self
                    .branch_suggestions()
                    .get(self.branch_selection)
                    .map(|suggestion| suggestion.selection.clone())
                {
                    self.branch = LineInput::from(selection.as_str());
                }
                let preview = self.preview()?;
                return match preview.kind {
                    BranchKind::CheckedOut { id } => Some(CreateEffect::Navigate(id)),
                    BranchKind::New => Some(CreateEffect::Submit(CreateSubmission {
                        branch: preview.branch,
                        from: preview.base,
                    })),
                    BranchKind::Local | BranchKind::Remote { .. } => {
                        Some(CreateEffect::Submit(CreateSubmission {
                            branch: preview.branch,
                            from: None,
                        }))
                    }
                };
            }
            CreateKey::Escape => return Some(CreateEffect::Close),
            CreateKey::Up => {
                self.branch_selection = self.branch_selection.saturating_sub(1);
            }
            CreateKey::Down => {
                self.branch_selection = self
                    .branch_selection
                    .saturating_add(1)
                    .min(self.branch_suggestions().len().saturating_sub(1));
            }
            CreateKey::Tab => {}
        }
        None
    }

    fn handle_base_picker_key(&mut self, key: CreateKey) -> Option<CreateEffect> {
        match key {
            CreateKey::Character(character) => {
                self.base_picker
                    .handle_key(RefPickerKey::Character(character));
            }
            CreateKey::Backspace => {
                self.base_picker.handle_key(RefPickerKey::Backspace);
            }
            CreateKey::Edit(edit) => {
                self.base_picker.handle_key(RefPickerKey::Edit(edit));
            }
            CreateKey::Up => {
                self.base_picker.handle_key(RefPickerKey::Up);
            }
            CreateKey::Down => {
                self.base_picker.handle_key(RefPickerKey::Down);
            }
            CreateKey::Enter => {
                if matches!(
                    self.base_picker.handle_key(RefPickerKey::Enter),
                    Some(RefPickerEffect::Selected(_))
                ) {
                    self.validation_error = None;
                    self.close_base_picker();
                }
            }
            CreateKey::Escape => self.close_base_picker(),
            CreateKey::Tab => {}
        }
        None
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
    fn branch_suggestions_are_keyboard_selectable_before_submission() {
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs(), []);
        dialog.set_branch("ttwo");

        assert_eq!(dialog.branch_selection(), 0);
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Submit(CreateSubmission {
                branch: "topic/two".to_string(),
                from: None,
            }))
        );

        dialog.set_branch("ttwo");
        assert_eq!(dialog.handle_key(CreateKey::Down), None);
        assert_eq!(dialog.branch_selection(), 1);
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Submit(CreateSubmission {
                branch: "ttwo".to_string(),
                from: Some("origin/main".to_string()),
            }))
        );
    }

    #[test]
    fn branch_name_supports_mid_line_unicode_edits() {
        use crate::tui::line_input::LineEdit;

        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs(), []);
        dialog.set_branch("a👨‍👩‍👧‍👦界");

        dialog.handle_key(CreateKey::Edit(LineEdit::Start));
        dialog.handle_key(CreateKey::Edit(LineEdit::NextCharacter));
        dialog.handle_key(CreateKey::Edit(LineEdit::DeleteNextCharacter));
        dialog.handle_key(CreateKey::Character('b'));

        assert_eq!(dialog.branch(), "ab界");
    }

    #[test]
    fn configured_default_base_drives_new_branch_preview() {
        let mut dialog = CreateDialog::new_with_configured_base(
            "trench",
            Path::new("/worktrees"),
            refs(),
            Some("release"),
            [],
        );

        dialog.set_branch("feature/auth");

        assert_eq!(dialog.preview().unwrap().base.as_deref(), Some("release"));
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

    #[test]
    fn keys_transform_the_form_select_a_base_and_emit_a_typed_submission() {
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs(), []);
        dialog.set_branch("feature/auth");

        assert_eq!(
            dialog.handle_key(CreateKey::Tab),
            Some(CreateEffect::RefreshOrigin)
        );
        assert_eq!(dialog.mode(), CreateMode::BasePicker);

        dialog.handle_key(CreateKey::Character('r'));
        dialog.handle_key(CreateKey::Character('e'));
        assert_eq!(
            dialog
                .base_candidates()
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            ["release"]
        );
        assert_eq!(dialog.handle_key(CreateKey::Enter), None);
        assert_eq!(dialog.mode(), CreateMode::Form);

        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Submit(CreateSubmission {
                branch: "feature/auth".to_string(),
                from: Some("release".to_string()),
            }))
        );
        assert_eq!(
            dialog.handle_key(CreateKey::Escape),
            Some(CreateEffect::Close)
        );
    }

    #[test]
    fn checked_out_branch_enter_navigates_and_local_branch_never_opens_base_picker() {
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
        dialog.set_branch("release");

        assert_eq!(dialog.handle_key(CreateKey::Tab), None);
        assert_eq!(dialog.mode(), CreateMode::Form);
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Navigate(checked_out.id))
        );
    }
}
