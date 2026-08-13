use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::{
    ref_catalog::RefSnapshot,
    tui::{
        keymap::{self, Action, Context, Key},
        refresh::RefreshPublication,
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
    OpenSearch,
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
    pub viewport: Viewport,
    pub inspector_override: Option<bool>,
    pub help_open: bool,
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
            viewport: Viewport {
                width: Viewport::MIN_WIDTH,
                height: Viewport::MIN_HEIGHT,
            },
            inspector_override: None,
            help_open: false,
        }
    }

    pub fn context(&self) -> Context {
        if self.viewport.is_tiny() {
            Context::Resize
        } else {
            Context::Cockpit
        }
    }

    pub fn selected_identity(&self) -> Option<&WorktreeIdentity> {
        let selected = self.selected.as_ref()?;
        self.identities.iter().find(|row| &row.id == selected)
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
            state.selected = state
                .selected
                .take()
                .filter(|selected| identities.iter().any(|row| &row.id == selected))
                .or_else(|| identities.first().map(|identity| identity.id.clone()));
            state
                .statuses
                .retain(|id, _| identities.iter().any(|row| &row.id == id));
            state.identities = identities;
        }
        Event::StatusLoaded { id, status } => {
            if state.identities.iter().any(|row| row.id == id) {
                state.statuses.insert(id, status);
            }
        }
        Event::RefreshPublished(publication) => {
            state.selected = state
                .selected
                .take()
                .filter(|selected| publication.identities.iter().any(|row| &row.id == selected))
                .or_else(|| {
                    publication
                        .identities
                        .first()
                        .map(|identity| identity.id.clone())
                });
            state.identities = publication.identities;
            state.statuses = publication.statuses;
            state.refs = publication.refs;
            state.refresh.waiting_rows = publication.waiting_rows;
            state.refresh.updating_refs = publication.updating_refs;
            state.refresh.warning = publication.warning;
        }
        Event::RefreshTick => {
            if state.refresh.updating_refs || !state.refresh.waiting_rows.is_empty() {
                state.refresh.spinner_tick = state.refresh.spinner_tick.wrapping_add(1);
            }
        }
        Event::Select(id) if state.identities.iter().any(|row| row.id == id) => {
            state.selected = Some(id);
        }
        Event::Select(_) => {}
        Event::Input(key) => {
            let Some(action) = keymap::action_for(state.context(), key) else {
                return Vec::new();
            };
            if let Some(reason) = unavailable_reason(state, action) {
                return vec![Effect::Unavailable {
                    action,
                    reason: reason.to_string(),
                }];
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
    let selected = state.selected_identity();
    match (action, selected) {
        (Action::Switch | Action::Open | Action::Sync | Action::Remove, None) => {
            Some("No worktree selected")
        }
        (Action::Sync, Some(identity)) if identity.detached => {
            Some("Detached worktrees cannot be synced")
        }
        (Action::Sync, Some(identity)) if !state.statuses.contains_key(&identity.id) => {
            Some("Git status is still loading")
        }
        (Action::Sync, Some(identity))
            if state
                .statuses
                .get(&identity.id)
                .is_some_and(|status| status.staged + status.modified + status.untracked > 0) =>
        {
            Some("Dirty worktrees cannot be synced")
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
            .selected
            .clone()
            .expect("eligibility checked selection")
    };
    match action {
        Action::Switch => vec![Effect::Switch(selected())],
        Action::Open => vec![Effect::Open(selected())],
        Action::Create => vec![Effect::OpenCreate],
        Action::Sync => vec![Effect::OpenSync(selected())],
        Action::Remove => vec![Effect::OpenRemove(selected())],
        Action::DeleteBranch => Vec::new(),
        Action::Search => vec![Effect::OpenSearch],
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
    if state.identities.is_empty() {
        state.selected = None;
        return;
    }
    let selected = state
        .selected
        .as_ref()
        .and_then(|id| state.identities.iter().position(|row| &row.id == id))
        .unwrap_or(0);
    let next = selected
        .saturating_add_signed(delta)
        .min(state.identities.len() - 1);
    state.selected = Some(state.identities[next].id.clone());
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
    fn tiny_view_routes_only_quit_and_help() {
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
        let _ = reduce(&mut state, Event::Input(Key::Char('?')));
        assert!(state.help_open);
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
}
