use std::process::{Command, Output};

fn trench(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trench"))
        .args(args)
        .output()
        .expect("trench should run")
}

#[test]
fn upgrade_is_a_first_class_command() {
    let help = trench(&["upgrade", "--help"]);
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    assert!(String::from_utf8_lossy(&help.stdout).contains("Upgrade trench"));
}

#[test]
fn development_binary_is_refused_before_installation_or_network_detection() {
    let output = trench(&["upgrade"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot upgrade a development build"),
        "{stderr}"
    );
}
