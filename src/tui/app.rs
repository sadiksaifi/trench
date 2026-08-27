use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{
    ref_catalog::RefSnapshot,
    tui::{
        create_flow::CreateDialog,
        keymap::{self, Action, Context, Key},
        operation_modal::OperationModal,
        refresh::RefreshPublication,
        remove_flow::RemoveDialog,
        search::{self, QueryBuffer},
        sync_flow::{self, SyncDialog},
    },
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorktreeId(PathBuf);

impl WorktreeId {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeIdentity {
    pub id: WorktreeId,
    pub worktree: String,
    pub branch: Option<String>,
    pub path: PathBuf,
    pub head: Option<String>,
    pub is_main: bool,
    pub is_current: bool,
    pub detached: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    IdentitiesLoaded(Vec<WorktreeIdentity>),
    StatusLoaded {
        id: WorktreeId,
        status: WorktreeStatus,
    },
    RefreshPublished(RefreshPublication),
    RefreshTick,
    OperationSucceeded {
        select: Option<WorktreeId>,
        message: String,
        shown_at: Instant,
    },
    NotificationShown {
        message: String,
        shown_at: Instant,
    },
    NotificationTick(Instant),
    Select(WorktreeId),
    Input(Key),
    ViewportChanged {
        width: u16,
        height: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Switch(WorktreeId),
    Open(WorktreeId),
    OpenCreate,
    OpenSync(WorktreeId),
    OpenRemove(WorktreeId),
    Refresh,
    Quit,
    Unavailable { action: Action, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub identities: Vec<WorktreeIdentity>,
    pub statuses: BTreeMap<WorktreeId, WorktreeStatus>,
    pub refs: Option<RefSnapshot>,
    pub refresh: RefreshActivity,
    pub selected: Option<WorktreeId>,
    pub search: Option<QueryBuffer>,
    pub viewport: Viewport,
    pub inspector_override: Option<bool>,
    pub help_open: bool,
    pub create_dialog: Option<CreateDialog>,
    pub sync_dialog: Option<SyncDialog>,
    pub remove_dialog: Option<RemoveDialog>,
    pub operation_modal: Option<OperationModal>,
    pub notification: Option<Notification>,
    pending_selection: Option<WorktreeId>,
}

pub const NOTIFICATION_DURATION: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub text: String,
    expires_at: Instant,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshActivity {
    pub waiting_rows: BTreeSet<WorktreeId>,
    pub updating_refs: bool,
    pub warning: Option<String>,
    pub spinner_tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
}

impl Viewport {
    pub const MIN_WIDTH: u16 = 60;
    pub const MIN_HEIGHT: u16 = 16;
    pub const WIDE_WIDTH: u16 = 100;

    pub fn is_tiny(self) -> bool {
        self.width < Self::MIN_WIDTH || self.height < Self::MIN_HEIGHT
    }

    pub fn is_wide(self) -> bool {
        self.width >= Self::WIDE_WIDTH
    }
}

impl AppState {
    pub fn new(identities: Vec<WorktreeIdentity>) -> Self {
        let selected = identities.first().map(|identity| identity.id.clone());
        Self {
            identities,
            statuses: BTreeMap::new(),
            refs: None,
            refresh: RefreshActivity::default(),
            selected,
            search: None,
            viewport: Viewport {
                width: Viewport::MIN_WIDTH,
                height: Viewport::MIN_HEIGHT,
            },
            inspector_override: None,
            help_open: false,
            create_dialog: None,
            sync_dialog: None,
            remove_dialog: None,
            operation_modal: None,
            notification: None,
            pending_selection: None,
        }
    }

    pub fn context(&self) -> Context {
        if self.viewport.is_tiny() {
            Context::Resize
        } else if self.search.is_some() {
            Context::Search
        } else {
            Context::Cockpit
        }
    }

    pub fn selected_identity(&self) -> Option<&WorktreeIdentity> {
        self.selected_visible()
    }

    pub fn visible_identities(&self) -> Vec<&WorktreeIdentity> {
        match self.search.as_ref() {
            Some(query) => search::rank(&self.identities, query.as_str()),
            None => self.identities.iter().collect(),
        }
    }

    pub fn selected_visible(&self) -> Option<&WorktreeIdentity> {
        let selected = self.selected.as_ref()?;
        self.visible_identities()
            .into_iter()
            .find(|row| &row.id == selected)
    }

    pub fn inspector_visible(&self) -> bool {
        self.inspector_override
            .unwrap_or_else(|| self.viewport.is_wide())
    }

    pub fn row_is_waiting(&self, id: &WorktreeId) -> bool {
        self.refresh.waiting_rows.contains(id)
    }
}

pub fn reduce(state: &mut AppState, event: Event) -> Vec<Effect> {
    match event {
        Event::IdentitiesLoaded(identities) => {
            state
                .statuses
                .retain(|id, _| identities.iter().any(|row| &row.id == id));
            state.identities = identities;
            reconcile_visible_selection(state);
        }
        Event::StatusLoaded { id, status } => {
            if state.identities.iter().any(|row| row.id == id) {
                state.statuses.insert(id, status);
            }
        }
        Event::RefreshPublished(publication) => {
            let pending = state
                .pending_selection
                .as_ref()
                .filter(|selected| {
                    publication
                        .identities
                        .iter()
                        .any(|row| &row.id == *selected)
                })
                .cloned();
            if pending.is_some() {
                state.pending_selection = None;
            }
            if pending.is_some() {
                state.selected = pending;
            }
            state.identities = publication.identities;
            state.statuses = publication.statuses;
            state.refs = publication.refs;
            state.refresh.waiting_rows = publication.waiting_rows;
            state.refresh.updating_refs = publication.updating_refs;
            state.refresh.warning = publication.warning;
            reconcile_visible_selection(state);
        }
        Event::RefreshTick => {
            if state.refresh.updating_refs || !state.refresh.waiting_rows.is_empty() {
                state.refresh.spinner_tick = state.refresh.spinner_tick.wrapping_add(1);
            }
        }
        Event::OperationSucceeded {
            select,
            message,
            shown_at,
        } => {
            state.pending_selection = select;
            state.create_dialog = None;
            state.sync_dialog = None;
            state.operation_modal = None;
            state.notification = Some(Notification {
                text: message,
                expires_at: shown_at + NOTIFICATION_DURATION,
            });
            return vec![Effect::Refresh];
        }
        Event::NotificationShown { message, shown_at } => {
            state.notification = Some(Notification {
                text: message,
                expires_at: shown_at + NOTIFICATION_DURATION,
            });
        }
        Event::NotificationTick(now) => {
            if state
                .notification
                .as_ref()
                .is_some_and(|notification| now >= notification.expires_at)
            {
                state.notification = None;
            }
        }
        Event::Select(id) if state.visible_identities().iter().any(|row| row.id == id) => {
            state.selected = Some(id);
        }
        Event::Select(_) => {}
        Event::Input(key) => {
            let Some(action) = keymap::action_for(state.context(), key) else {
                if let Some(query) = state.search.as_mut() {
                    let changed = match key {
                        Key::Backspace => query.backspace(),
                        Key::Edit(edit) => query.edit(edit),
                        Key::Char(character) => query.insert(character),
                        _ => false,
                    };
                    if changed {
                        reconcile_visible_selection(state);
                    }
                }
                return Vec::new();
            };
            if let Some(reason) = unavailable_reason(state, action) {
                return vec![Effect::Unavailable {
                    action,
                    reason: reason.to_string(),
                }];
            }
            if !matches!(action, Action::SelectNext | Action::SelectPrevious) {
                state.notification = None;
            }
            return reduce_action(state, action);
        }
        Event::ViewportChanged { width, height } => {
            state.viewport = Viewport { width, height };
        }
    }
    Vec::new()
}

pub fn unavailable_reason(state: &AppState, action: Action) -> Option<&'static str> {
    let selected = state.selected_visible();
    match (action, selected) {
        (Action::Switch | Action::Open | Action::Sync | Action::Remove, None) => {
            Some("No worktree selected")
        }
        (Action::Sync, Some(identity)) => {
            sync_flow::unavailable_reason(identity, state.statuses.get(&identity.id))
        }
        (Action::DeleteBranch, Some(identity)) if identity.detached => {
            Some("Detached worktrees have no local branch to delete")
        }
        (Action::Remove, Some(identity)) if identity.is_main => {
            Some("The main worktree cannot be removed")
        }
        _ => None,
    }
}

fn reduce_action(state: &mut AppState, action: Action) -> Vec<Effect> {
    let selected = || {
        state
            .selected_visible()
            .expect("eligibility checked visible selection")
            .id
            .clone()
    };
    match action {
        Action::Switch => vec![Effect::Switch(selected())],
        Action::Open => vec![Effect::Open(selected())],
        Action::Create => vec![Effect::OpenCreate],
        Action::Sync => vec![Effect::OpenSync(selected())],
        Action::Remove => vec![Effect::OpenRemove(selected())],
        Action::DeleteBranch => Vec::new(),
        Action::Search => {
            state.search = Some(QueryBuffer::default());
            reconcile_visible_selection(state);
            Vec::new()
        }
        Action::CloseSearch => {
            state.search = None;
            state.help_open = false;
            reconcile_visible_selection(state);
            Vec::new()
        }
        Action::Refresh => vec![Effect::Refresh],
        Action::Quit => vec![Effect::Quit],
        Action::ToggleInspector => {
            state.inspector_override = Some(!state.inspector_visible());
            Vec::new()
        }
        Action::SelectNext => {
            select_relative(state, 1);
            Vec::new()
        }
        Action::SelectPrevious => {
            select_relative(state, -1);
            Vec::new()
        }
        Action::Help => {
            state.help_open = !state.help_open;
            Vec::new()
        }
    }
}

fn select_relative(state: &mut AppState, delta: isize) {
    let visible = state
        .visible_identities()
        .into_iter()
        .map(|row| row.id.clone())
        .collect::<Vec<_>>();
    if visible.is_empty() {
        state.selected = None;
        return;
    }
    let selected = state
        .selected
        .as_ref()
        .and_then(|id| visible.iter().position(|row| row == id))
        .unwrap_or(0);
    let next = selected.saturating_add_signed(delta).min(visible.len() - 1);
    state.selected = Some(visible[next].clone());
}

fn reconcile_visible_selection(state: &mut AppState) {
    let selected = state.selected.take();
    let query = state.search.as_ref().map(QueryBuffer::as_str).unwrap_or("");
    state.selected = search::reconcile_selection(&state.identities, query, selected.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(path: &str, worktree: &str) -> WorktreeIdentity {
        WorktreeIdentity {
            id: WorktreeId::new(path),
            worktree: worktree.to_string(),
            branch: Some(format!("feature/{worktree}")),
            path: PathBuf::from(path),
            head: Some("1234567890abcdef".to_string()),
            is_main: false,
            is_current: false,
            detached: false,
        }
    }

    #[test]
    fn identity_refresh_preserves_selection_by_worktree_id() {
        let alpha = identity("/worktrees/alpha", "alpha");
        let beta = identity("/worktrees/beta", "beta");
        let mut state = AppState::new(vec![alpha.clone(), beta.clone()]);
        let _ = reduce(&mut state, Event::Select(beta.id.clone()));

        let _ = reduce(
            &mut state,
            Event::IdentitiesLoaded(vec![alpha, beta.clone()]),
        );

        assert_eq!(state.selected, Some(beta.id));
    }

    #[test]
    fn detached_selection_disables_sync_with_an_explanation() {
        let mut detached = identity("/worktrees/review", "detached@1234567");
        detached.branch = None;
        detached.detached = true;
        let mut state = AppState::new(vec![detached]);

        let effects = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('s')));

        assert_eq!(
            effects,
            vec![Effect::Unavailable {
                action: crate::tui::keymap::Action::Sync,
                reason: "Detached worktrees cannot be synced".to_string(),
            }]
        );
    }

    #[test]
    fn initial_identity_snapshot_keeps_catalog_order_and_has_no_statuses() {
        let current = identity("/worktrees/zebra", "zebra");
        let alpha = identity("/worktrees/alpha", "alpha");

        let state = AppState::new(vec![current.clone(), alpha.clone()]);

        assert_eq!(state.identities, vec![current.clone(), alpha]);
        assert_eq!(state.selected, Some(current.id));
        assert!(state.statuses.is_empty());
    }

    #[test]
    fn progressive_status_attaches_to_identity_after_order_changes() {
        let alpha = identity("/worktrees/alpha", "alpha");
        let beta = identity("/worktrees/beta", "beta");
        let mut state = AppState::new(vec![alpha.clone(), beta.clone()]);
        let _ = reduce(
            &mut state,
            Event::IdentitiesLoaded(vec![beta.clone(), alpha]),
        );
        let status = WorktreeStatus {
            staged: 1,
            ..WorktreeStatus::default()
        };

        let _ = reduce(
            &mut state,
            Event::StatusLoaded {
                id: beta.id.clone(),
                status: status.clone(),
            },
        );

        assert_eq!(state.statuses.get(&beta.id), Some(&status));
    }

    #[test]
    fn detached_rows_keep_switch_open_and_remove_actions() {
        let mut detached = identity("/worktrees/review", "detached@1234567");
        detached.branch = None;
        detached.detached = true;
        let id = detached.id.clone();
        let mut state = AppState::new(vec![detached]);

        assert_eq!(
            reduce(&mut state, Event::Input(Key::Enter)),
            vec![Effect::Switch(id.clone())]
        );
        assert_eq!(
            reduce(&mut state, Event::Input(Key::Char('o'))),
            vec![Effect::Open(id.clone())]
        );
        assert_eq!(
            reduce(&mut state, Event::Input(Key::Char('d'))),
            vec![Effect::OpenRemove(id)]
        );
        assert_eq!(
            unavailable_reason(&state, Action::DeleteBranch),
            Some("Detached worktrees have no local branch to delete")
        );
    }

    #[test]
    fn main_worktree_cannot_be_removed_but_can_be_synced() {
        let mut main = identity("/repos/trench", "trench");
        main.is_main = true;
        let id = main.id.clone();
        let mut state = AppState::new(vec![main]);
        state.statuses.insert(id.clone(), WorktreeStatus::default());

        assert_eq!(
            reduce(&mut state, Event::Input(Key::Char('d'))),
            vec![Effect::Unavailable {
                action: Action::Remove,
                reason: "The main worktree cannot be removed".to_string(),
            }]
        );
        assert_eq!(
            reduce(&mut state, Event::Input(Key::Char('s'))),
            vec![Effect::OpenSync(id)]
        );
    }

    #[test]
    fn sync_waits_for_clean_status_without_calling_git_from_the_reducer() {
        let row = identity("/worktrees/alpha", "alpha");
        let id = row.id.clone();
        let mut state = AppState::new(vec![row]);

        assert_eq!(
            unavailable_reason(&state, Action::Sync),
            Some("Git status is still loading")
        );
        state.statuses.insert(
            id,
            WorktreeStatus {
                modified: 1,
                ..WorktreeStatus::default()
            },
        );
        assert_eq!(
            unavailable_reason(&state, Action::Sync),
            Some("Dirty worktrees cannot be synced")
        );
    }

    #[test]
    fn tiny_view_routes_only_quit() {
        let row = identity("/worktrees/alpha", "alpha");
        let mut state = AppState::new(vec![row]);
        let _ = reduce(
            &mut state,
            Event::ViewportChanged {
                width: Viewport::MIN_WIDTH - 1,
                height: Viewport::MIN_HEIGHT,
            },
        );

        assert!(reduce(&mut state, Event::Input(Key::Char('c'))).is_empty());
        assert_eq!(
            reduce(&mut state, Event::Input(Key::Char('q'))),
            vec![Effect::Quit]
        );
        assert!(reduce(&mut state, Event::Input(Key::Char('?'))).is_empty());
        assert!(!state.help_open);
    }

    #[test]
    fn refresh_publication_preserves_selection_and_stale_values_while_waiting() {
        let alpha = identity("/worktrees/alpha", "alpha");
        let beta = identity("/worktrees/beta", "beta");
        let mut state = AppState::new(vec![alpha.clone(), beta.clone()]);
        let _ = reduce(&mut state, Event::Select(beta.id.clone()));
        let stale = WorktreeStatus {
            modified: 2,
            ..WorktreeStatus::default()
        };

        let _ = reduce(
            &mut state,
            Event::RefreshPublished(RefreshPublication {
                identities: vec![alpha, beta.clone()],
                refs: Some(RefSnapshot {
                    local: vec!["main".to_string()],
                    origin: Vec::new(),
                    origin_head: None,
                    main_branch: Some("main".to_string()),
                    has_origin: false,
                }),
                statuses: BTreeMap::from([(beta.id.clone(), stale.clone())]),
                waiting_rows: BTreeSet::from([beta.id.clone()]),
                updating_refs: true,
                warning: None,
            }),
        );

        assert_eq!(state.selected, Some(beta.id.clone()));
        assert_eq!(state.statuses.get(&beta.id), Some(&stale));
        assert!(state.row_is_waiting(&beta.id));
        assert_eq!(state.refs.as_ref().unwrap().local, ["main"]);
    }

    #[test]
    fn spinner_ticks_only_while_refresh_activity_is_waiting() {
        let alpha = identity("/worktrees/alpha", "alpha");
        let id = alpha.id.clone();
        let mut state = AppState::new(vec![alpha.clone()]);
        let _ = reduce(&mut state, Event::RefreshTick);
        assert_eq!(state.refresh.spinner_tick, 0);

        let _ = reduce(
            &mut state,
            Event::RefreshPublished(RefreshPublication {
                identities: vec![alpha.clone()],
                refs: None,
                statuses: BTreeMap::new(),
                waiting_rows: BTreeSet::from([id]),
                updating_refs: false,
                warning: None,
            }),
        );
        let _ = reduce(&mut state, Event::RefreshTick);
        assert_eq!(state.refresh.spinner_tick, 1);

        let _ = reduce(
            &mut state,
            Event::RefreshPublished(RefreshPublication {
                identities: vec![alpha],
                refs: None,
                statuses: BTreeMap::new(),
                waiting_rows: BTreeSet::new(),
                updating_refs: false,
                warning: None,
            }),
        );
        let _ = reduce(&mut state, Event::RefreshTick);
        assert_eq!(state.refresh.spinner_tick, 1);
    }

    #[test]
    fn search_mode_edits_query_and_keeps_submit_and_close_keys() {
        let alpha = identity("/worktrees/alpha", "alpha");
        let beta = identity("/worktrees/beta", "beta");
        let mut state = AppState::new(vec![alpha, beta.clone()]);

        assert!(reduce(&mut state, Event::Input(Key::Char('/'))).is_empty());
        assert_eq!(state.search.as_ref().unwrap().as_str(), "");

        assert!(reduce(&mut state, Event::Input(Key::Char('b'))).is_empty());
        assert_eq!(state.search.as_ref().unwrap().as_str(), "b");
        assert_eq!(state.selected_visible().unwrap().id, beta.id);

        assert!(reduce(&mut state, Event::Input(Key::Backspace)).is_empty());
        assert_eq!(state.search.as_ref().unwrap().as_str(), "");
        assert_eq!(state.selected_visible().unwrap().id, beta.id);

        assert!(reduce(&mut state, Event::Input(Key::Escape)).is_empty());
        assert!(state.search.is_none());
        assert_eq!(state.selected, Some(beta.id));
    }

    #[test]
    fn launcher_search_supports_mid_line_unicode_edits() {
        use crate::tui::line_input::LineEdit;

        let mut state = AppState::new(vec![identity("/worktrees/alpha", "alpha")]);
        let _ = reduce(&mut state, Event::Input(Key::Char('/')));

        for character in "a👨‍👩‍👧‍👦界".chars() {
            let _ = reduce(&mut state, Event::Input(Key::Char(character)));
        }
        let _ = reduce(&mut state, Event::Input(Key::Edit(LineEdit::Start)));
        let _ = reduce(&mut state, Event::Input(Key::Edit(LineEdit::NextCharacter)));
        let _ = reduce(
            &mut state,
            Event::Input(Key::Edit(LineEdit::DeleteNextCharacter)),
        );
        let _ = reduce(&mut state, Event::Input(Key::Char('b')));

        assert_eq!(state.search.as_ref().unwrap().as_str(), "ab界");
    }

    #[test]
    fn focused_launcher_search_accepts_printable_cockpit_shortcuts_as_text() {
        let mut state = AppState::new(vec![identity("/worktrees/alpha", "alpha")]);
        assert!(reduce(&mut state, Event::Input(Key::Char('/'))).is_empty());

        for character in "osdjk?".chars() {
            assert!(reduce(&mut state, Event::Input(Key::Char(character))).is_empty());
        }

        assert_eq!(state.search.as_ref().unwrap().as_str(), "osdjk?");
        assert!(!state.help_open);
    }

    #[test]
    fn launcher_submit_dispatches_only_the_visible_filtered_worktree() {
        let alpha = identity("/worktrees/alpha", "alpha");
        let beta = identity("/worktrees/beta", "beta");
        let mut state = AppState::new(vec![alpha, beta.clone()]);
        state
            .statuses
            .insert(beta.id.clone(), WorktreeStatus::default());
        let _ = reduce(&mut state, Event::Input(Key::Char('/')));
        let _ = reduce(&mut state, Event::Input(Key::Char('b')));

        assert_eq!(
            reduce(&mut state, Event::Input(Key::Enter)),
            vec![Effect::Switch(beta.id.clone())]
        );
        assert_eq!(state.search.as_ref().unwrap().as_str(), "b");
    }

    #[test]
    fn active_search_refresh_reconciles_selection_without_stale_dispatch() {
        let alpha = identity("/worktrees/alpha", "alpha");
        let beta = identity("/worktrees/beta", "beta");
        let mut state = AppState::new(vec![alpha.clone(), beta.clone()]);
        let _ = reduce(&mut state, Event::Input(Key::Char('/')));
        let _ = reduce(&mut state, Event::Input(Key::Char('b')));
        assert_eq!(state.selected, Some(beta.id));

        let bravo = identity("/worktrees/bravo", "bravo");
        let _ = reduce(
            &mut state,
            Event::RefreshPublished(RefreshPublication {
                identities: vec![bravo.clone()],
                refs: None,
                statuses: BTreeMap::from([(bravo.id.clone(), WorktreeStatus::default())]),
                waiting_rows: BTreeSet::new(),
                updating_refs: false,
                warning: None,
            }),
        );
        assert_eq!(state.selected, Some(bravo.id));

        let _ = reduce(
            &mut state,
            Event::RefreshPublished(RefreshPublication {
                identities: vec![alpha],
                refs: None,
                statuses: BTreeMap::new(),
                waiting_rows: BTreeSet::new(),
                updating_refs: false,
                warning: None,
            }),
        );
        assert!(state.selected_visible().is_none());

        assert!(matches!(
            reduce(&mut state, Event::Input(Key::Enter)).as_slice(),
            [Effect::Unavailable { reason, .. }] if reason == "No worktree selected"
        ));
    }

    #[test]
    fn filtered_main_and_detached_rows_keep_their_action_eligibility() {
        let mut detached = identity("/worktrees/review", "review");
        detached.branch = None;
        detached.detached = true;
        let mut main = identity("/repos/trench", "trench");
        main.branch = Some("main".to_string());
        main.is_main = true;
        let mut state = AppState::new(vec![main.clone(), detached.clone()]);
        state
            .statuses
            .insert(main.id.clone(), WorktreeStatus::default());

        let _ = reduce(&mut state, Event::Input(Key::Char('/')));
        for character in "rev".chars() {
            let _ = reduce(&mut state, Event::Input(Key::Char(character)));
        }
        assert_eq!(state.selected, Some(detached.id.clone()));
        let _ = reduce(&mut state, Event::Input(Key::Escape));
        assert_eq!(
            reduce(&mut state, Event::Input(Key::Char('s'))),
            vec![Effect::Unavailable {
                action: Action::Sync,
                reason: "Detached worktrees cannot be synced".to_string(),
            }]
        );
        assert_eq!(
            reduce(&mut state, Event::Input(Key::Char('d'))),
            vec![Effect::OpenRemove(detached.id)]
        );

        let _ = reduce(&mut state, Event::Input(Key::Char('/')));
        for character in "mai".chars() {
            let _ = reduce(&mut state, Event::Input(Key::Char(character)));
        }
        assert_eq!(state.selected, Some(main.id));
        let _ = reduce(&mut state, Event::Input(Key::Escape));
        assert_eq!(
            reduce(&mut state, Event::Input(Key::Char('d'))),
            vec![Effect::Unavailable {
                action: Action::Remove,
                reason: "The main worktree cannot be removed".to_string(),
            }]
        );
    }

    #[test]
    fn create_success_refreshes_selects_and_keeps_a_five_second_navigation_notice() {
        let alpha = identity("/worktrees/alpha", "alpha");
        let beta = identity("/worktrees/beta", "beta");
        let mut state = AppState::new(vec![alpha.clone()]);
        let shown_at = Instant::now();

        assert_eq!(
            reduce(
                &mut state,
                Event::OperationSucceeded {
                    select: Some(beta.id.clone()),
                    message: "Created feature/beta".to_string(),
                    shown_at,
                },
            ),
            [Effect::Refresh]
        );
        assert_eq!(
            state
                .notification
                .as_ref()
                .map(|notice| notice.text.as_str()),
            Some("Created feature/beta")
        );

        let _ = reduce(
            &mut state,
            Event::RefreshPublished(RefreshPublication {
                identities: vec![alpha, beta.clone()],
                refs: None,
                statuses: BTreeMap::new(),
                waiting_rows: BTreeSet::new(),
                updating_refs: false,
                warning: None,
            }),
        );
        assert_eq!(state.selected, Some(beta.id));

        let _ = reduce(&mut state, Event::Input(Key::Up));
        assert!(state.notification.is_some());
        let _ = reduce(
            &mut state,
            Event::OperationSucceeded {
                select: None,
                message: "Created feature/beta".to_string(),
                shown_at,
            },
        );
        let _ = reduce(&mut state, Event::Input(Key::Char('?')));
        assert!(state.notification.is_none());
        let _ = reduce(
            &mut state,
            Event::OperationSucceeded {
                select: None,
                message: "Created feature/beta".to_string(),
                shown_at,
            },
        );
        let _ = reduce(
            &mut state,
            Event::NotificationTick(shown_at + NOTIFICATION_DURATION),
        );
        assert!(state.notification.is_none());
    }

    #[test]
    fn direct_explanations_use_the_shared_five_second_notice() {
        let mut state = AppState::new(Vec::new());
        let shown_at = Instant::now();

        assert!(reduce(
            &mut state,
            Event::NotificationShown {
                message: "Dirty worktrees cannot be synced".to_string(),
                shown_at,
            },
        )
        .is_empty());
        assert_eq!(
            state
                .notification
                .as_ref()
                .map(|notice| notice.text.as_str()),
            Some("Dirty worktrees cannot be synced")
        );

        let _ = reduce(
            &mut state,
            Event::NotificationTick(shown_at + NOTIFICATION_DURATION),
        );
        assert!(state.notification.is_none());
    }
}
