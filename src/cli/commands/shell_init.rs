//! Generate shell function definitions for `tn()` shell integration.
//!
//! The `tn()` function wraps `trench switch --print-path` with `cd` so
//! switching worktrees changes the shell's working directory. All other
//! subcommands pass through to `trench` unmodified.

use crate::ShellType;

/// Generate the shell function definition for the given shell type.
pub fn generate(shell: ShellType) -> &'static str {
    match shell {
        ShellType::Bash | ShellType::Zsh => generate_posix(),
        ShellType::Fish => generate_fish(),
    }
}

fn generate_posix() -> &'static str {
    r#"tn() {
    if [ "$1" = "switch" ]; then
        shift
        local dir
        dir="$(command trench switch --print-path "$@")"
        local exit_code=$?
        if [ "$exit_code" -ne 0 ]; then
            return "$exit_code"
        fi
        if [ -z "$dir" ]; then
            echo "trench: switch returned empty path" >&2
            return 1
        fi
        cd -- "$dir" || return 1
    else
        command trench "$@"
    fi
}
"#
}

fn generate_fish() -> &'static str {
    r#"function tn
    if test (count $argv) -gt 0 -a "$argv[1]" = "switch"
        set -l rest $argv[2..-1]
        set -l dir (command trench switch --print-path $rest)
        set -l exit_code $status
        if test $exit_code -ne 0
            return $exit_code
        end
        if test -z "$dir"
            echo "trench: switch returned empty path" >&2
            return 1
        end
        cd -- "$dir"
    else
        command trench $argv
    end
end
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_output_defines_tn_function() {
        let output = generate(ShellType::Bash);
        assert!(
            output.contains("tn()"),
            "bash output should define tn() function"
        );
        assert!(
            !output.contains("tr()"),
            "bash output should not define old tr() function"
        );
    }

    #[test]
    fn bash_output_contains_trench_switch_with_print_path() {
        let output = generate(ShellType::Bash);
        assert!(
            output.contains("trench switch --print-path"),
            "bash output should call trench switch --print-path"
        );
    }

    #[test]
    fn bash_output_contains_cd() {
        let output = generate(ShellType::Bash);
        assert!(
            output.contains("cd "),
            "bash output should cd into the worktree path"
        );
    }

    #[test]
    fn bash_output_passes_through_non_switch_commands() {
        let output = generate(ShellType::Bash);
        assert!(
            output.contains("command trench"),
            "bash output should pass non-switch commands through to trench"
        );
    }

    #[test]
    fn zsh_output_defines_tn_function() {
        let output = generate(ShellType::Zsh);
        assert!(
            output.contains("tn()"),
            "zsh output should define tn() function"
        );
        assert!(
            !output.contains("tr()"),
            "zsh output should not define old tr() function"
        );
    }

    #[test]
    fn zsh_output_contains_trench_switch_with_print_path() {
        let output = generate(ShellType::Zsh);
        assert!(
            output.contains("trench switch --print-path"),
            "zsh output should call trench switch --print-path"
        );
    }

    #[test]
    fn zsh_and_bash_produce_same_output() {
        let bash = generate(ShellType::Bash);
        let zsh = generate(ShellType::Zsh);
        assert_eq!(bash, zsh, "bash and zsh should use the same POSIX function");
    }

    #[test]
    fn fish_output_defines_tn_function() {
        let output = generate(ShellType::Fish);
        assert!(
            output.contains("function tn"),
            "fish output should define function tn"
        );
        assert!(
            !output.contains("\nfunction tr\n"),
            "fish output should not define old function tr"
        );
    }

    #[test]
    fn fish_output_contains_trench_switch_with_print_path() {
        let output = generate(ShellType::Fish);
        assert!(
            output.contains("trench switch --print-path"),
            "fish output should call trench switch --print-path"
        );
    }

    #[test]
    fn fish_output_contains_cd() {
        let output = generate(ShellType::Fish);
        assert!(
            output.contains("cd "),
            "fish output should cd into the worktree path"
        );
    }

    #[test]
    fn fish_output_passes_through_non_switch_commands() {
        let output = generate(ShellType::Fish);
        assert!(
            output.contains("command trench"),
            "fish output should pass non-switch commands through to trench"
        );
    }

    #[test]
    fn fish_output_differs_from_bash() {
        let fish = generate(ShellType::Fish);
        let bash = generate(ShellType::Bash);
        assert_ne!(fish, bash, "fish syntax differs from bash/zsh");
    }

    #[test]
    fn posix_output_reports_error_on_empty_path() {
        let output = generate(ShellType::Bash);
        assert!(
            output.contains("switch returned empty path\" >&2\n            return 1"),
            "posix output should report error and return non-zero when switch returns empty path"
        );
    }

    #[test]
    fn fish_output_reports_error_on_empty_path() {
        let output = generate(ShellType::Fish);
        assert!(
            output.contains("switch returned empty path\" >&2\n            return 1"),
            "fish output should report error and return non-zero when switch returns empty path"
        );
    }

    fn assert_valid_shell_syntax(shell: &str, args: &[&str], script: &str) {
        let result = std::process::Command::new(shell)
            .args(args)
            .arg(script)
            .output();
        match result {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "{shell} syntax check failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("{shell} not found, skipping syntax check");
            }
            Err(e) => panic!("failed to run {shell}: {e}"),
        }
    }

    #[test]
    fn bash_output_is_valid_shell_syntax() {
        assert_valid_shell_syntax("bash", &["-n", "-c"], generate(ShellType::Bash));
    }

    #[test]
    fn zsh_output_is_valid_shell_syntax() {
        assert_valid_shell_syntax("zsh", &["-n", "-c"], generate(ShellType::Zsh));
    }

    #[test]
    fn fish_output_is_valid_shell_syntax() {
        assert_valid_shell_syntax("fish", &["--no-execute", "-c"], generate(ShellType::Fish));
    }
}
