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
}
