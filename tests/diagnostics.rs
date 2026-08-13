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

#[test]
fn startup_rotates_one_mibibyte_logs_and_caps_retention_at_five_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let state_home = dir.path().join("state");
    let log_dir = state_home.join("trench");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("trench.log"), vec![b'x'; 1024 * 1024]).unwrap();
    for index in 1..=5 {
        std::fs::write(
            log_dir.join(format!("trench.log.{index}")),
            format!("old-{index}"),
        )
        .unwrap();
    }

    let output = version_with_state_home(&state_home);

    assert!(output.status.success());
    let mut names = std::fs::read_dir(&log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        [
            "trench.log",
            "trench.log.1",
            "trench.log.2",
            "trench.log.3",
            "trench.log.4",
        ]
    );
    assert_eq!(
        std::fs::metadata(log_dir.join("trench.log.1"))
            .unwrap()
            .len(),
        1024 * 1024
    );
}
