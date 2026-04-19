//! Generate shell completions for the trench CLI.

use crate::ShellType;
use clap::CommandFactory;
use clap_complete::{generate as gen_completions, Shell};
use std::io;

/// Write shell completions for the given shell type.
pub fn generate<C: CommandFactory>(shell: ShellType, buf: &mut dyn io::Write) {
    let clap_shell = match shell {
        ShellType::Bash => Shell::Bash,
        ShellType::Zsh => Shell::Zsh,
        ShellType::Fish => Shell::Fish,
    };
    let mut cmd = C::command();
    gen_completions(clap_shell, &mut cmd, "trench", buf);
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
        let mut buf = Vec::new();
        generate::<TestCli>(ShellType::Bash, &mut buf);
        let output = String::from_utf8(buf).expect("completions should be valid utf-8");
        assert!(!output.is_empty(), "bash completions should produce output");
        assert!(
            output.contains("trench"),
            "bash completions should reference the command name"
        );
    }

    #[test]
    fn zsh_completions_are_generated() {
        let mut buf = Vec::new();
        generate::<TestCli>(ShellType::Zsh, &mut buf);
        let output = String::from_utf8(buf).expect("completions should be valid utf-8");
        assert!(!output.is_empty(), "zsh completions should produce output");
        assert!(
            output.contains("trench"),
            "zsh completions should reference the command name"
        );
    }

    #[test]
    fn fish_completions_are_generated() {
        let mut buf = Vec::new();
        generate::<TestCli>(ShellType::Fish, &mut buf);
        let output = String::from_utf8(buf).expect("completions should be valid utf-8");
        assert!(!output.is_empty(), "fish completions should produce output");
        assert!(
            output.contains("trench"),
            "fish completions should reference the command name"
        );
    }
}
