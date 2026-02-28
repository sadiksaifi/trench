use std::collections::HashMap;
use std::fmt;

use crate::config::{HookDef, HooksConfig};

/// Re-export HookDef as HookConfig for the hooks module public API.
/// This is the per-hook configuration with copy, run, shell, timeout_secs fields.
pub type HookConfig = HookDef;

/// Six lifecycle hooks fired during worktree operations (FR-18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    PreCreate,
    PostCreate,
    PreSync,
    PostSync,
    PreRemove,
    PostRemove,
}

impl HookEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreCreate => "pre_create",
            Self::PostCreate => "post_create",
            Self::PreSync => "pre_sync",
            Self::PostSync => "post_sync",
            Self::PreRemove => "pre_remove",
            Self::PostRemove => "post_remove",
        }
    }
}

impl fmt::Display for HookEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_event_has_six_variants_with_correct_strings() {
        let cases = vec![
            (HookEvent::PreCreate, "pre_create"),
            (HookEvent::PostCreate, "post_create"),
            (HookEvent::PreSync, "pre_sync"),
            (HookEvent::PostSync, "post_sync"),
            (HookEvent::PreRemove, "pre_remove"),
            (HookEvent::PostRemove, "post_remove"),
        ];

        for (event, expected) in cases {
            assert_eq!(event.as_str(), expected);
            assert_eq!(format!("{event}"), expected);
        }
    }
}
