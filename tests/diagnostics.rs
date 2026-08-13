use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn trench_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trench"))
}

fn version_with_state_home(state_home: &Path) -> Output {
    Command::new(trench_bin())
        .arg("--version")
        .env("XDG_STATE_HOME", state_home)
        .output()
        .expect("failed to run trench --version")
}

#[test]
fn linux_and_macos_style_xdg_state_homes_hold_diagnostics() {
    let dir = tempfile::TempDir::new().unwrap();
    let state_homes = [
        dir.path().join("linux/home/alice/.local/state"),
        dir.path().join("macos/Users/alice/.local/state"),
    ];

    for state_home in state_homes {
        let output = version_with_state_home(&state_home);

        assert!(output.status.success());
        let log_path = state_home.join("trench/trench.log");
        assert!(log_path.is_file(), "missing {}", log_path.display());
        assert!(!log_path.starts_with("/var/log"));
        assert!(!state_home.join("trench/trench.db").exists());
    }
}

#[test]
fn unavailable_state_path_does_not_change_command_results_or_exit_status() {
    let dir = tempfile::TempDir::new().unwrap();
    let usable_state_home = dir.path().join("usable-state");
    let blocked_state_home = dir.path().join("not-a-directory");
    std::fs::write(&blocked_state_home, "blocks directory creation").unwrap();

    let baseline = version_with_state_home(&usable_state_home);
    let unavailable = version_with_state_home(&blocked_state_home);

    assert_eq!(unavailable.status.code(), baseline.status.code());
    assert_eq!(unavailable.stdout, baseline.stdout);
    assert_eq!(unavailable.stderr, baseline.stderr);
}
