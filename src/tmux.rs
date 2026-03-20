/// Tmux integration for worktree switching.
///
/// When enabled, `trench switch` opens a new tmux window in the worktree
/// directory instead of just printing the path.

/// Check whether the current process is running inside a tmux session.
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok_and(|v| !v.is_empty())
}

/// Build the argument list for `tmux new-window` targeting the given
/// worktree path with the window named after the branch.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_inside_tmux_returns_false_when_unset() {
        // Remove TMUX env var for this test
        std::env::remove_var("TMUX");
        assert!(!is_inside_tmux());
    }

    #[test]
    fn is_inside_tmux_returns_false_when_empty() {
        std::env::set_var("TMUX", "");
        let result = is_inside_tmux();
        std::env::remove_var("TMUX");
        assert!(!result);
    }

    #[test]
    fn is_inside_tmux_returns_true_when_set() {
        std::env::set_var("TMUX", "/tmp/tmux-501/default,12345,0");
        let result = is_inside_tmux();
        std::env::remove_var("TMUX");
        assert!(result);
    }

    #[test]
    fn build_new_window_command_constructs_correct_args() {
        let cmd = build_new_window_command("/home/user/.worktrees/repo/feature-auth", "feature-auth");
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
}
