pub mod screens;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{layout::Alignment, widgets::Paragraph, Frame};

use crate::paths;
use crate::state::Database;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    List,
    Detail,
    Create,
    Help,
    SyncPicker,
}

type PanicHook = dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync;

/// Stores the pre-TUI panic hook so `restore_panic_hook` can put it back.
static PREV_PANIC_HOOK: Mutex<Option<Arc<PanicHook>>> = Mutex::new(None);

/// Launch the TUI. This is the single public entry point.
pub fn run() -> Result<()> {
    install_panic_hook();
    let mut terminal = ratatui::init();
    let mut app = App::new();

    // Load worktree data before entering the event loop
    app.refresh_list();

    let result = (|| -> Result<()> {
        while app.is_running() {
            terminal.draw(|frame| app.ui(frame))?;

            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key_event(key);
                }
            }
        }
        Ok(())
    })();

    ratatui::restore();
    restore_panic_hook();
    result
}

fn install_panic_hook() {
    let original: Arc<PanicHook> = Arc::from(std::panic::take_hook());
    PREV_PANIC_HOOK
        .lock()
        .unwrap()
        .replace(Arc::clone(&original));
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        original(info);
    }));
}

fn restore_panic_hook() {
    let _ = std::panic::take_hook();
    if let Some(hook) = PREV_PANIC_HOOK.lock().unwrap().take() {
        std::panic::set_hook(Box::new(move |info| hook(info)));
    }
}

pub struct App {
    running: bool,
    nav_stack: Vec<Screen>,
    pub list_state: screens::list::ListState,
    pub detail_state: Option<screens::detail::DetailState>,
    pub sync_picker_state: Option<screens::sync_picker::SyncPickerState>,
}

impl App {
    pub fn new() -> Self {
        Self {
            running: true,
            nav_stack: vec![Screen::List],
            list_state: screens::list::ListState::new(vec![]),
            detail_state: None,
            sync_picker_state: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn active_screen(&self) -> Screen {
        *self
            .nav_stack
            .last()
            .expect("nav stack must never be empty")
    }

    pub fn nav_stack_depth(&self) -> usize {
        self.nav_stack.len()
    }

    pub fn push_screen(&mut self, screen: Screen) {
        self.nav_stack.push(screen);
    }

    pub fn ui(&self, frame: &mut Frame) {
        match self.active_screen() {
            Screen::List => screens::list::render(&self.list_state, frame, frame.area()),
            Screen::Detail => {
                if let Some(ref detail) = self.detail_state {
                    screens::detail::render(detail, frame, frame.area());
                } else {
                    let placeholder = Paragraph::new("trench TUI — press q to quit")
                        .alignment(Alignment::Center);
                    frame.render_widget(placeholder, frame.area());
                }
            }
            _ => {
                let placeholder =
                    Paragraph::new("trench TUI — press q to quit").alignment(Alignment::Center);
                frame.render_widget(placeholder, frame.area());
            }
        }
    }

    pub fn pop_screen(&mut self) {
        if self.nav_stack.len() > 1 {
            self.nav_stack.pop();
            if self.active_screen() == Screen::List {
                self.refresh_list();
            }
        } else {
            self.running = false;
        }
    }

    fn open_db() -> Option<(std::path::PathBuf, Database)> {
        let cwd = std::env::current_dir().ok()?;
        let db_path = paths::data_dir().ok()?.join("trench.db");
        let db = Database::open(&db_path).ok()?;
        Some((cwd, db))
    }

    /// Reload worktree data from git + DB for the list screen.
    pub fn refresh_list(&mut self) {
        let Some((cwd, db)) = Self::open_db() else { return };
        if let Ok(rows) = screens::list::load_worktrees(&cwd, &db, &[]) {
            let prev_selected = self.list_state.selected;
            self.list_state = screens::list::ListState::new(rows);
            if self.list_state.rows.len() > prev_selected {
                self.list_state.selected = prev_selected;
            }
        }
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        // Global keys handled at app level
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.running = false,
            (KeyCode::Char('?'), _) => self.push_screen(Screen::Help),
            (KeyCode::Esc, _) | (KeyCode::Char('q'), _) => self.pop_screen(),
            _ => self.handle_screen_key(key),
        }
    }

    fn handle_screen_key(&mut self, key: KeyEvent) {
        match self.active_screen() {
            Screen::List => self.handle_list_key(key),
            Screen::Detail => self.handle_detail_key(key),
            Screen::SyncPicker => self.handle_sync_picker_key(key),
            Screen::Create => {}
            Screen::Help => {}
        }
    }

    /// If the selected worktree is unmanaged, silently adopt it into the DB.
    fn adopt_selected_if_unmanaged(&mut self) {
        let row = match self.list_state.rows.get(self.list_state.selected) {
            Some(r) if !r.managed => r,
            _ => return,
        };
        let identifier = if row.branch == "(detached)" {
            row.name.clone()
        } else {
            row.branch.clone()
        };

        let Some((cwd, db)) = Self::open_db() else { return };
        let repo_info = match crate::git::discover_repo(&cwd) {
            Ok(r) => r,
            Err(_) => return,
        };
        let _ = crate::adopt::resolve_or_adopt(&identifier, &repo_info, &db);
        self.refresh_list();
    }

    fn load_detail(&mut self, name: &str) -> bool {
        self.detail_state = None;
        let Some((cwd, db)) = Self::open_db() else { return false };
        self.detail_state = Some(screens::detail::load_detail(name, &cwd, &db));
        true
    }

    fn handle_detail_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('s') => {} // TODO: trigger sync
            KeyCode::Char('o') => {} // TODO: open in $EDITOR
            _ => {}
        }
    }

    fn handle_sync_picker_key(&mut self, _key: KeyEvent) {
    }

    fn handle_list_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let identity = self
                    .list_state
                    .rows
                    .get(self.list_state.selected)
                    .map(|r| r.name.clone());
                self.adopt_selected_if_unmanaged();
                if let Some(ref name) = identity {
                    if let Some(idx) = self.list_state.rows.iter().position(|r| r.name == *name) {
                        self.list_state.selected = idx;
                    }
                }
                // Load detail data for the selected worktree
                if let Some(name) = identity {
                    if self.load_detail(&name) {
                        self.push_screen(Screen::Detail);
                    }
                }
            }
            KeyCode::Char('n') => self.push_screen(Screen::Create),
            KeyCode::Down | KeyCode::Char('j') => self.list_state.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.list_state.select_previous(),
            KeyCode::Char('s') => {} // TODO: trigger sync
            KeyCode::Char('D') => {} // TODO: trigger delete with confirmation
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serial_test::serial;

    #[test]
    fn app_starts_in_running_state() {
        let app = App::new();
        assert!(app.is_running(), "newly created app should be running");
    }

    #[test]
    fn app_starts_on_list_screen() {
        let app = App::new();
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "app should start on the List screen"
        );
    }

    #[test]
    fn screen_enum_has_five_variants() {
        // Verify all five screen variants exist and are distinct
        let screens = [Screen::List, Screen::Detail, Screen::Create, Screen::Help, Screen::SyncPicker];
        for (i, a) in screens.iter().enumerate() {
            for (j, b) in screens.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn question_mark_pushes_help_from_any_screen() {
        let mut app = App::new();
        assert_eq!(app.active_screen(), Screen::List);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Help,
            "? should push Help screen"
        );
        assert_eq!(app.nav_stack_depth(), 2, "stack should have List + Help");
    }

    #[test]
    fn esc_pops_back_to_previous_screen() {
        let mut app = App::new();
        // Push Help from List
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 2);

        // Esc should pop back to List
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Esc should pop back to List"
        );
        assert_eq!(app.nav_stack_depth(), 1);
        assert!(
            app.is_running(),
            "app should still be running after popping"
        );
    }

    #[test]
    fn esc_on_root_screen_quits_app() {
        let mut app = App::new();
        assert_eq!(app.active_screen(), Screen::List);
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(
            !app.is_running(),
            "Esc on root screen (List) should quit the app"
        );
    }

    #[test]
    fn q_on_root_screen_quits_app() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.is_running(), "q on root screen should quit the app");
    }

    #[test]
    fn q_on_non_root_screen_pops_back() {
        let mut app = App::new();
        // Push Help
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);

        // q should pop back, not quit
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "q on non-root should pop back to List"
        );
        assert!(app.is_running(), "q on non-root should not quit the app");
    }

    #[test]
    fn app_exits_on_ctrl_c() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.is_running(), "app should stop after Ctrl+C");
    }

    #[test]
    fn enter_on_list_with_rows_pushes_detail() {
        let mut app = app_with_rows();
        assert_eq!(app.active_screen(), Screen::List);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Detail,
            "Enter on List with rows should push Detail screen"
        );
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn n_on_list_pushes_create() {
        let mut app = App::new();
        assert_eq!(app.active_screen(), Screen::List);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Create,
            "n on List should push Create screen"
        );
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn enter_on_non_list_screen_does_nothing() {
        let mut app = App::new();
        // Push Help
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);

        // Enter on Help should not push anything
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Help,
            "Enter on Help should do nothing"
        );
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn deep_stack_navigation_push_pop_sequence() {
        let mut app = app_with_rows();
        // List → Detail → Help → pop → pop → List
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Detail);
        assert_eq!(app.nav_stack_depth(), 2);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 3);

        // Pop Help → Detail
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Detail);
        assert_eq!(app.nav_stack_depth(), 2);

        // Pop Detail → List
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert_eq!(app.nav_stack_depth(), 1);
        assert!(app.is_running());

        // Pop List → quit
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.is_running());
    }

    #[test]
    fn question_mark_opens_help_from_detail_screen() {
        let mut app = app_with_rows();
        // Navigate to Detail first
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Detail);

        // ? should still open Help from Detail
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 3);
    }

    fn app_with_rows() -> App {
        use screens::list::WorktreeRow;
        let mut app = App::new();
        app.list_state = screens::list::ListState::new(vec![
            WorktreeRow {
                name: "feat-a".into(),
                branch: "feat/a".into(),
                status: "clean".into(),
                ahead_behind: "+0/-0".into(),
                managed: true,
            },
            WorktreeRow {
                name: "feat-b".into(),
                branch: "feat/b".into(),
                status: "~2".into(),
                ahead_behind: "+1/-0".into(),
                managed: true,
            },
            WorktreeRow {
                name: "main".into(),
                branch: "main".into(),
                status: "clean".into(),
                ahead_behind: "-".into(),
                managed: false,
            },
        ]);
        app
    }

    #[test]
    fn j_key_moves_selection_down() {
        let mut app = app_with_rows();
        assert_eq!(app.list_state.selected, 0);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.list_state.selected, 1);
    }

    #[test]
    fn k_key_moves_selection_up() {
        let mut app = app_with_rows();
        app.list_state.selected = 2;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.list_state.selected, 1);
    }

    #[test]
    fn arrow_down_moves_selection_down() {
        let mut app = app_with_rows();
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.list_state.selected, 1);
    }

    #[test]
    fn arrow_up_moves_selection_up() {
        let mut app = app_with_rows();
        app.list_state.selected = 1;
        app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.list_state.selected, 0);
    }

    #[test]
    fn s_on_list_is_handled() {
        let mut app = app_with_rows();
        // s should not crash and should not quit or push a screen
        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(app.is_running());
        assert_eq!(app.active_screen(), Screen::List);
    }

    #[test]
    fn shift_d_on_list_is_handled() {
        let mut app = app_with_rows();
        // D (shift+d) should not crash and should not quit or push a screen
        app.handle_key_event(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        assert!(app.is_running());
        assert_eq!(app.active_screen(), Screen::List);
    }

    #[test]
    fn enter_on_empty_list_does_not_push_detail() {
        let mut app = App::new();
        // Empty list — no rows
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Enter on empty list should stay on List"
        );
        assert!(app.detail_state.is_none());
    }

    #[test]
    fn app_ignores_unbound_keys() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(app.is_running(), "unbound keys should not stop the app");
    }

    #[test]
    #[serial]
    fn restore_panic_hook_restores_prior_hook() {
        use std::panic::{self, catch_unwind};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        // Install marker as the pre-existing hook BEFORE TUI touches anything
        let marker_ran = Arc::new(AtomicBool::new(false));
        let marker = marker_ran.clone();
        panic::set_hook(Box::new(move |_| {
            marker.store(true, Ordering::SeqCst);
        }));

        install_panic_hook(); // wraps marker in TUI hook
        restore_panic_hook(); // must restore marker, not default

        let _ = catch_unwind(|| panic!("test panic"));

        // Clean up so we don't affect other tests
        let _ = panic::take_hook();

        assert!(
            marker_ran.load(Ordering::SeqCst),
            "prior hook should run after restore, proving TUI hook was removed and original restored"
        );
    }

    #[test]
    fn list_screen_renders_empty_state_by_default() {
        let app = App::new();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            content.contains("No worktrees"),
            "empty list should show 'No worktrees' message, got: {:?}",
            content.trim()
        );
    }

    #[test]
    fn non_list_screen_renders_placeholder() {
        let mut app = App::new();
        app.push_screen(Screen::Help);
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            content.contains("trench TUI"),
            "non-list screens should show placeholder, got: {:?}",
            content.trim()
        );
    }

    fn sample_detail_state() -> screens::detail::DetailState {
        screens::detail::DetailState {
            name: "feat-a".into(),
            branch: "feat/a".into(),
            path: "/tmp/wt/feat-a".into(),
            base_branch: "main".into(),
            ahead_behind: "+0/-0".into(),
            created: "2026-03-10".into(),
            last_accessed: "2026-03-11".into(),
            hook_status: "success".into(),
            hook_timestamp: "2026-03-10".into(),
            changed_files: vec![("file.rs".into(), "modified".into())],
            commits: vec![("abc1234".into(), "test commit".into())],
        }
    }

    #[test]
    fn detail_screen_renders_detail_state_not_placeholder() {
        let mut app = App::new();
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);

        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(
            content.contains("feat/a"),
            "detail screen should show branch from detail_state, got: {:?}",
            content.trim()
        );
        assert!(
            !content.contains("trench TUI"),
            "detail screen should NOT show placeholder"
        );
    }

    #[test]
    fn detail_screen_shows_changed_files_and_commits() {
        let mut app = App::new();
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);

        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(content.contains("file.rs"), "should show changed file");
        assert!(content.contains("abc1234"), "should show commit hash");
    }

    #[test]
    fn s_on_detail_is_handled_without_crash() {
        let mut app = App::new();
        app.push_screen(Screen::Detail);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(app.is_running(), "s on detail should not crash or quit");
        assert_eq!(app.active_screen(), Screen::Detail);
    }

    #[test]
    fn o_on_detail_is_handled_without_crash() {
        let mut app = App::new();
        app.push_screen(Screen::Detail);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert!(app.is_running(), "o on detail should not crash or quit");
        assert_eq!(app.active_screen(), Screen::Detail);
    }

    #[test]
    fn screen_enum_has_sync_picker_variant() {
        let screen = Screen::SyncPicker;
        assert_ne!(screen, Screen::List);
        assert_ne!(screen, Screen::Detail);
    }

    #[test]
    fn app_has_sync_picker_state_initially_none() {
        let app = App::new();
        assert!(app.sync_picker_state.is_none());
    }

    #[test]
    fn push_sync_picker_screen_works() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);
        assert_eq!(app.active_screen(), Screen::SyncPicker);
        assert_eq!(app.nav_stack_depth(), 2);
    }
}
