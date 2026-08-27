use std::process::{Command, Output};

fn trench(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trench"))
        .args(args)
        .output()
        .expect("trench should run")
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
