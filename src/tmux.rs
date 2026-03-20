/// Tmux integration for worktree switching.
///
/// When enabled, `trench switch` opens a new tmux window in the worktree
/// directory instead of just printing the path.

/// Check whether the current process is running inside a tmux session.
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok_and(|v| !v.is_empty())
}

/// Build the argument list for `tmux new-window` targeting the given
/// worktree path with the window named after the worktree.
///
/// Returns the full argv including "tmux".
pub fn build_new_window_command(worktree_path: &str, window_name: &str) -> Vec<String> {
    vec![
        "tmux".to_string(),
        "new-window".to_string(),
        "-n".to_string(),
        window_name.to_string(),
        "-c".to_string(),
        worktree_path.to_string(),
    ]
}

/// The action that `run_switch` should take after resolving the worktree.
#[derive(Debug, PartialEq)]
pub enum SwitchAction {
    /// Open a new tmux window. Contains the argv for `tmux new-window`.
    TmuxNewWindow(Vec<String>),
    /// Print path or message (normal behavior). Includes a warning if
    /// the user explicitly asked for tmux but we're not inside a session.
    PrintPath { warn_not_in_tmux: bool },
}

/// Decide what action to take for a switch operation.
///
/// `tmux_flag`: whether `--tmux` was passed on the CLI.
/// `config_tmux`: whether `[shell] tmux = true` is set in config.
/// `inside_tmux`: whether we're currently inside a tmux session.
pub fn resolve_switch_action(
    tmux_flag: bool,
    config_tmux: bool,
    inside_tmux: bool,
    worktree_path: &str,
    window_name: &str,
) -> SwitchAction {
    let use_tmux = tmux_flag || config_tmux;

    if use_tmux && inside_tmux {
        SwitchAction::TmuxNewWindow(build_new_window_command(worktree_path, window_name))
    } else if tmux_flag && !inside_tmux {
        // User explicitly asked for tmux but we're not in a session
        SwitchAction::PrintPath {
            warn_not_in_tmux: true,
        }
    } else {
        // Not using tmux (either not enabled, or config-only and not in tmux)
        SwitchAction::PrintPath {
            warn_not_in_tmux: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;

    /// RAII guard that saves the current value of an env var and restores it on drop.
    struct EnvGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let prev = std::env::var_os(key);
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    #[serial]
    fn is_inside_tmux_returns_false_when_unset() {
        let _guard = EnvGuard::set("TMUX", None);
        assert!(!is_inside_tmux());
    }

    #[test]
    #[serial]
    fn is_inside_tmux_returns_false_when_empty() {
        let _guard = EnvGuard::set("TMUX", Some(""));
        assert!(!is_inside_tmux());
    }

    #[test]
    #[serial]
    fn is_inside_tmux_returns_true_when_set() {
        let _guard = EnvGuard::set("TMUX", Some("/tmp/tmux-501/default,12345,0"));
        assert!(is_inside_tmux());
    }

    #[test]
    fn build_new_window_command_constructs_correct_args() {
        let cmd =
            build_new_window_command("/home/user/.worktrees/repo/feature-auth", "feature-auth");
        assert_eq!(
            cmd,
            vec![
                "tmux",
                "new-window",
                "-n",
                "feature-auth",
                "-c",
                "/home/user/.worktrees/repo/feature-auth",
            ]
        );
    }

    #[test]
    fn build_new_window_command_handles_branch_with_slashes() {
        let cmd = build_new_window_command("/wt/feat-login", "feat-login");
        assert_eq!(cmd[3], "feat-login");
        assert_eq!(cmd[5], "/wt/feat-login");
    }

    // --- resolve_switch_action tests ---

    #[test]
    fn action_tmux_flag_inside_tmux_opens_window() {
        let action = resolve_switch_action(true, false, true, "/wt/feat", "feat");
        assert_eq!(
            action,
            SwitchAction::TmuxNewWindow(vec![
                "tmux".into(),
                "new-window".into(),
                "-n".into(),
                "feat".into(),
                "-c".into(),
                "/wt/feat".into(),
            ])
        );
    }

    #[test]
    fn action_config_tmux_inside_tmux_opens_window() {
        let action = resolve_switch_action(false, true, true, "/wt/feat", "feat");
        assert_eq!(
            action,
            SwitchAction::TmuxNewWindow(vec![
                "tmux".into(),
                "new-window".into(),
                "-n".into(),
                "feat".into(),
                "-c".into(),
                "/wt/feat".into(),
            ])
        );
    }

    #[test]
    fn action_tmux_flag_not_in_tmux_warns_and_falls_back() {
        let action = resolve_switch_action(true, false, false, "/wt/feat", "feat");
        assert_eq!(
            action,
            SwitchAction::PrintPath {
                warn_not_in_tmux: true
            }
        );
    }

    #[test]
    fn action_config_tmux_not_in_tmux_silent_fallback() {
        let action = resolve_switch_action(false, true, false, "/wt/feat", "feat");
        assert_eq!(
            action,
            SwitchAction::PrintPath {
                warn_not_in_tmux: false
            }
        );
    }

    #[test]
    fn action_no_tmux_at_all_prints_path() {
        let action = resolve_switch_action(false, false, false, "/wt/feat", "feat");
        assert_eq!(
            action,
            SwitchAction::PrintPath {
                warn_not_in_tmux: false
            }
        );
    }

    #[test]
    fn action_no_tmux_even_inside_tmux_prints_path() {
        let action = resolve_switch_action(false, false, true, "/wt/feat", "feat");
        assert_eq!(
            action,
            SwitchAction::PrintPath {
                warn_not_in_tmux: false
            }
        );
    }

    #[test]
    fn action_both_flag_and_config_inside_tmux_opens_window() {
        let action = resolve_switch_action(true, true, true, "/wt/feat", "feat");
        assert_eq!(
            action,
            SwitchAction::TmuxNewWindow(vec![
                "tmux".into(),
                "new-window".into(),
                "-n".into(),
                "feat".into(),
                "-c".into(),
                "/wt/feat".into(),
            ])
        );
    }
}
