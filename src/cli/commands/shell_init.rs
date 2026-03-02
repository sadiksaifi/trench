//! Generate shell function definitions for `tr()` shell integration.
//!
//! The `tr()` function wraps `trench switch --print-path` with `cd` so
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
    r#"tr() {
    if [ "$1" = "switch" ]; then
        shift
        local dir
        dir="$(command trench switch --print-path "$@")"
        local exit_code=$?
        if [ "$exit_code" -eq 0 ] && [ -n "$dir" ]; then
            cd "$dir" || return 1
        else
            return "$exit_code"
        fi
    else
        command trench "$@"
    fi
}
"#
}

fn generate_fish() -> &'static str {
    r#"function tr
    if test (count $argv) -gt 0 -a "$argv[1]" = "switch"
        set -l rest $argv[2..-1]
        set -l dir (command trench switch --print-path $rest)
        set -l exit_code $status
        if test $exit_code -eq 0 -a -n "$dir"
            cd $dir
        else
            return $exit_code
        end
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
    fn bash_output_defines_tr_function() {
        let output = generate(ShellType::Bash);
        assert!(
            output.contains("tr()"),
            "bash output should define tr() function"
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
    fn zsh_output_defines_tr_function() {
        let output = generate(ShellType::Zsh);
        assert!(
            output.contains("tr()"),
            "zsh output should define tr() function"
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
    fn fish_output_defines_tr_function() {
        let output = generate(ShellType::Fish);
        assert!(
            output.contains("function tr"),
            "fish output should define function tr"
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
    fn bash_output_is_valid_shell_syntax() {
        let result = std::process::Command::new("bash")
            .arg("-n")
            .arg("-c")
            .arg(&generate(ShellType::Bash))
            .output();
        match result {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "bash syntax check failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("bash not found, skipping syntax check");
            }
            Err(e) => panic!("failed to run bash: {e}"),
        }
    }

    #[test]
    fn zsh_output_is_valid_shell_syntax() {
        let result = std::process::Command::new("zsh")
            .arg("-n")
            .arg("-c")
            .arg(&generate(ShellType::Zsh))
            .output();
        match result {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "zsh syntax check failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("zsh not found, skipping syntax check");
            }
            Err(e) => panic!("failed to run zsh: {e}"),
        }
    }

    #[test]
    fn fish_output_is_valid_shell_syntax() {
        // fish --no-execute parses without executing
        let result = std::process::Command::new("fish")
            .arg("--no-execute")
            .arg("-c")
            .arg(&generate(ShellType::Fish))
            .output();
        match result {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "fish syntax check failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // fish not installed — skip gracefully
                eprintln!("fish not found, skipping syntax check");
            }
            Err(e) => panic!("failed to run fish: {e}"),
        }
    }
}
