/// Generate shell function definitions for `tr()` shell integration.
///
/// The `tr()` function wraps `trench switch --print-path` with `cd` so
/// switching worktrees changes the shell's working directory. All other
/// subcommands pass through to `trench` unmodified.

/// Generate the shell function definition for the given shell type.
pub fn generate(shell: &str) -> String {
    match shell {
        "bash" | "zsh" => generate_posix(),
        "fish" => generate_fish(),
        _ => unreachable!("unsupported shell: {shell}"),
    }
}

fn generate_posix() -> String {
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
    .to_string()
}

fn generate_fish() -> String {
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
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_output_defines_tr_function() {
        let output = generate("bash");
        assert!(
            output.contains("tr()"),
            "bash output should define tr() function"
        );
    }

    #[test]
    fn bash_output_contains_trench_switch_with_print_path() {
        let output = generate("bash");
        assert!(
            output.contains("trench switch --print-path"),
            "bash output should call trench switch --print-path"
        );
    }

    #[test]
    fn bash_output_contains_cd() {
        let output = generate("bash");
        assert!(
            output.contains("cd "),
            "bash output should cd into the worktree path"
        );
    }

    #[test]
    fn bash_output_passes_through_non_switch_commands() {
        let output = generate("bash");
        assert!(
            output.contains("command trench"),
            "bash output should pass non-switch commands through to trench"
        );
    }

    #[test]
    fn zsh_output_defines_tr_function() {
        let output = generate("zsh");
        assert!(
            output.contains("tr()"),
            "zsh output should define tr() function"
        );
    }

    #[test]
    fn zsh_output_contains_trench_switch_with_print_path() {
        let output = generate("zsh");
        assert!(
            output.contains("trench switch --print-path"),
            "zsh output should call trench switch --print-path"
        );
    }

    #[test]
    fn zsh_and_bash_produce_same_output() {
        let bash = generate("bash");
        let zsh = generate("zsh");
        assert_eq!(bash, zsh, "bash and zsh should use the same POSIX function");
    }

    #[test]
    fn fish_output_defines_tr_function() {
        let output = generate("fish");
        assert!(
            output.contains("function tr"),
            "fish output should define function tr"
        );
    }

    #[test]
    fn fish_output_contains_trench_switch_with_print_path() {
        let output = generate("fish");
        assert!(
            output.contains("trench switch --print-path"),
            "fish output should call trench switch --print-path"
        );
    }

    #[test]
    fn fish_output_contains_cd() {
        let output = generate("fish");
        assert!(
            output.contains("cd "),
            "fish output should cd into the worktree path"
        );
    }

    #[test]
    fn fish_output_passes_through_non_switch_commands() {
        let output = generate("fish");
        assert!(
            output.contains("command trench"),
            "fish output should pass non-switch commands through to trench"
        );
    }

    #[test]
    fn fish_output_differs_from_bash() {
        let fish = generate("fish");
        let bash = generate("bash");
        assert_ne!(fish, bash, "fish syntax differs from bash/zsh");
    }

    #[test]
    fn bash_output_is_valid_shell_syntax() {
        let output = generate("bash");
        let status = std::process::Command::new("bash")
            .arg("-n")
            .arg("-c")
            .arg(&output)
            .output()
            .expect("failed to run bash");
        assert!(
            status.status.success(),
            "bash syntax check failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    #[test]
    fn zsh_output_is_valid_shell_syntax() {
        let output = generate("zsh");
        let status = std::process::Command::new("zsh")
            .arg("-n")
            .arg("-c")
            .arg(&output)
            .output()
            .expect("failed to run zsh");
        assert!(
            status.status.success(),
            "zsh syntax check failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    #[test]
    fn fish_output_is_valid_shell_syntax() {
        // fish --no-execute parses without executing
        let result = std::process::Command::new("fish")
            .arg("--no-execute")
            .arg("-c")
            .arg(&generate("fish"))
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
