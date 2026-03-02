//! Generate shell completions for the trench CLI.

use clap::CommandFactory;
use clap_complete::{generate as gen_completions, Shell};
use std::io;

/// Write shell completions to stdout for the given shell type.
pub fn generate<C: CommandFactory>(shell: &str) {
    let shell = match shell {
        "bash" => Shell::Bash,
        "zsh" => Shell::Zsh,
        "fish" => Shell::Fish,
        _ => unreachable!("unsupported shell: {shell}"),
    };
    let mut cmd = C::command();
    gen_completions(shell, &mut cmd, "trench", &mut io::stdout());
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal clap struct for testing completions generation
    #[derive(clap::Parser, Debug)]
    #[command(name = "trench")]
    struct TestCli {
        #[command(subcommand)]
        command: Option<TestCommands>,
    }

    #[derive(clap::Subcommand, Debug)]
    enum TestCommands {
        Create { branch: String },
        Switch { branch: String },
        List,
    }

    #[test]
    fn bash_completions_are_generated() {
        let shell = Shell::Bash;
        let mut cmd = TestCli::command();
        let mut buf = Vec::new();
        gen_completions(shell, &mut cmd, "trench", &mut buf);
        let output = String::from_utf8(buf).expect("completions should be valid utf-8");
        assert!(
            !output.is_empty(),
            "bash completions should produce output"
        );
        assert!(
            output.contains("trench"),
            "bash completions should reference the command name"
        );
    }

    #[test]
    fn zsh_completions_are_generated() {
        let shell = Shell::Zsh;
        let mut cmd = TestCli::command();
        let mut buf = Vec::new();
        gen_completions(shell, &mut cmd, "trench", &mut buf);
        let output = String::from_utf8(buf).expect("completions should be valid utf-8");
        assert!(!output.is_empty(), "zsh completions should produce output");
        assert!(
            output.contains("trench"),
            "zsh completions should reference the command name"
        );
    }

    #[test]
    fn fish_completions_are_generated() {
        let shell = Shell::Fish;
        let mut cmd = TestCli::command();
        let mut buf = Vec::new();
        gen_completions(shell, &mut cmd, "trench", &mut buf);
        let output = String::from_utf8(buf).expect("completions should be valid utf-8");
        assert!(
            !output.is_empty(),
            "fish completions should produce output"
        );
        assert!(
            output.contains("trench"),
            "fish completions should reference the command name"
        );
    }
}
