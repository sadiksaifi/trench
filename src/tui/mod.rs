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
    DeleteConfirm,
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

            if let Some(path) = app.editor_request.take() {
                ratatui::restore();
                let editor = std::env::var("EDITOR")
                    .or_else(|_| std::env::var("VISUAL"))
                    .unwrap_or_else(|_| "vi".into());
                let _ = std::process::Command::new(&editor).arg(&path).status();
                terminal = ratatui::init();
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
    pub create_state: Option<screens::create::CreateState>,
    pub sync_picker_state: Option<screens::sync_picker::SyncPickerState>,
    pub delete_confirm_state: Option<screens::delete_confirm::DeleteConfirmState>,
    pub editor_request: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            running: true,
            nav_stack: vec![Screen::List],
            list_state: screens::list::ListState::new(vec![]),
            detail_state: None,
            create_state: None,
            sync_picker_state: None,
            delete_confirm_state: None,
            editor_request: None,
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
            Screen::SyncPicker => {
                if let Some(ref picker) = self.sync_picker_state {
                    screens::sync_picker::render(picker, frame, frame.area());
                } else {
                    let placeholder =
                        Paragraph::new("trench TUI — press q to quit").alignment(Alignment::Center);
                    frame.render_widget(placeholder, frame.area());
                }
            }
            Screen::DeleteConfirm => {
                // Render list underneath, then overlay the dialog
                screens::list::render(&self.list_state, frame, frame.area());
                if let Some(ref confirm) = self.delete_confirm_state {
                    screens::delete_confirm::render(confirm, frame, frame.area());
                }
            }
            Screen::Help => {
                // Render underlying screen first, then overlay help
                self.render_underlying_screen(frame);
                screens::help::render(frame, frame.area());
            }
            Screen::Create => {
                if let Some(ref create) = self.create_state {
                    screens::create::render(create, frame, frame.area());
                } else {
                    let placeholder =
                        Paragraph::new("trench TUI — press q to quit").alignment(Alignment::Center);
                    frame.render_widget(placeholder, frame.area());
                }
            }
        }
    }

    /// Render the screen underneath the current overlay (e.g. for Help).
    fn render_underlying_screen(&self, frame: &mut Frame) {
        let underlying = self.nav_stack.iter().rev().nth(1).copied();
        match underlying {
            Some(Screen::Detail) => {
                if let Some(ref detail) = self.detail_state {
                    screens::detail::render(detail, frame, frame.area());
                }
            }
            Some(Screen::Create) => {
                if let Some(ref create) = self.create_state {
                    screens::create::render(create, frame, frame.area());
                }
            }
            Some(Screen::SyncPicker) => {
                if let Some(ref picker) = self.sync_picker_state {
                    screens::sync_picker::render(picker, frame, frame.area());
                }
            }
            Some(Screen::DeleteConfirm) => {
                screens::list::render(&self.list_state, frame, frame.area());
                if let Some(ref confirm) = self.delete_confirm_state {
                    screens::delete_confirm::render(confirm, frame, frame.area());
                }
            }
            _ => screens::list::render(&self.list_state, frame, frame.area()),
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

    fn is_create_branch_text_entry_active(&self) -> bool {
        self.active_screen() == Screen::Create
            && self
                .create_state
                .as_ref()
                .is_some_and(|s| !s.is_result_mode() && s.focused_field == screens::create::CreateField::Branch)
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        // Global keys handled at app level
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.running = false,
            (KeyCode::Char('?'), _) => {
                if self.active_screen() == Screen::Help {
                    self.pop_screen();
                } else {
                    self.push_screen(Screen::Help);
                }
            }
            (KeyCode::Esc, _) => {
                match self.active_screen() {
                    Screen::DeleteConfirm => {
                        self.delete_confirm_state = None;
                    }
                    Screen::SyncPicker => {
                        self.sync_picker_state = None;
                    }
                    Screen::Create => {
                        self.create_state = None;
                    }
                    _ => {}
                }
                self.pop_screen();
            }
            (KeyCode::Char('q'), _) if !self.is_create_branch_text_entry_active() => {
                match self.active_screen() {
                    Screen::DeleteConfirm => {
                        self.delete_confirm_state = None;
                    }
                    Screen::SyncPicker => {
                        self.sync_picker_state = None;
                    }
                    Screen::Create => {
                        self.create_state = None;
                    }
                    _ => {}
                }
                self.pop_screen();
            }
            _ => self.handle_screen_key(key),
        }
    }

    fn handle_screen_key(&mut self, key: KeyEvent) {
        match self.active_screen() {
            Screen::List => self.handle_list_key(key),
            Screen::Detail => self.handle_detail_key(key),
            Screen::SyncPicker => self.handle_sync_picker_key(key),
            Screen::DeleteConfirm => self.handle_delete_confirm_key(key),
            Screen::Create => self.handle_create_key(key),
            Screen::Help => {}
        }
    }

    fn handle_delete_confirm_key(&mut self, key: KeyEvent) {
        let in_result_mode = self
            .delete_confirm_state
            .as_ref()
            .is_some_and(|s| s.is_result_mode());

        if in_result_mode {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.delete_confirm_state = None;
                    while self.active_screen() != Screen::List {
                        self.pop_screen();
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => self.execute_delete(),
            KeyCode::Char('n') => {
                self.delete_confirm_state = None;
                self.pop_screen();
            }
            _ => {}
        }
    }

    fn execute_delete(&mut self) {
        let confirm = match self.delete_confirm_state.as_ref() {
            Some(c) => c,
            None => return,
        };
        let worktree_name = confirm.worktree_name.clone();

        let Some((cwd, db)) = Self::open_db() else {
            if let Some(ref mut c) = self.delete_confirm_state {
                c.result = Some(screens::delete_confirm::DeleteResultMessage {
                    success: false,
                    message: "Failed to open database".into(),
                });
            }
            return;
        };

        match crate::cli::commands::remove::execute(&worktree_name, &cwd, &db, false) {
            Ok(result) => {
                let msg = format!("Removed '{}'", result.name);
                if let Some(ref mut c) = self.delete_confirm_state {
                    c.result = Some(screens::delete_confirm::DeleteResultMessage {
                        success: true,
                        message: msg,
                    });
                }
            }
            Err(e) => {
                if let Some(ref mut c) = self.delete_confirm_state {
                    c.result = Some(screens::delete_confirm::DeleteResultMessage {
                        success: false,
                        message: format!("Delete failed: {e:#}"),
                    });
                }
            }
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
            KeyCode::Char('s') => {
                if let Some(ref detail) = self.detail_state {
                    self.sync_picker_state =
                        Some(screens::sync_picker::SyncPickerState::new(&detail.name));
                    self.push_screen(Screen::SyncPicker);
                }
            }
            KeyCode::Char('o') => {
                if let Some(ref detail) = self.detail_state {
                    self.editor_request = Some(detail.path.clone());
                }
            }
            _ => {}
        }
    }

    fn handle_sync_picker_key(&mut self, key: KeyEvent) {
        let in_result_mode = self
            .sync_picker_state
            .as_ref()
            .is_some_and(|p| p.is_result_mode());
        if in_result_mode {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    while self.active_screen() != Screen::List {
                        self.pop_screen();
                    }
                    self.sync_picker_state = None;
                }
                _ => {}
            }
            return;
        }

        if let Some(ref mut picker) = self.sync_picker_state {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => picker.select_next(),
                KeyCode::Up | KeyCode::Char('k') => picker.select_previous(),
                KeyCode::Enter => self.execute_sync(),
                _ => {}
            }
        }
    }

    fn execute_sync(&mut self) {
        let picker = match self.sync_picker_state.as_ref() {
            Some(p) => p,
            None => return,
        };
        let strategy = picker.confirmed_strategy();
        let worktree_name = picker.worktree_name.clone();

        let Some((cwd, db)) = Self::open_db() else {
            if let Some(ref mut p) = self.sync_picker_state {
                p.result = Some(screens::sync_picker::SyncResultMessage {
                    success: false,
                    message: "Failed to open database".into(),
                });
            }
            return;
        };

        match crate::cli::commands::sync::execute(&worktree_name, &cwd, &db, strategy) {
            Ok(result) => {
                let msg = format!(
                    "Synced '{}' via {}\nBefore: +{}/-{}  After: +{}/-{}",
                    result.name,
                    result.strategy,
                    result.before_ahead,
                    result.before_behind,
                    result.after_ahead,
                    result.after_behind,
                );
                if let Some(ref mut p) = self.sync_picker_state {
                    p.result = Some(screens::sync_picker::SyncResultMessage {
                        success: true,
                        message: msg,
                    });
                }
            }
            Err(e) => {
                if let Some(ref mut p) = self.sync_picker_state {
                    p.result = Some(screens::sync_picker::SyncResultMessage {
                        success: false,
                        message: format!("Sync failed: {e:#}"),
                    });
                }
            }
        }
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
            KeyCode::Char('n') => {
                self.init_create_form();
                self.push_screen(Screen::Create);
            }
            KeyCode::Down | KeyCode::Char('j') => self.list_state.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.list_state.select_previous(),
            KeyCode::Char('s') => {
                if let Some(row) = self.list_state.rows.get(self.list_state.selected) {
                    self.sync_picker_state =
                        Some(screens::sync_picker::SyncPickerState::new(&row.name));
                    self.push_screen(Screen::SyncPicker);
                }
            }
            KeyCode::Char('D') => {
                if let Some(row) = self.list_state.rows.get(self.list_state.selected) {
                    self.delete_confirm_state = Some(
                        screens::delete_confirm::DeleteConfirmState::new(
                            &row.name,
                            &row.path,
                            &row.branch,
                        ),
                    );
                    self.push_screen(Screen::DeleteConfirm);
                }
            }
            _ => {}
        }
    }

    /// Initialize the create form state from the current repo context.
    fn init_create_form(&mut self) {
        let mut base_branches = vec!["main".to_string()];
        let mut repo_name = String::new();
        let template = paths::DEFAULT_WORKTREE_TEMPLATE.to_string();

        if let Some((cwd, _db)) = Self::open_db() {
            if let Ok(repo_info) = crate::git::discover_repo(&cwd) {
                repo_name = repo_info.name.clone();
                // Collect local branches for the base selector
                base_branches = crate::git::list_local_branches(&repo_info.path)
                    .unwrap_or_else(|_| vec![repo_info.default_branch.clone()]);
                // Ensure default branch is first
                if let Some(pos) = base_branches.iter().position(|b| b == &repo_info.default_branch) {
                    if pos != 0 {
                        let default = base_branches.remove(pos);
                        base_branches.insert(0, default);
                    }
                }
            }
        }
        if base_branches.is_empty() {
            base_branches.push("main".to_string());
        }

        self.create_state = Some(screens::create::CreateState::new(
            base_branches,
            repo_name,
            template,
        ));
    }

    fn handle_create_key(&mut self, key: KeyEvent) {
        use screens::create::CreateField;

        let in_result_mode = self
            .create_state
            .as_ref()
            .is_some_and(|s| s.is_result_mode());

        if in_result_mode {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.create_state = None;
                    while self.active_screen() != Screen::List {
                        self.pop_screen();
                    }
                }
                _ => {}
            }
            return;
        }

        let Some(ref mut state) = self.create_state else {
            return;
        };

        match state.focused_field {
            CreateField::Branch => match key.code {
                KeyCode::Char(c) => {
                    state.insert_char(c);
                    state.update_path_preview();
                    let _ = state.validate();
                }
                KeyCode::Backspace => {
                    state.backspace();
                    state.update_path_preview();
                    let _ = state.validate();
                }
                KeyCode::Left => state.cursor_left(),
                KeyCode::Right => state.cursor_right(),
                KeyCode::Tab => state.focus_next(),
                KeyCode::BackTab => state.focus_previous(),
                KeyCode::Enter => state.focus_next(),
                _ => {}
            },
            CreateField::Base => match key.code {
                KeyCode::Left | KeyCode::Char('h') => state.select_previous_base(),
                KeyCode::Right | KeyCode::Char('l') => state.select_next_base(),
                KeyCode::Tab => state.focus_next(),
                KeyCode::BackTab => state.focus_previous(),
                KeyCode::Enter => state.focus_next(),
                _ => {}
            },
            CreateField::Hooks => match key.code {
                KeyCode::Char(' ') => state.toggle_hooks(),
                KeyCode::Tab => state.focus_next(),
                KeyCode::BackTab => state.focus_previous(),
                KeyCode::Enter => {
                    // On last field, Enter triggers create
                    self.execute_create();
                }
                _ => {}
            },
        }
    }

    fn execute_create(&mut self) {
        let state = match self.create_state.as_mut() {
            Some(s) => s,
            None => return,
        };

        // Validate first
        if state.validate().is_err() {
            return;
        }

        let branch = state.branch_input.trim().to_string();
        let base = state.selected_base_branch().map(|s| s.to_string());

        let Some((cwd, db)) = Self::open_db() else {
            state.result = Some(screens::create::CreateResultMessage {
                success: false,
                message: "Failed to open database".into(),
            });
            return;
        };

        let worktree_root = match paths::worktree_root() {
            Ok(r) => r,
            Err(e) => {
                state.result = Some(screens::create::CreateResultMessage {
                    success: false,
                    message: format!("Failed to resolve worktree root: {e:#}"),
                });
                return;
            }
        };

        let template = &state.worktree_template;
        match crate::cli::commands::create::execute(
            &branch,
            base.as_deref(),
            &cwd,
            &worktree_root,
            template,
            &db,
        ) {
            Ok(result) => {
                let msg = format!("Created '{}' at {}", result.name, result.path.display());
                if let Some(ref mut s) = self.create_state {
                    s.result = Some(screens::create::CreateResultMessage {
                        success: true,
                        message: msg,
                    });
                }
            }
            Err(e) => {
                if let Some(ref mut s) = self.create_state {
                    s.result = Some(screens::create::CreateResultMessage {
                        success: false,
                        message: format!("Create failed: {e:#}"),
                    });
                }
            }
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
    fn screen_enum_has_six_variants() {
        // Verify all six screen variants exist and are distinct
        let screens = [Screen::List, Screen::Detail, Screen::Create, Screen::Help, Screen::SyncPicker, Screen::DeleteConfirm];
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
    fn question_mark_toggles_help_closed_when_already_on_help() {
        let mut app = App::new();
        // Open help
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 2);

        // Press ? again — should close help (toggle)
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "? while on Help should pop back to previous screen"
        );
        assert_eq!(app.nav_stack_depth(), 1);
    }

    #[test]
    fn question_mark_toggles_help_from_detail_screen() {
        let mut app = app_with_rows();
        // Navigate to detail
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Detail);

        // Open help from detail
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 3);

        // Close help — should return to detail
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Detail,
            "? toggle should return to Detail, not List"
        );
        assert_eq!(app.nav_stack_depth(), 2);
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
                path: "/tmp/wt/feat-a".into(),
                status: "clean".into(),
                ahead_behind: "+0/-0".into(),
                managed: true,
            },
            WorktreeRow {
                name: "feat-b".into(),
                branch: "feat/b".into(),
                path: "/tmp/wt/feat-b".into(),
                status: "~2".into(),
                ahead_behind: "+1/-0".into(),
                managed: true,
            },
            WorktreeRow {
                name: "main".into(),
                branch: "main".into(),
                path: "/tmp/wt/main".into(),
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
    fn s_on_list_pushes_sync_picker() {
        let mut app = app_with_rows();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(app.is_running());
        assert_eq!(app.active_screen(), Screen::SyncPicker);
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn s_on_list_sets_sync_picker_state_with_selected_worktree() {
        let mut app = app_with_rows();
        // Select second row
        app.list_state.selected = 1;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        let state = app.sync_picker_state.as_ref().expect("sync_picker_state should be set");
        assert_eq!(state.worktree_name, "feat-b");
    }

    #[test]
    fn d_on_list_pushes_delete_confirm() {
        let mut app = app_with_rows();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        assert!(app.is_running());
        assert_eq!(app.active_screen(), Screen::DeleteConfirm);
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn d_on_list_sets_delete_confirm_state_with_selected_worktree() {
        let mut app = app_with_rows();
        app.list_state.selected = 1; // select feat-b
        app.handle_key_event(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        let state = app.delete_confirm_state.as_ref().expect("delete_confirm_state should be set");
        assert_eq!(state.worktree_name, "feat-b");
        assert_eq!(state.worktree_path, "/tmp/wt/feat-b");
        assert_eq!(state.branch, "feat/b");
    }

    #[test]
    fn d_on_empty_list_does_not_push_delete_confirm() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.delete_confirm_state.is_none());
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
    fn create_screen_renders_placeholder() {
        let mut app = App::new();
        app.push_screen(Screen::Create);
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            content.contains("trench TUI"),
            "Create screen should show placeholder, got: {:?}",
            content.trim()
        );
    }

    fn app_with_create_state() -> App {
        let mut app = App::new();
        app.create_state = Some(screens::create::CreateState::new(
            vec!["main".into(), "develop".into()],
            "my-project".into(),
            "{{ repo }}/{{ branch | sanitize }}".into(),
        ));
        app.push_screen(Screen::Create);
        app
    }

    #[test]
    fn create_screen_renders_form_when_state_present() {
        let app = app_with_create_state();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            content.contains("Create Worktree"),
            "Create screen with state should show form title, got: {:?}",
            content.trim()
        );
    }

    #[test]
    fn esc_on_create_pops_to_list_and_clears_state() {
        let mut app = app_with_create_state();
        assert_eq!(app.active_screen(), Screen::Create);
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.create_state.is_none(), "create_state should be cleared on Esc");
    }

    #[test]
    fn q_on_create_pops_when_not_in_branch_field() {
        let mut app = app_with_create_state();
        // Move focus to Base field so q acts as cancel
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Base
        );
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.create_state.is_none());
    }

    #[test]
    fn typing_on_create_updates_branch_input() {
        let mut app = app_with_create_state();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let state = app.create_state.as_ref().unwrap();
        assert_eq!(state.branch_input, "foo");
    }

    #[test]
    fn typing_on_create_updates_path_preview() {
        let mut app = app_with_create_state();
        for c in "feature/auth".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let state = app.create_state.as_ref().unwrap();
        assert_eq!(state.path_preview, "my-project/feature-auth");
    }

    #[test]
    fn tab_on_create_moves_to_next_field() {
        let mut app = app_with_create_state();
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Branch
        );
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Base
        );
    }

    #[test]
    fn backtab_on_create_moves_to_previous_field() {
        let mut app = app_with_create_state();
        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Hooks
        );
    }

    #[test]
    fn left_right_on_base_field_cycles_base_branches() {
        let mut app = app_with_create_state();
        // Move to Base field
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Base
        );
        assert_eq!(app.create_state.as_ref().unwrap().selected_base, 0);
        app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.create_state.as_ref().unwrap().selected_base, 1);
        app.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.create_state.as_ref().unwrap().selected_base, 0);
    }

    #[test]
    fn space_on_hooks_field_toggles_hooks() {
        let mut app = app_with_create_state();
        // Move to Hooks field
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Hooks
        );
        assert!(app.create_state.as_ref().unwrap().hooks_enabled);
        app.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(!app.create_state.as_ref().unwrap().hooks_enabled);
    }

    #[test]
    fn backspace_on_create_removes_char() {
        let mut app = app_with_create_state();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.create_state.as_ref().unwrap().branch_input, "a");
    }

    #[test]
    fn app_has_create_state_initially_none() {
        let app = App::new();
        assert!(app.create_state.is_none());
    }

    #[test]
    fn n_on_list_sets_create_state() {
        let mut app = App::new();
        // Without a real git repo, init_create_form will use fallback defaults
        app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Create);
        assert!(app.create_state.is_some(), "create_state should be initialized");
    }

    #[test]
    fn enter_on_hooks_field_with_empty_branch_shows_validation_error() {
        let mut app = app_with_create_state();
        // Move to Hooks field
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Hooks
        );
        // Try to create with empty branch
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        // Should still be on Create screen with error
        assert_eq!(app.active_screen(), Screen::Create);
        assert!(
            app.create_state.as_ref().unwrap().error.is_some(),
            "should show validation error for empty branch"
        );
        assert!(
            app.create_state.as_ref().unwrap().result.is_none(),
            "should not have result yet"
        );
    }

    #[test]
    fn enter_on_hooks_field_with_branch_triggers_execute() {
        let mut app = app_with_create_state();
        // Type a branch name
        for c in "test-branch".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        // Move to Hooks field
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        // Try to create — will fail without real git repo but should set result
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Create);
        let state = app.create_state.as_ref().unwrap();
        assert!(state.is_result_mode(), "should be in result mode after execute attempt");
    }

    #[test]
    fn enter_in_create_result_mode_pops_to_list() {
        let mut app = app_with_create_state();
        // Set result directly
        app.create_state.as_mut().unwrap().result = Some(screens::create::CreateResultMessage {
            success: true,
            message: "Created 'test'".into(),
        });
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List, "Enter in result mode should pop to list");
        assert!(app.create_state.is_none(), "create_state should be cleared");
    }

    #[test]
    fn space_in_create_result_mode_pops_to_list() {
        let mut app = app_with_create_state();
        app.create_state.as_mut().unwrap().result = Some(screens::create::CreateResultMessage {
            success: false,
            message: "Create failed".into(),
        });
        app.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.create_state.is_none());
    }

    #[test]
    fn create_result_mode_renders_result_message() {
        let mut app = app_with_create_state();
        app.create_state.as_mut().unwrap().result = Some(screens::create::CreateResultMessage {
            success: true,
            message: "Created 'feat-x' at /tmp/wt/feat-x".into(),
        });
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(content.contains("Created"), "should show result message");
        assert!(content.contains("dismiss"), "should show dismiss footer");
    }

    #[test]
    fn help_screen_renders_help_overlay_not_placeholder() {
        let mut app = App::new();
        app.push_screen(Screen::Help);
        let backend = ratatui::backend::TestBackend::new(80, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            content.contains("Help"),
            "Help screen should render help overlay with title"
        );
        assert!(
            content.contains("Global"),
            "Help overlay should show Global group header"
        );
        assert!(
            content.contains("List"),
            "Help overlay should show List group header"
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
    fn s_on_empty_list_does_not_push_sync_picker() {
        let mut app = App::new();
        // Empty list — no rows
        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List, "s on empty list should stay on List");
        assert!(app.sync_picker_state.is_none(), "sync_picker_state should remain None");
    }

    #[test]
    fn s_on_detail_pushes_sync_picker() {
        let mut app = App::new();
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::SyncPicker);
        let picker = app.sync_picker_state.as_ref().expect("sync_picker_state should be set");
        assert_eq!(picker.worktree_name, "feat-a");
    }

    #[test]
    fn o_on_detail_sets_editor_request() {
        let mut app = App::new();
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert!(app.is_running(), "o on detail should not crash or quit");
        assert_eq!(app.active_screen(), Screen::Detail);
        assert_eq!(
            app.editor_request,
            Some("/tmp/wt/feat-a".to_string()),
            "o should set editor_request to worktree path"
        );
    }

    #[test]
    fn arrow_down_on_sync_picker_selects_merge() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);
        assert_eq!(app.sync_picker_state.as_ref().unwrap().selected, 0);

        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.sync_picker_state.as_ref().unwrap().selected, 1, "down should select Merge");
    }

    #[test]
    fn arrow_up_on_sync_picker_selects_rebase() {
        let mut app = App::new();
        let mut state = screens::sync_picker::SyncPickerState::new("feat-auth");
        state.selected = 1;
        app.sync_picker_state = Some(state);
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.sync_picker_state.as_ref().unwrap().selected, 0, "up should select Rebase");
    }

    #[test]
    fn j_k_keys_work_on_sync_picker() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.sync_picker_state.as_ref().unwrap().selected, 1, "j should move down");

        app.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.sync_picker_state.as_ref().unwrap().selected, 0, "k should move up");
    }

    #[test]
    fn enter_on_sync_picker_triggers_sync_and_sets_result() {
        // Without a real git repo, execute_sync will fail — but it should
        // set a result message (failure) and stay on the SyncPicker screen.
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        // Should still be on SyncPicker (showing result)
        assert_eq!(app.active_screen(), Screen::SyncPicker);
        let picker = app.sync_picker_state.as_ref().unwrap();
        assert!(picker.is_result_mode(), "should be in result mode after Enter");
        assert!(picker.result.is_some());
    }

    #[test]
    fn enter_in_result_mode_pops_back_to_list() {
        let mut app = App::new();
        let mut state = screens::sync_picker::SyncPickerState::new("feat-auth");
        state.result = Some(screens::sync_picker::SyncResultMessage {
            success: true,
            message: "Synced successfully".into(),
        });
        app.sync_picker_state = Some(state);
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List, "Enter in result mode should pop to list");
        assert!(app.sync_picker_state.is_none(), "sync_picker_state should be cleared");
    }

    #[test]
    fn enter_in_result_mode_pops_to_list_from_detail_path() {
        let mut app = app_with_rows();
        // Simulate Detail → SyncPicker flow: nav stack = [List, Detail, SyncPicker]
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);
        let mut state = screens::sync_picker::SyncPickerState::new("feat-a");
        state.result = Some(screens::sync_picker::SyncResultMessage {
            success: true,
            message: "Synced successfully".into(),
        });
        app.sync_picker_state = Some(state);
        app.push_screen(Screen::SyncPicker);
        assert_eq!(app.nav_stack_depth(), 3);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List, "should pop all the way to List, not Detail");
        assert!(app.sync_picker_state.is_none(), "sync_picker_state should be cleared");
    }

    #[test]
    fn sync_picker_screen_renders_options_through_app() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);

        let backend = ratatui::backend::TestBackend::new(80, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(content.contains("Rebase"), "should render Rebase option");
        assert!(content.contains("Merge"), "should render Merge option");
        assert!(content.contains("feat-auth"), "should show worktree name");
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
    fn app_has_delete_confirm_state_initially_none() {
        let app = App::new();
        assert!(app.delete_confirm_state.is_none());
    }

    #[test]
    fn push_delete_confirm_screen_works() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth", "/tmp/wt/feat-auth", "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);
        assert_eq!(app.active_screen(), Screen::DeleteConfirm);
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn enter_on_delete_confirm_triggers_delete_and_sets_result() {
        // Without a real git repo, execute_delete will fail — but it should
        // set a result message (failure) and stay on the DeleteConfirm screen.
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth", "/tmp/wt/feat-auth", "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.active_screen(), Screen::DeleteConfirm);
        let state = app.delete_confirm_state.as_ref().unwrap();
        assert!(state.is_result_mode(), "should be in result mode after Enter");
        assert!(state.result.is_some());
    }

    #[test]
    fn y_on_delete_confirm_triggers_delete_and_sets_result() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth", "/tmp/wt/feat-auth", "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert_eq!(app.active_screen(), Screen::DeleteConfirm);
        let state = app.delete_confirm_state.as_ref().unwrap();
        assert!(state.is_result_mode(), "y should also trigger delete");
    }

    #[test]
    fn n_on_delete_confirm_cancels_dialog() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth", "/tmp/wt/feat-auth", "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

        assert_eq!(app.active_screen(), Screen::List, "n should pop back to list");
        assert!(app.delete_confirm_state.is_none(), "state should be cleared on cancel");
    }

    #[test]
    fn enter_in_delete_result_mode_pops_to_list() {
        let mut app = App::new();
        let mut state = screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth", "/tmp/wt/feat-auth", "feature/auth",
        );
        state.result = Some(screens::delete_confirm::DeleteResultMessage {
            success: true,
            message: "Removed successfully".into(),
        });
        app.delete_confirm_state = Some(state);
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List, "Enter in result mode should pop to list");
        assert!(app.delete_confirm_state.is_none(), "state should be cleared");
    }

    #[test]
    fn space_in_delete_result_mode_pops_to_list() {
        let mut app = App::new();
        let mut state = screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth", "/tmp/wt/feat-auth", "feature/auth",
        );
        state.result = Some(screens::delete_confirm::DeleteResultMessage {
            success: true,
            message: "Done".into(),
        });
        app.delete_confirm_state = Some(state);
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List, "Space in result mode should pop to list");
        assert!(app.delete_confirm_state.is_none());
    }

    #[test]
    fn esc_on_delete_confirm_clears_state() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth", "/tmp/wt/feat-auth", "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List, "Esc should pop back to List");
        assert!(app.delete_confirm_state.is_none(), "Esc should clear delete_confirm_state");
    }

    #[test]
    fn q_on_delete_confirm_clears_state() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth", "/tmp/wt/feat-auth", "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.delete_confirm_state.is_none(), "q should clear delete_confirm_state");
    }

    #[test]
    fn esc_on_delete_result_mode_clears_state() {
        let mut app = App::new();
        let mut state = screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth", "/tmp/wt/feat-auth", "feature/auth",
        );
        state.result = Some(screens::delete_confirm::DeleteResultMessage {
            success: true,
            message: "Done".into(),
        });
        app.delete_confirm_state = Some(state);
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List, "Esc in result mode should pop to List");
        assert!(app.delete_confirm_state.is_none(), "Esc in result mode should clear state");
    }

    #[test]
    fn esc_on_sync_picker_clears_state() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.sync_picker_state.is_none(), "Esc should clear sync_picker_state");
    }

    #[test]
    fn push_sync_picker_screen_works() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);
        assert_eq!(app.active_screen(), Screen::SyncPicker);
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn help_over_sync_picker_renders_sync_picker_underneath() {
        let mut app = app_with_rows();
        // Push SyncPicker, then Help
        app.sync_picker_state =
            Some(screens::sync_picker::SyncPickerState::new("feat-a"));
        app.push_screen(Screen::SyncPicker);
        app.push_screen(Screen::Help);
        assert_eq!(app.active_screen(), Screen::Help);

        let backend = ratatui::backend::TestBackend::new(80, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        // Help overlay should be present
        assert!(content.contains("Help"), "should render Help overlay");
        // SyncPicker should be underneath (not list)
        assert!(
            content.contains("Sync strategy for"),
            "should render SyncPicker underneath Help overlay, got: {:?}",
            content.trim()
        );
    }

    #[test]
    fn q_inserts_into_branch_field_on_create_screen() {
        let mut app = app_with_create_state();
        assert_eq!(app.active_screen(), Screen::Create);
        // Branch field is focused by default
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Branch
        );

        // Press 'q' — should insert into branch_input, NOT pop screen
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

        assert_eq!(
            app.active_screen(),
            Screen::Create,
            "q should NOT pop the Create screen when Branch field is focused"
        );
        assert_eq!(
            app.create_state.as_ref().unwrap().branch_input,
            "q",
            "q should be inserted into branch_input"
        );
    }

    #[test]
    fn backspace_to_empty_revalidates_and_shows_error() {
        let mut app = app_with_create_state();
        // Type a char then backspace — should revalidate and show error for empty
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(
            app.create_state.as_ref().unwrap().error.is_none(),
            "valid input should have no error"
        );
        app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(
            app.create_state.as_ref().unwrap().error.is_some(),
            "empty input after backspace should revalidate and show error"
        );
    }
}
