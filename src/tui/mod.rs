pub mod screens;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{layout::Alignment, widgets::Paragraph, Frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    List,
    Detail,
    Create,
    Help,
}

type PanicHook = dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync;

/// Stores the pre-TUI panic hook so `restore_panic_hook` can put it back.
static PREV_PANIC_HOOK: Mutex<Option<Arc<PanicHook>>> = Mutex::new(None);

/// Launch the TUI. This is the single public entry point.
pub fn run() -> Result<()> {
    install_panic_hook();
    let mut terminal = ratatui::init();
    let mut app = App::new();

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
    PREV_PANIC_HOOK.lock().unwrap().replace(Arc::clone(&original));
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
}

impl App {
    pub fn new() -> Self {
        Self {
            running: true,
            nav_stack: vec![Screen::List],
        }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn active_screen(&self) -> Screen {
        *self.nav_stack.last().expect("nav stack must never be empty")
    }

    pub fn ui(&self, frame: &mut Frame) {
        let placeholder = Paragraph::new("trench TUI — press q to quit")
            .alignment(Alignment::Center);
        frame.render_widget(placeholder, frame.area());
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => self.running = false,
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.running = false,
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
    fn screen_enum_has_four_variants() {
        // Verify all four screen variants exist and are distinct
        let screens = [Screen::List, Screen::Detail, Screen::Create, Screen::Help];
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
    fn app_exits_on_q_key() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.is_running(), "app should stop after pressing 'q'");
    }

    #[test]
    fn app_exits_on_ctrl_c() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.is_running(), "app should stop after Ctrl+C");
    }

    #[test]
    fn app_ignores_other_keys() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.is_running(), "non-quit keys should not stop the app");
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
    fn placeholder_ui_renders_trench_tui() {
        let app = App::new();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| app.ui(frame))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            content.contains("trench TUI"),
            "placeholder screen should contain 'trench TUI', got: {:?}",
            content.trim()
        );
    }
}
