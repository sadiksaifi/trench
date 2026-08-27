pub mod app;
pub mod cockpit;
pub mod create_flow;
pub mod keymap;
pub mod operation_modal;
pub mod operation_runtime;
pub mod ref_picker;
pub mod refresh;
pub mod refresh_runtime;
pub mod remove_flow;
pub mod runtime;
pub mod search;
pub mod sync_flow;
pub mod theme;
pub mod watcher;

pub use runtime::run;

use std::sync::{Arc, Mutex};

type PanicHook = dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync;

static PREV_PANIC_HOOK: Mutex<Option<Arc<PanicHook>>> = Mutex::new(None);

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    PREV_PANIC_HOOK
        .lock()
        .expect("panic hook lock poisoned")
        .replace(Arc::from(previous));
    std::panic::set_hook(Box::new(|info| {
        ratatui::restore();
        if let Some(previous) = PREV_PANIC_HOOK
            .lock()
            .expect("panic hook lock poisoned")
            .as_ref()
        {
            previous(info);
        }
    }));
}

fn restore_panic_hook() {
    if let Some(previous) = PREV_PANIC_HOOK
        .lock()
        .expect("panic hook lock poisoned")
        .take()
    {
        std::panic::set_hook(Box::new(move |info| previous(info)));
    }
}
