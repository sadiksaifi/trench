use std::process::{Command, Output};

fn trench(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trench"))
        .args(args)
        .output()
        .expect("trench should run")
}

fn help(command: &str) -> String {
    let output = trench(&[command, "--help"]);
    assert!(output.status.success(), "help failed for {command}");
    assert!(output.stderr.is_empty(), "help wrote stderr for {command}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn help_exposes_exactly_the_supported_commands() {
    let output = trench(&["--help"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let commands = stdout
        .split_once("Commands:\n")
        .expect("help should contain a commands section")
        .1
        .split_once("\n\n")
        .map_or_else(|| "", |(section, _)| section)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect::<Vec<_>>();

    assert_eq!(
        commands,
        [
            "create",
            "remove",
            "switch",
            "open",
            "list",
            "sync",
            "init",
            "shell-init",
            "completions",
        ]
    );
}

#[test]
fn structured_and_preview_flags_are_scoped_to_supported_commands() {
    let top = String::from_utf8(trench(&["--help"]).stdout).unwrap();
    for removed in [
        "--json",
        "--porcelain",
        "--dry-run",
        "--quiet",
        "--verbose",
        "--no-color",
    ] {
        assert!(!top.contains(removed), "top-level help exposed {removed}");
    }

    for command in ["create", "remove", "sync"] {
        let command_help = help(command);
        assert!(command_help.contains("--json"), "{command_help}");
        assert!(command_help.contains("--dry-run"), "{command_help}");
        assert!(!command_help.contains("--porcelain"), "{command_help}");
    }

    let list = help("list");
    assert!(list.contains("--json"), "{list}");
    assert!(list.contains("--porcelain"), "{list}");
    assert!(!list.contains("--dry-run"), "{list}");

    for command in ["switch", "open", "init", "shell-init", "completions"] {
        let command_help = help(command);
        for unsupported in ["--json", "--porcelain", "--dry-run"] {
            assert!(
                !command_help.contains(unsupported),
                "{command} exposed {unsupported}: {command_help}"
            );
        }
    }

    for args in [
        &["--json", "list"][..],
        &["--quiet", "list"][..],
        &["--verbose", "list"][..],
        &["--no-color", "list"][..],
        &["switch", "main", "--json"][..],
        &["open", "main", "--dry-run"][..],
    ] {
        let output = trench(args);
        assert_eq!(output.status.code(), Some(2), "accepted {args:?}");
        assert!(
            output.stdout.is_empty(),
            "invalid args wrote stdout: {args:?}"
        );
    }
}
