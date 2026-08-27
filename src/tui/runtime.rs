use std::{collections::VecDeque, path::Path};

use anyhow::Result;
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind};

use crate::{
    config,
    tui::{
        app::{self, AppState, Effect, Event, WorktreeId, WorktreeIdentity, WorktreeStatus},
        cockpit,
        keymap::Key,
        theme,
    },
    worktree_catalog::WorktreeCatalog,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogRequest {
    Create,
    Sync(WorktreeId),
    Remove(WorktreeId),
    Search,
}

#[derive(Debug, Default)]
pub struct DialogRegistry {
    pending: Option<DialogRequest>,
}

impl DialogRegistry {
    pub fn pending(&self) -> Option<&DialogRequest> {
        self.pending.as_ref()
    }

    pub fn take(&mut self) -> Option<DialogRequest> {
        self.pending.take()
    }

    pub fn register(&mut self, request: DialogRequest) {
        self.pending = Some(request);
    }
}

struct CatalogSnapshot {
    catalog: WorktreeCatalog,
    pending_statuses: VecDeque<WorktreeId>,
}

impl CatalogSnapshot {
    fn discover(cwd: &Path, default_base: Option<&str>) -> Result<(Self, Vec<WorktreeIdentity>)> {
        let catalog = WorktreeCatalog::discover(cwd)?.with_base(default_base);
        let identities = catalog
            .identities()
            .iter()
            .map(|identity| WorktreeIdentity {
                id: WorktreeId::new(identity.path.clone()),
                worktree: identity.worktree.clone(),
                branch: identity.branch.clone(),
                path: identity.path.clone(),
                head: identity.head.clone(),
                is_main: identity.is_main,
                is_current: identity.is_current,
                detached: identity.detached,
            })
            .collect::<Vec<_>>();
        let pending_statuses = identities
            .iter()
            .map(|identity| identity.id.clone())
            .collect();
        Ok((
            Self {
                catalog,
                pending_statuses,
            },
            identities,
        ))
    }

    fn populate_next_status(&mut self, state: &mut AppState) {
        let Some(id) = self.pending_statuses.pop_front() else {
            return;
        };
        let Ok(status) = self.catalog.status(id.as_path()) else {
            return;
        };
        let _ = app::reduce(
            state,
            Event::StatusLoaded {
                id,
                status: WorktreeStatus {
                    base: status.base,
                    staged: status.staged,
                    modified: status.modified,
                    untracked: status.untracked,
                    ahead: status.ahead,
                    behind: status.behind,
                },
            },
        );
    }
}

pub fn run() -> Result<Option<String>> {
    let cwd = std::env::current_dir()?;
    let repo = crate::git::discover_repo(&cwd)?;
    let global = config::load_global_config()?;
    let project = config::load_project_config(&repo.path)?;
    let resolved = config::resolve_config(None, project.as_ref(), &global);
    let (mut catalog, identities) =
        CatalogSnapshot::discover(&cwd, resolved.git.default_base.as_deref())?;
    let mut state = AppState::new(identities);
    let selected_theme = theme::from_name(&resolved.ui.theme);
    let mut dialogs = DialogRegistry::default();

    super::install_panic_hook();
    let mut terminal = ratatui::init();
    let result = (|| -> Result<Option<String>> {
        'event_loop: loop {
            let (width, height) = crossterm::terminal::size()?;
            let _ = app::reduce(&mut state, Event::ViewportChanged { width, height });
            terminal.draw(|frame| {
                cockpit::render(&state, frame, frame.area(), &selected_theme);
            })?;

            // Identity rows reach the terminal before any status work. Status remains
            // synchronous until the refresh coordinator replaces this queue in #144.
            catalog.populate_next_status(&mut state);

            if !event::poll(std::time::Duration::from_millis(50))? {
                continue;
            }
            let TerminalEvent::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.code == KeyCode::Char('c')
                && key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL)
            {
                break 'event_loop Ok(None);
            }
            let Some(key) = translate_key(key) else {
                continue;
            };
            for effect in app::reduce(&mut state, Event::Input(key)) {
                match effect {
                    Effect::Switch(id) => {
                        break 'event_loop Ok(Some(id.as_path().to_string_lossy().into_owned()));
                    }
                    Effect::Open(_) => {
                        // Registered by #142, which owns terminal editor suspension.
                    }
                    Effect::OpenCreate => dialogs.register(DialogRequest::Create),
                    Effect::OpenSync(id) => dialogs.register(DialogRequest::Sync(id)),
                    Effect::OpenRemove(id) => dialogs.register(DialogRequest::Remove(id)),
                    Effect::OpenSearch => dialogs.register(DialogRequest::Search),
                    Effect::Refresh => {
                        let (replacement, identities) =
                            CatalogSnapshot::discover(&cwd, resolved.git.default_base.as_deref())?;
                        catalog = replacement;
                        let _ = app::reduce(&mut state, Event::IdentitiesLoaded(identities));
                    }
                    Effect::Quit => break 'event_loop Ok(None),
                    Effect::Unavailable { .. } => {}
                }
            }

            // Later cockpit layers consume the typed request and render their overlay.
            // Keeping the registry live here makes unhandled actions observable to them.
            let _ = dialogs.pending();
        }
    })();

    ratatui::restore();
    super::restore_panic_hook();
    result
}

fn translate_key(key: KeyEvent) -> Option<Key> {
    match key.code {
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Esc => Some(Key::Escape),
        KeyCode::Up => Some(Key::Up),
        KeyCode::Down => Some(Key::Down),
        KeyCode::Char(character) => Some(Key::Char(character)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_keys_translate_to_the_pure_reducer_vocabulary() {
        assert_eq!(
            translate_key(KeyEvent::new(
                KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            )),
            Some(Key::Enter)
        );
        assert_eq!(
            translate_key(KeyEvent::new(
                KeyCode::Char('?'),
                crossterm::event::KeyModifiers::NONE,
            )),
            Some(Key::Char('?'))
        );
        assert_eq!(
            translate_key(KeyEvent::new(
                KeyCode::PageDown,
                crossterm::event::KeyModifiers::NONE,
            )),
            None
        );
    }

    #[test]
    fn dialog_registry_is_a_replaceable_operation_seam() {
        let mut dialogs = DialogRegistry::default();
        dialogs.register(DialogRequest::Search);
        assert_eq!(dialogs.pending(), Some(&DialogRequest::Search));
        assert_eq!(dialogs.take(), Some(DialogRequest::Search));
        assert_eq!(dialogs.pending(), None);
    }
}
