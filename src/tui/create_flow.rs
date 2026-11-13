//! The create dialog: one screen, branch first.
//!
//! The user types a branch name. The list under the field always starts with
//! what that name means: a new branch, or the existing branch it matches. The
//! highlighted row is the only thing Enter acts on. The base (`From`) matters
//! only for a new branch, so it is a second field reached with Tab instead of
//! a step everyone must pass through.

use std::path::{Path, PathBuf};

use crate::{
    paths,
    ref_catalog::{RefCandidate, RefKind, RefSnapshot},
    tui::{
        app::WorktreeId,
        line_input::{LineEdit, LineInput},
        ref_picker::{RefPicker, RefPickerEffect, RefPickerKey},
        search,
    },
};

pub use crate::tui::ref_picker::OriginRefresh;

/// Shown when live Git no longer matches what the dialog showed for the
/// highlighted branch.
pub const STALE_CREATE_SUMMARY: &str = "That branch changed. Pick it again.";

/// Shown when Enter is pressed after what the form says changed, so Enter
/// did nothing.
pub const SUMMARY_CHANGED: &str = "This changed. Check it, then press Enter again.";

/// A worktree that already exists: its directory and the branch it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingWorktree {
    pub branch: Option<String>,
    pub id: WorktreeId,
    pub path: PathBuf,
}

impl ExistingWorktree {
    pub fn new(branch: Option<&str>, id: WorktreeId, path: impl Into<PathBuf>) -> Self {
        Self {
            branch: branch.map(ToOwned::to_owned),
            id,
            path: path.into(),
        }
    }

    /// The label a person recognizes this worktree by.
    fn label(&self) -> String {
        self.branch.clone().unwrap_or_else(|| self.directory())
    }

    fn directory(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// What a row in the branch list is, and so what Enter does with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// The typed name does not exist yet.
    New,
    /// A local branch without a worktree.
    Local,
    /// A branch that exists only on origin.
    Remote { upstream: String },
    /// A branch that already has a worktree.
    HasWorktree { id: WorktreeId, path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRow {
    /// The local branch the worktree would check out.
    pub branch: String,
    pub kind: RowKind,
    /// Why this row cannot be created, when it cannot.
    pub problem: Option<Problem>,
}

/// Why a new branch name cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The name is not a valid Git branch name.
    InvalidName(String),
    /// The name maps to a directory another worktree already uses.
    DirectoryTaken { owner: String, directory: String },
    /// Git stores branches as paths, so `a` and `a/b` cannot both exist.
    NamespaceConflict { existing: String, name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateFocus {
    Branch,
    Base,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateKey {
    Character(char),
    Edit(LineEdit),
    Tab,
    Enter,
    Escape,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSubmission {
    /// The name handed to planning. For a remote-only branch this is the
    /// full remote ref (`refs/remotes/origin/x`), not the local name the row
    /// shows, so planning cannot mistake it for a local branch of the same
    /// or a similar name.
    pub selection: String,
    pub from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateEffect {
    Close,
    /// Close the dialog and select a worktree that already exists.
    Navigate(WorktreeId),
    Submit(CreateSubmission),
}

/// Everything the dialog says about the highlighted row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Summary {
    /// Nothing is highlighted yet.
    Empty,
    Blocked(Problem),
    /// A new branch has no base to start from.
    NoBase,
    New {
        branch: String,
        base: String,
        path: PathBuf,
    },
    Local {
        branch: String,
        path: PathBuf,
    },
    Remote {
        branch: String,
        upstream: String,
        path: PathBuf,
    },
    HasWorktree {
        branch: String,
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDialog {
    repository: String,
    worktree_root: PathBuf,
    refs: RefSnapshot,
    worktrees: Vec<ExistingWorktree>,
    branch: LineInput,
    highlight: Option<usize>,
    base_picker: RefPicker,
    focus: CreateFocus,
    validation_error: Option<String>,
    /// One line above the summary, for a key that did nothing because what
    /// the form says changed. Unlike an error it leaves the summary visible.
    notice: Option<&'static str>,
    /// The summary in the last frame drawn. Enter acts only on that.
    drawn_summary: Option<Summary>,
}

impl CreateDialog {
    pub fn new<I>(
        repository: impl Into<String>,
        worktree_root: &Path,
        refs: RefSnapshot,
        configured_base: Option<&str>,
        worktrees: I,
    ) -> Self
    where
        I: IntoIterator<Item = ExistingWorktree>,
    {
        Self {
            repository: repository.into(),
            worktree_root: worktree_root.to_path_buf(),
            base_picker: RefPicker::new(refs.clone(), configured_base),
            refs,
            worktrees: worktrees.into_iter().collect(),
            branch: LineInput::default(),
            highlight: None,
            focus: CreateFocus::Branch,
            validation_error: None,
            notice: None,
            drawn_summary: None,
        }
    }

    /// Replace the typed branch name, as if the user had typed it.
    #[cfg(test)]
    pub fn set_branch(&mut self, branch: &str) {
        self.branch = LineInput::default();
        self.highlight = None;
        for character in branch.chars() {
            self.edit_branch(LineEdit::Insert(character));
        }
    }

    pub fn focus(&self) -> CreateFocus {
        self.focus
    }

    pub fn branch_input(&self) -> &LineInput {
        &self.branch
    }

    pub fn base_input(&self) -> &LineInput {
        self.base_picker.query_input()
    }

    /// The base a new branch would start from.
    pub fn base(&self) -> Option<&str> {
        self.base_picker.selected()
    }

    pub fn base_candidates(&self) -> Vec<RefCandidate> {
        self.base_picker.candidates()
    }

    pub fn base_highlight(&self) -> usize {
        self.base_picker.selection()
    }

    pub fn highlight(&self) -> Option<usize> {
        self.highlight
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
        if error.is_some() {
            self.notice = None;
        }
        self.validation_error = error;
    }

    pub fn notice(&self) -> Option<&'static str> {
        self.notice
    }

    /// Show `notice` above the summary. An error in its place goes, so the
    /// summary it is about is visible.
    pub fn set_notice(&mut self, notice: &'static str) {
        self.validation_error = None;
        self.notice = Some(notice);
    }

    fn clear_messages(&mut self) {
        self.validation_error = None;
        self.notice = None;
    }

    pub fn set_origin_refresh(&mut self, refresh: OriginRefresh) {
        self.base_picker.set_origin_refresh(refresh);
    }

    pub fn update_refs(&mut self, refs: RefSnapshot) {
        let highlighted = self.highlighted_row();
        self.base_picker.update_refs(refs.clone());
        self.refs = refs;
        self.restore_highlight(highlighted);
    }

    pub fn update_worktrees<I>(&mut self, worktrees: I)
    where
        I: IntoIterator<Item = ExistingWorktree>,
    {
        let highlighted = self.highlighted_row();
        self.worktrees = worktrees.into_iter().collect();
        self.restore_highlight(highlighted);
    }

    /// The branch list for the typed text.
    ///
    /// With text typed, row 0 is always what that exact text means: the
    /// existing branch of that name, or a new branch. Fuzzy matches follow,
    /// best first. Among equal matches, branches that can get a worktree come
    /// before branches that already have one.
    pub fn rows(&self) -> Vec<BranchRow> {
        let query = self.branch.value().trim();
        // A remote branch is listed by its local name, so `origin/x` also
        // matches the row `x`. A local branch really named `origin/x` is
        // listed under that whole name and matches the text as typed.
        let short = query.strip_prefix("origin/").unwrap_or(query);
        let mut ranked = self
            .existing_rows()
            .into_iter()
            .filter_map(|row| {
                search::fuzzy_score(&row.branch, query)
                    .max(search::fuzzy_score(&row.branch, short))
                    .map(|score| (score, kind_rank(&row), row))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            (right.0, left.1, &left.2.branch).cmp(&(left.0, right.1, &right.2.branch))
        });
        let mut rows = ranked
            .into_iter()
            .map(|(_, _, row)| row)
            .collect::<Vec<_>>();
        if query.is_empty() {
            return rows;
        }
        match exact_match(&rows, query, short) {
            Some(exact) => {
                let row = rows.remove(exact);
                rows.insert(0, row);
            }
            None => rows.insert(
                0,
                BranchRow {
                    branch: query.to_string(),
                    kind: RowKind::New,
                    problem: self.problem_with(query),
                },
            ),
        }
        rows
    }

    pub fn highlighted_row(&self) -> Option<BranchRow> {
        let highlight = self.highlight?;
        self.rows().into_iter().nth(highlight)
    }

    pub fn summary(&self) -> Summary {
        let Some(row) = self.highlighted_row() else {
            return Summary::Empty;
        };
        if let Some(problem) = row.problem {
            return Summary::Blocked(problem);
        }
        let path = self.planned_path(&row.branch);
        match row.kind {
            RowKind::New => match self.base() {
                Some(base) => Summary::New {
                    branch: row.branch,
                    base: base.to_string(),
                    path,
                },
                None => Summary::NoBase,
            },
            RowKind::Local => Summary::Local {
                branch: row.branch,
                path,
            },
            RowKind::Remote { upstream } => Summary::Remote {
                branch: row.branch,
                upstream,
                path,
            },
            RowKind::HasWorktree { path, .. } => Summary::HasWorktree {
                branch: row.branch,
                path,
            },
        }
    }

    /// Whether the base applies to the highlighted row.
    /// Whether the repository has any branch at all. It has none before
    /// its first commit.
    pub fn repository_has_branches(&self) -> bool {
        self.base_picker.has_refs()
    }

    /// Record that the current summary is on screen.
    pub fn note_drawn(&mut self) {
        // An error fills the summary area, so that frame showed no summary.
        self.drawn_summary = self.validation_error.is_none().then(|| self.summary());
    }

    /// Whether the summary on screen is still what Enter would do. A refresh
    /// can change it between a frame and the next key.
    pub fn summary_as_drawn(&self) -> bool {
        self.drawn_summary.as_ref() == Some(&self.summary())
    }

    pub fn base_applies(&self) -> bool {
        self.repository_has_branches()
            && self
                .highlighted_row()
                .is_some_and(|row| row.kind == RowKind::New && row.problem.is_none())
    }

    /// What Enter would start for the highlighted row, if anything.
    pub fn submission(&self) -> Option<CreateSubmission> {
        let row = self.highlighted_row()?;
        if row.problem.is_some() {
            return None;
        }
        match row.kind {
            RowKind::New => Some(CreateSubmission {
                selection: row.branch,
                from: Some(self.base()?.to_string()),
            }),
            RowKind::Remote { upstream } => Some(CreateSubmission {
                selection: format!("refs/remotes/{upstream}"),
                from: None,
            }),
            RowKind::Local | RowKind::HasWorktree { .. } => Some(CreateSubmission {
                selection: row.branch,
                from: None,
            }),
        }
    }

    /// Whether Enter does anything right now.
    pub fn can_submit(&self) -> bool {
        match self.focus {
            CreateFocus::Branch => self.submission().is_some(),
            CreateFocus::Base => !self.base_candidates().is_empty(),
        }
    }

    pub fn handle_key(&mut self, key: CreateKey) -> Option<CreateEffect> {
        match self.focus {
            CreateFocus::Branch => self.handle_branch_key(key),
            CreateFocus::Base => {
                self.handle_base_key(key);
                None
            }
        }
    }

    fn handle_branch_key(&mut self, key: CreateKey) -> Option<CreateEffect> {
        match key {
            CreateKey::Character(character) => self.edit_branch(LineEdit::Insert(character)),
            CreateKey::Edit(edit) => self.edit_branch(edit),
            // Moving the highlight asks about a different row, so a message
            // about the previous attempt no longer applies.
            CreateKey::Up => {
                self.clear_messages();
                self.highlight = self.highlight.map(|index| index.saturating_sub(1));
            }
            CreateKey::Down => {
                self.clear_messages();
                let last = self.rows().len().checked_sub(1)?;
                self.highlight = Some(self.highlight.map_or(0, |index| (index + 1).min(last)));
            }
            CreateKey::Tab => {
                if self.base_applies() {
                    self.base_picker.open();
                    self.focus = CreateFocus::Base;
                }
            }
            CreateKey::Escape => return Some(CreateEffect::Close),
            CreateKey::Enter => {
                let row = self.highlighted_row()?;
                if let RowKind::HasWorktree { id, .. } = row.kind {
                    return Some(CreateEffect::Navigate(id));
                }
                return self.submission().map(CreateEffect::Submit);
            }
        }
        None
    }

    fn handle_base_key(&mut self, key: CreateKey) {
        match key {
            CreateKey::Character(character) => {
                self.base_picker
                    .handle_key(RefPickerKey::Character(character));
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
                    self.clear_messages();
                    self.focus = CreateFocus::Branch;
                }
            }
            CreateKey::Escape | CreateKey::Tab => self.focus = CreateFocus::Branch,
        }
    }

    fn edit_branch(&mut self, edit: LineEdit) {
        if self.branch.edit(edit) {
            self.clear_messages();
            // Row 0 is the typed name itself, so it is what Enter should do.
            self.highlight = (!self.branch.value().trim().is_empty()).then_some(0);
        }
    }

    /// Put the highlight back on the row it was on before a refresh.
    ///
    /// Enter acts on the highlight, and a refresh can arrive at any moment
    /// (the watcher, a background fetch). The highlight therefore follows the
    /// branch, not the position. When that branch is gone, or now means a
    /// different action, nothing is highlighted, and the dialog says why so
    /// the user picks again.
    fn restore_highlight(&mut self, previous: Option<BranchRow>) {
        let had_highlight = previous.is_some();
        self.highlight = previous.and_then(|previous| {
            self.rows().iter().position(|row| {
                row.branch == previous.branch
                    && std::mem::discriminant(&row.kind) == std::mem::discriminant(&previous.kind)
            })
        });
        if had_highlight && self.highlight.is_none() {
            self.set_notice(STALE_CREATE_SUMMARY);
        }
    }

    /// Where a worktree for `branch` would be created, as the summary shows.
    pub fn planned_path(&self, branch: &str) -> PathBuf {
        crate::worktree_policy::planned_path(&self.worktree_root, &self.repository, branch)
    }

    fn existing_rows(&self) -> Vec<BranchRow> {
        let mut rows = self
            .refs
            .candidates()
            .into_iter()
            .map(|candidate| {
                // The ref's kind decides what the name means. A local branch
                // may itself be called `origin/x`; only a remote ref has a
                // prefix to strip.
                let branch = match candidate.kind {
                    RefKind::Local => candidate.name.clone(),
                    RefKind::Remote => candidate
                        .name
                        .strip_prefix("origin/")
                        .unwrap_or(&candidate.name)
                        .to_string(),
                };
                let kind = match (self.worktree_for(&branch), candidate.kind) {
                    (Some(worktree), _) => RowKind::HasWorktree {
                        id: worktree.id.clone(),
                        path: worktree.path.clone(),
                    },
                    (None, RefKind::Local) => RowKind::Local,
                    (None, RefKind::Remote) => RowKind::Remote {
                        upstream: candidate.name,
                    },
                };
                BranchRow {
                    branch,
                    kind,
                    problem: None,
                }
            })
            .collect::<Vec<_>>();
        // A worktree can hold a branch the ref snapshot has not seen yet.
        for worktree in &self.worktrees {
            let Some(branch) = worktree.branch.as_deref() else {
                continue;
            };
            if !rows.iter().any(|row| row.branch == branch) {
                rows.push(BranchRow {
                    branch: branch.to_string(),
                    kind: RowKind::HasWorktree {
                        id: worktree.id.clone(),
                        path: worktree.path.clone(),
                    },
                    problem: None,
                });
            }
        }
        rows
    }

    fn worktree_for(&self, branch: &str) -> Option<&ExistingWorktree> {
        self.worktrees
            .iter()
            .find(|worktree| worktree.branch.as_deref() == Some(branch))
    }

    /// Check a new branch name against what the dialog already knows.
    ///
    /// Planning repeats these checks against live Git when Enter is pressed;
    /// checking here only moves the answer to before the keypress.
    fn problem_with(&self, name: &str) -> Option<Problem> {
        if let Err(reason) = paths::validate_branch_name(name) {
            return Some(Problem::InvalidName(reason));
        }
        if let Some(existing) = self.refs.namespace_conflict(name) {
            return Some(Problem::NamespaceConflict {
                existing: existing.to_string(),
                name: name.to_string(),
            });
        }
        let directory = paths::sanitize_branch(name);
        self.worktrees
            .iter()
            .find(|worktree| worktree.directory() == directory)
            .map(|worktree| Problem::DirectoryTaken {
                owner: worktree.label(),
                directory,
            })
    }
}

/// The row the typed text names exactly, in the order planning resolves it
/// (`CreatePlanner::classify`): a local branch spelled as typed, then the
/// local branch of the short name, then the remote ref.
fn exact_match(rows: &[BranchRow], query: &str, short: &str) -> Option<usize> {
    let local = |name: &str| {
        rows.iter()
            .position(|row| row.branch == name && !matches!(row.kind, RowKind::Remote { .. }))
    };
    local(query).or_else(|| local(short)).or_else(|| {
        rows.iter().position(|row| {
            matches!(&row.kind, RowKind::Remote { upstream }
                if upstream == query || upstream == &format!("origin/{query}"))
        })
    })
}

/// Order among equally good matches: creatable branches first.
fn kind_rank(row: &BranchRow) -> u8 {
    match row.kind {
        RowKind::New => 0,
        RowKind::Local => 1,
        RowKind::Remote { .. } => 2,
        RowKind::HasWorktree { .. } => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/worktrees";

    fn refs() -> RefSnapshot {
        RefSnapshot::from_parts(
            ["main", "feature/login-page", "feature/auth", "release"],
            ["origin/main", "origin/feature/login-v2", "origin/release"],
            Some("origin/main"),
            Some("main"),
            true,
        )
    }

    fn worktree(branch: &str, directory: &str) -> ExistingWorktree {
        let path = Path::new(ROOT).join("acme").join(directory);
        ExistingWorktree::new(Some(branch), WorktreeId::new(&path), path)
    }

    fn dialog() -> CreateDialog {
        CreateDialog::new(
            "acme",
            Path::new(ROOT),
            refs(),
            None,
            [
                ExistingWorktree::new(Some("main"), WorktreeId::new("/code/acme"), "/code/acme"),
                worktree("feature/auth", "feature-auth"),
            ],
        )
    }

    fn type_text(dialog: &mut CreateDialog, text: &str) {
        for character in text.chars() {
            dialog.handle_key(CreateKey::Character(character));
        }
    }

    fn branches(dialog: &CreateDialog) -> Vec<String> {
        dialog.rows().into_iter().map(|row| row.branch).collect()
    }

    #[test]
    fn a_refresh_that_takes_away_the_highlighted_branch_says_so() {
        let mut dialog = dialog();
        dialog.set_branch("feature/login-page");
        assert_eq!(dialog.highlighted_row().unwrap().kind, RowKind::Local);

        dialog.update_refs(RefSnapshot::from_parts(
            ["main", "feature/auth", "release"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        ));

        assert_eq!(dialog.highlighted_row(), None);
        assert_eq!(dialog.notice(), Some(STALE_CREATE_SUMMARY));

        // A refresh that keeps the branch says nothing.
        let mut kept = self::dialog();
        kept.set_branch("release");
        kept.update_refs(refs());
        assert!(kept.highlighted_row().is_some());
        assert_eq!(kept.notice(), None);
    }

    #[test]
    fn dialog_opens_on_the_branch_field_with_nothing_highlighted() {
        let mut dialog = dialog();

        assert_eq!(dialog.focus(), CreateFocus::Branch);
        assert_eq!(dialog.highlight(), None);
        assert_eq!(dialog.summary(), Summary::Empty);
        assert!(!dialog.can_submit());
        assert_eq!(dialog.handle_key(CreateKey::Enter), None);
        assert_eq!(dialog.base(), Some("origin/main"));
    }

    #[test]
    fn empty_input_lists_creatable_branches_before_ones_that_have_a_worktree() {
        assert_eq!(
            branches(&dialog()),
            [
                "feature/login-page",
                "release",
                "feature/login-v2",
                "feature/auth",
                "main"
            ]
        );
    }

    #[test]
    fn typed_text_is_row_zero_and_enter_creates_exactly_that_branch() {
        let mut dialog = dialog();
        type_text(&mut dialog, "feature/login");

        let rows = dialog.rows();
        assert_eq!(rows[0].branch, "feature/login");
        assert_eq!(rows[0].kind, RowKind::New);
        assert_eq!(dialog.highlight(), Some(0));
        assert_eq!(
            dialog.summary(),
            Summary::New {
                branch: "feature/login".to_string(),
                base: "origin/main".to_string(),
                path: PathBuf::from("/worktrees/acme/feature-login"),
            }
        );
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Submit(CreateSubmission {
                selection: "feature/login".to_string(),
                from: Some("origin/main".to_string()),
            }))
        );
    }

    #[test]
    fn enter_acts_on_the_highlighted_row_not_on_the_typed_text() {
        let mut dialog = dialog();
        type_text(&mut dialog, "feature/login-p");
        dialog.handle_key(CreateKey::Down);

        assert_eq!(
            dialog.highlighted_row().unwrap().branch,
            "feature/login-page"
        );
        assert_eq!(
            dialog.summary(),
            Summary::Local {
                branch: "feature/login-page".to_string(),
                path: PathBuf::from("/worktrees/acme/feature-login-page"),
            }
        );
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Submit(CreateSubmission {
                selection: "feature/login-page".to_string(),
                from: None,
            }))
        );
        assert_eq!(dialog.branch_input().value(), "feature/login-p");
    }

    #[test]
    fn an_exact_existing_branch_replaces_the_new_branch_row() {
        let mut dialog = dialog();
        type_text(&mut dialog, "release");

        let rows = dialog.rows();
        assert_eq!(rows[0].branch, "release");
        assert_eq!(rows[0].kind, RowKind::Local);
        assert!(rows.iter().all(|row| row.kind != RowKind::New));
        assert!(!dialog.base_applies());
    }

    #[test]
    fn a_remote_only_branch_is_listed_by_its_local_name_and_tracks_origin() {
        let mut dialog = dialog();
        type_text(&mut dialog, "feature/login-v2");

        assert_eq!(
            dialog.summary(),
            Summary::Remote {
                branch: "feature/login-v2".to_string(),
                upstream: "origin/feature/login-v2".to_string(),
                path: PathBuf::from("/worktrees/acme/feature-login-v2"),
            }
        );
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Submit(CreateSubmission {
                // Planning is given the full remote ref, so it cannot confuse
                // the row with a local branch of the same or a similar name.
                selection: "refs/remotes/origin/feature/login-v2".to_string(),
                from: None,
            }))
        );

        let mut prefixed = self::dialog();
        type_text(&mut prefixed, "origin/feature/login-v2");
        assert_eq!(prefixed.rows()[0].branch, "feature/login-v2");
        assert!(prefixed.rows().iter().all(|row| row.kind != RowKind::New));
    }

    #[test]
    fn a_local_branch_whose_name_starts_with_origin_is_not_mistaken_for_a_remote() {
        let path = Path::new(ROOT).join("acme").join("origin-foo");
        let mut dialog = CreateDialog::new(
            "acme",
            Path::new(ROOT),
            RefSnapshot::from_parts(
                ["main", "origin/foo", "origin/bar"],
                ["origin/main"],
                Some("origin/main"),
                Some("main"),
                true,
            ),
            None,
            [ExistingWorktree::new(
                Some("origin/foo"),
                WorktreeId::new(&path),
                &path,
            )],
        );
        type_text(&mut dialog, "origin/foo");

        let row = dialog.highlighted_row().unwrap();
        assert_eq!(row.branch, "origin/foo", "the local name is kept whole");
        assert!(matches!(row.kind, RowKind::HasWorktree { .. }));
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Navigate(WorktreeId::new(&path)))
        );

        // The local branch `foo` exists too. The typed text still means the
        // branch spelled exactly that way, as it does in planning.
        let mut with_short = dialog.clone();
        with_short.update_refs(RefSnapshot::from_parts(
            ["main", "foo", "origin/foo", "origin/bar"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        ));
        with_short.set_branch("origin/foo");
        assert_eq!(with_short.highlighted_row().unwrap().branch, "origin/foo");

        let mut local = dialog.clone();
        local.set_branch("origin/bar");
        assert_eq!(
            local.summary(),
            Summary::Local {
                branch: "origin/bar".to_string(),
                path: PathBuf::from("/worktrees/acme/origin-bar"),
            }
        );
    }

    #[test]
    fn a_branch_with_a_worktree_navigates_instead_of_creating() {
        let mut dialog = dialog();
        type_text(&mut dialog, "feature/auth");

        assert_eq!(
            dialog.summary(),
            Summary::HasWorktree {
                branch: "feature/auth".to_string(),
                path: PathBuf::from("/worktrees/acme/feature-auth"),
            }
        );
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Navigate(WorktreeId::new(
                "/worktrees/acme/feature-auth"
            )))
        );
    }

    #[test]
    fn matches_are_ranked_best_first() {
        let mut dialog = dialog();
        type_text(&mut dialog, "rel");

        assert_eq!(branches(&dialog)[..2], ["rel", "release"]);
    }

    #[test]
    fn an_invalid_name_is_reported_while_typing_and_blocks_enter() {
        let mut dialog = dialog();
        type_text(&mut dialog, "fix login");

        let row = dialog.highlighted_row().unwrap();
        assert_eq!(
            row.problem,
            Some(Problem::InvalidName(
                "Branch name cannot contain ' '".to_string()
            ))
        );
        assert!(!dialog.can_submit());
        assert_eq!(dialog.handle_key(CreateKey::Enter), None);
        dialog.handle_key(CreateKey::Tab);
        assert_eq!(dialog.focus(), CreateFocus::Branch);
    }

    #[test]
    fn a_name_that_maps_to_a_used_directory_is_not_available() {
        let mut dialog = dialog();
        type_text(&mut dialog, "feature@auth");

        assert_eq!(
            dialog.summary(),
            Summary::Blocked(Problem::DirectoryTaken {
                owner: "feature/auth".to_string(),
                directory: "feature-auth".to_string(),
            })
        );
        assert_eq!(dialog.handle_key(CreateKey::Enter), None);
    }

    #[test]
    fn a_name_inside_or_above_an_existing_branch_is_not_available() {
        for name in ["release/2.4", "feature"] {
            let mut dialog = dialog();
            type_text(&mut dialog, name);

            assert!(
                matches!(
                    dialog.summary(),
                    Summary::Blocked(Problem::NamespaceConflict { .. })
                ),
                "{name}: {:?}",
                dialog.summary()
            );
            assert_eq!(dialog.handle_key(CreateKey::Enter), None, "{name}");
        }
    }

    #[test]
    fn tab_opens_the_base_field_only_for_a_new_branch() {
        let mut existing = dialog();
        type_text(&mut existing, "release");
        existing.handle_key(CreateKey::Tab);
        assert_eq!(existing.focus(), CreateFocus::Branch);

        let mut new = dialog();
        type_text(&mut new, "feature/login");
        new.handle_key(CreateKey::Tab);
        assert_eq!(new.focus(), CreateFocus::Base);
    }

    #[test]
    fn choosing_a_base_returns_to_the_branch_field_and_changes_the_submission() {
        let mut dialog = dialog();
        type_text(&mut dialog, "feature/login");
        dialog.handle_key(CreateKey::Tab);
        type_text(&mut dialog, "rel");

        assert_eq!(dialog.base_input().value(), "rel");
        assert_eq!(dialog.branch_input().value(), "feature/login");
        assert_eq!(dialog.handle_key(CreateKey::Enter), None);
        assert_eq!(dialog.focus(), CreateFocus::Branch);
        assert_eq!(dialog.base(), Some("release"));
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Submit(CreateSubmission {
                selection: "feature/login".to_string(),
                from: Some("release".to_string()),
            }))
        );
    }

    #[test]
    fn escape_leaves_the_base_field_unchanged_and_then_closes_the_dialog() {
        let mut dialog = dialog();
        type_text(&mut dialog, "feature/login");
        dialog.handle_key(CreateKey::Tab);
        dialog.handle_key(CreateKey::Down);

        assert_eq!(dialog.handle_key(CreateKey::Escape), None);
        assert_eq!(dialog.focus(), CreateFocus::Branch);
        assert_eq!(dialog.base(), Some("origin/main"));
        assert_eq!(
            dialog.handle_key(CreateKey::Escape),
            Some(CreateEffect::Close)
        );
    }

    #[test]
    fn arrows_move_the_highlight_without_editing_the_name() {
        let mut dialog = dialog();
        dialog.handle_key(CreateKey::Down);
        assert_eq!(dialog.highlight(), Some(0));
        dialog.handle_key(CreateKey::Down);
        dialog.handle_key(CreateKey::Down);
        assert_eq!(dialog.highlight(), Some(2));
        dialog.handle_key(CreateKey::Up);
        assert_eq!(dialog.highlight(), Some(1));
        for _ in 0..10 {
            dialog.handle_key(CreateKey::Down);
        }
        assert_eq!(dialog.highlight(), Some(4));
        assert_eq!(dialog.branch_input().value(), "");
        assert_eq!(dialog.highlighted_row().unwrap().branch, "main");
    }

    #[test]
    fn editing_resets_the_highlight_to_the_typed_name_and_clears_stale_errors() {
        let mut dialog = dialog();
        type_text(&mut dialog, "feature/login");
        dialog.handle_key(CreateKey::Down);
        dialog.set_validation_error(Some("path already exists".to_string()));

        dialog.handle_key(CreateKey::Character('s'));

        assert_eq!(dialog.highlight(), Some(0));
        assert_eq!(dialog.validation_error(), None);
        for _ in 0.."feature/logins".len() {
            dialog.handle_key(CreateKey::Edit(LineEdit::DeletePreviousCharacter));
        }
        assert_eq!(dialog.highlight(), None);
    }

    #[test]
    fn mid_line_unicode_edits_keep_the_typed_name_intact() {
        let mut dialog = dialog();
        type_text(&mut dialog, "日本");
        dialog.handle_key(CreateKey::Edit(LineEdit::PreviousCharacter));
        dialog.handle_key(CreateKey::Character('x'));

        assert_eq!(dialog.branch_input().value(), "日x本");
        assert_eq!(dialog.rows()[0].branch, "日x本");
    }

    #[test]
    fn live_updates_reclassify_rows_without_reopening_the_dialog() {
        let mut dialog = dialog();
        type_text(&mut dialog, "release");
        assert_eq!(dialog.rows()[0].kind, RowKind::Local);

        dialog.update_worktrees([worktree("release", "release")]);

        assert!(matches!(dialog.rows()[0].kind, RowKind::HasWorktree { .. }));

        dialog.update_refs(RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        ));
        dialog.update_worktrees([]);
        assert_eq!(dialog.rows()[0].kind, RowKind::New);
    }

    #[test]
    fn a_refresh_keeps_the_highlight_on_the_same_branch_not_the_same_position() {
        let mut dialog = dialog();
        dialog.handle_key(CreateKey::Down);
        dialog.handle_key(CreateKey::Down);
        assert_eq!(dialog.highlighted_row().unwrap().branch, "release");

        // A branch that sorts before the highlight appears, as after a fetch.
        dialog.update_refs(RefSnapshot::from_parts(
            [
                "main",
                "aaa-newer",
                "feature/login-page",
                "feature/auth",
                "release",
            ],
            ["origin/main", "origin/feature/login-v2", "origin/release"],
            Some("origin/main"),
            Some("main"),
            true,
        ));

        assert_eq!(dialog.highlighted_row().unwrap().branch, "release");
        assert_eq!(
            dialog.handle_key(CreateKey::Enter),
            Some(CreateEffect::Submit(CreateSubmission {
                selection: "release".to_string(),
                from: None,
            }))
        );
    }

    #[test]
    fn a_refresh_drops_the_highlight_when_its_row_is_gone_or_means_something_else() {
        let mut gone = dialog();
        gone.handle_key(CreateKey::Down);
        gone.handle_key(CreateKey::Down);
        gone.update_refs(RefSnapshot::from_parts(
            ["main", "feature/login-page", "feature/auth"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        ));
        assert_eq!(gone.highlight(), None, "release was deleted");
        assert_eq!(gone.handle_key(CreateKey::Enter), None);

        // The same name now has a worktree: Enter would navigate, not create.
        let mut changed = dialog();
        type_text(&mut changed, "release");
        assert_eq!(changed.rows()[0].kind, RowKind::Local);
        changed.update_worktrees([worktree("release", "release")]);
        assert_eq!(changed.highlight(), None);
        assert_eq!(changed.handle_key(CreateKey::Enter), None);
        changed.handle_key(CreateKey::Down);
        assert!(matches!(
            changed.handle_key(CreateKey::Enter),
            Some(CreateEffect::Navigate(_))
        ));
    }

    #[test]
    fn a_new_branch_without_any_base_cannot_be_submitted() {
        let mut dialog = CreateDialog::new(
            "acme",
            Path::new(ROOT),
            RefSnapshot::from_parts(
                Vec::<String>::new(),
                Vec::<String>::new(),
                None,
                None,
                false,
            ),
            None,
            [],
        );
        type_text(&mut dialog, "feature/first");

        assert_eq!(dialog.summary(), Summary::NoBase);
        assert_eq!(dialog.handle_key(CreateKey::Enter), None);
    }
}
