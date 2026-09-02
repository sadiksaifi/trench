use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

fn trench_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trench"))
}

fn startup_with_state_home(state_home: &Path) -> Output {
    Command::new(trench_bin())
        .args(["completions", "bash"])
        .env("XDG_STATE_HOME", state_home)
        .output()
        .expect("failed to run trench completions bash")
}

fn spawn_startup_with_state_home(state_home: &Path) -> Child {
    Command::new(trench_bin())
        .args(["completions", "bash"])
        .env("XDG_STATE_HOME", state_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn trench completions bash")
}

fn diagnostic_log_names(log_dir: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| {
            name == "trench.log"
                || name
                    .strip_prefix("trench.log.")
                    .is_some_and(|suffix| suffix.parse::<usize>().is_ok())
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn linux_and_macos_style_xdg_state_homes_hold_diagnostics() {
    let dir = tempfile::TempDir::new().unwrap();
    let state_homes = [
        dir.path().join("linux/home/alice/.local/state"),
        dir.path().join("macos/Users/alice/.local/state"),
    ];

    for state_home in state_homes {
        let output = startup_with_state_home(&state_home);

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

    let baseline = startup_with_state_home(&usable_state_home);
    let unavailable = startup_with_state_home(&blocked_state_home);

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

    let output = startup_with_state_home(&state_home);

    assert!(output.status.success());
    let names = diagnostic_log_names(&log_dir);
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

#[test]
fn concurrent_process_startup_keeps_diagnostic_retention_bounded() {
    let dir = tempfile::TempDir::new().unwrap();
    let state_home = dir.path().join("state");
    let log_dir = state_home.join("trench");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join("trench.log"), vec![b'x'; 1024 * 1024]).unwrap();
    for index in 1..=9 {
        std::fs::write(
            log_dir.join(format!("trench.log.{index}")),
            format!("old-{index}"),
        )
        .unwrap();
    }

    let process_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(log_dir.join(".trench.log.lock"))
        .unwrap();
    process_lock.lock().unwrap();
    let mut children = (0..12)
        .map(|_| spawn_startup_with_state_home(&state_home))
        .collect::<Vec<_>>();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let finished_while_locked = loop {
        let mut all_finished = true;
        for child in &mut children {
            match child.try_wait().unwrap() {
                Some(status) => assert!(status.success()),
                None => all_finished = false,
            }
        }
        if all_finished || std::time::Instant::now() >= deadline {
            break all_finished;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };

    process_lock.unlock().unwrap();
    for mut child in children {
        if child.try_wait().unwrap().is_none() {
            assert!(child.wait().unwrap().success());
        }
    }
    assert!(
        finished_while_locked,
        "diagnostic lock contention delayed a product process"
    );

    let recovery = startup_with_state_home(&state_home);
    assert!(recovery.status.success());
    assert_eq!(
        diagnostic_log_names(&log_dir),
        [
            "trench.log",
            "trench.log.1",
            "trench.log.2",
            "trench.log.3",
            "trench.log.4",
        ]
    );
}
