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

/// Context needed to build TRENCH_* environment variables for hook processes (FR-23).
pub struct HookEnvContext {
    pub worktree_path: String,
    pub worktree_name: String,
    pub branch: String,
    pub repo_name: String,
    pub repo_path: String,
    pub base_branch: String,
}

/// Retrieve the HookConfig for a specific lifecycle event from HooksConfig.
pub fn get_hook_config<'a>(hooks: &'a HooksConfig, event: &HookEvent) -> Option<&'a HookConfig> {
    match event {
        HookEvent::PreCreate => hooks.pre_create.as_ref(),
        HookEvent::PostCreate => hooks.post_create.as_ref(),
        HookEvent::PreSync => hooks.pre_sync.as_ref(),
        HookEvent::PostSync => hooks.post_sync.as_ref(),
        HookEvent::PreRemove => hooks.pre_remove.as_ref(),
        HookEvent::PostRemove => hooks.post_remove.as_ref(),
    }
}

/// Build the 7 TRENCH_* environment variables injected into hook processes (FR-23).
pub fn build_env(ctx: &HookEnvContext, event: &HookEvent) -> HashMap<String, String> {
    HashMap::from([
        ("TRENCH_WORKTREE_PATH".into(), ctx.worktree_path.clone()),
        ("TRENCH_WORKTREE_NAME".into(), ctx.worktree_name.clone()),
        ("TRENCH_BRANCH".into(), ctx.branch.clone()),
        ("TRENCH_REPO_NAME".into(), ctx.repo_name.clone()),
        ("TRENCH_REPO_PATH".into(), ctx.repo_path.clone()),
        ("TRENCH_BASE_BRANCH".into(), ctx.base_branch.clone()),
        ("TRENCH_EVENT".into(), event.as_str().to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_env_returns_all_seven_trench_vars() {
        let ctx = HookEnvContext {
            worktree_path: "/home/user/.worktrees/myrepo/feat-auth".into(),
            worktree_name: "feat-auth".into(),
            branch: "feature/auth".into(),
            repo_name: "myrepo".into(),
            repo_path: "/home/user/code/myrepo".into(),
            base_branch: "main".into(),
        };

        let env = build_env(&ctx, &HookEvent::PostCreate);

        assert_eq!(env.len(), 7);
        assert_eq!(env["TRENCH_WORKTREE_PATH"], "/home/user/.worktrees/myrepo/feat-auth");
        assert_eq!(env["TRENCH_WORKTREE_NAME"], "feat-auth");
        assert_eq!(env["TRENCH_BRANCH"], "feature/auth");
        assert_eq!(env["TRENCH_REPO_NAME"], "myrepo");
        assert_eq!(env["TRENCH_REPO_PATH"], "/home/user/code/myrepo");
        assert_eq!(env["TRENCH_BASE_BRANCH"], "main");
        assert_eq!(env["TRENCH_EVENT"], "post_create");
    }

    #[test]
    fn build_env_event_string_matches_hook_event() {
        let ctx = HookEnvContext {
            worktree_path: "/tmp/wt".into(),
            worktree_name: "wt".into(),
            branch: "fix/bug".into(),
            repo_name: "repo".into(),
            repo_path: "/tmp/repo".into(),
            base_branch: "develop".into(),
        };

        for (event, expected) in [
            (HookEvent::PreCreate, "pre_create"),
            (HookEvent::PreSync, "pre_sync"),
            (HookEvent::PostSync, "post_sync"),
            (HookEvent::PreRemove, "pre_remove"),
            (HookEvent::PostRemove, "post_remove"),
        ] {
            let env = build_env(&ctx, &event);
            assert_eq!(env["TRENCH_EVENT"], expected);
        }
    }

    #[test]
    fn get_hook_config_returns_matching_hook() {
        let hooks = HooksConfig {
            post_create: Some(HookDef {
                copy: Some(vec![".env*".into()]),
                run: Some(vec!["bun install".into()]),
                shell: None,
                timeout_secs: Some(300),
            }),
            ..Default::default()
        };

        let config = get_hook_config(&hooks, &HookEvent::PostCreate);
        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.copy, Some(vec![".env*".to_string()]));
        assert_eq!(config.run, Some(vec!["bun install".to_string()]));
        assert_eq!(config.timeout_secs, Some(300));
    }

    #[test]
    fn get_hook_config_returns_none_for_unconfigured_hook() {
        let hooks = HooksConfig::default();

        for event in [
            HookEvent::PreCreate,
            HookEvent::PostCreate,
            HookEvent::PreSync,
            HookEvent::PostSync,
            HookEvent::PreRemove,
            HookEvent::PostRemove,
        ] {
            assert!(get_hook_config(&hooks, &event).is_none());
        }
    }

    #[test]
    fn hooks_deserialize_from_toml_and_resolve_by_event() {
        let toml_str = r#"
[hooks.post_create]
copy = [".env*", "!.env.example"]
run = ["bun install", "bunx prisma generate"]
timeout_secs = 300

[hooks.pre_remove]
shell = "pkill -f 'next dev' || true"
timeout_secs = 60
"#;
        let config: crate::config::ProjectConfig = toml::from_str(toml_str).unwrap();
        let hooks = config.hooks.unwrap();

        // post_create is configured with copy + run
        let post_create = get_hook_config(&hooks, &HookEvent::PostCreate).unwrap();
        assert_eq!(
            post_create.copy,
            Some(vec![".env*".to_string(), "!.env.example".to_string()])
        );
        assert_eq!(
            post_create.run,
            Some(vec!["bun install".to_string(), "bunx prisma generate".to_string()])
        );
        assert!(post_create.shell.is_none());
        assert_eq!(post_create.timeout_secs, Some(300));

        // pre_remove uses shell instead of run
        let pre_remove = get_hook_config(&hooks, &HookEvent::PreRemove).unwrap();
        assert!(pre_remove.copy.is_none());
        assert!(pre_remove.run.is_none());
        assert_eq!(pre_remove.shell, Some("pkill -f 'next dev' || true".to_string()));
        assert_eq!(pre_remove.timeout_secs, Some(60));

        // unconfigured hooks return None
        assert!(get_hook_config(&hooks, &HookEvent::PreCreate).is_none());
        assert!(get_hook_config(&hooks, &HookEvent::PreSync).is_none());
        assert!(get_hook_config(&hooks, &HookEvent::PostSync).is_none());
        assert!(get_hook_config(&hooks, &HookEvent::PostRemove).is_none());
    }

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
