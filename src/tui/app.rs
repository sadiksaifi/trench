use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::tui::keymap::{self, Action, Context, Key};

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
    Select(WorktreeId),
    Input(Key),
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
    pub selected: Option<WorktreeId>,
}

impl AppState {
    pub fn new(identities: Vec<WorktreeIdentity>) -> Self {
        let selected = identities.first().map(|identity| identity.id.clone());
        Self {
            identities,
            statuses: BTreeMap::new(),
            selected,
        }
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
            state.identities = identities;
        }
        Event::StatusLoaded { id, status } => {
            state.statuses.insert(id, status);
        }
        Event::Select(id) if state.identities.iter().any(|row| row.id == id) => {
            state.selected = Some(id);
        }
        Event::Select(_) => {}
        Event::Input(key) => {
            let Some(action) = keymap::action_for(Context::Cockpit, key) else {
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
    }
    Vec::new()
}

pub fn unavailable_reason(state: &AppState, action: Action) -> Option<&'static str> {
    let selected = state
        .selected
        .as_ref()
        .and_then(|id| state.identities.iter().find(|row| &row.id == id));
    match (action, selected) {
        (Action::Switch | Action::Open | Action::Sync | Action::Remove, None) => {
            Some("No worktree selected")
        }
        (Action::Sync, Some(identity)) if identity.detached => {
            Some("Detached worktrees cannot be synced")
        }
        (Action::Remove, Some(identity)) if identity.is_main => {
            Some("The main worktree cannot be removed")
        }
        _ => None,
    }
}

fn reduce_action(state: &mut AppState, action: Action) -> Vec<Effect> {
    let selected = || state.selected.clone().expect("eligibility checked selection");
    match action {
        Action::Switch => vec![Effect::Switch(selected())],
        Action::Open => vec![Effect::Open(selected())],
        Action::Create => vec![Effect::OpenCreate],
        Action::Sync => vec![Effect::OpenSync(selected())],
        Action::Remove => vec![Effect::OpenRemove(selected())],
        Action::Search => vec![Effect::OpenSearch],
        Action::Refresh => vec![Effect::Refresh],
        Action::Quit => vec![Effect::Quit],
        Action::ToggleInspector | Action::SelectNext | Action::SelectPrevious | Action::Help => {
            Vec::new()
        }
    }
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
        reduce(&mut state, Event::Select(beta.id.clone()));

        reduce(
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
}
