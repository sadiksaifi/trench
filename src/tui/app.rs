use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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

pub fn reduce(state: &mut AppState, event: Event) {
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
}
