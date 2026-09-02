use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

#[path = "../build/version.rs"]
mod build_version;

#[test]
fn version_flag_reports_the_embedded_git_derived_version() {
    let expected = build_version::derive(Path::new(env!("CARGO_MANIFEST_DIR")), false)
        .expect("workspace build identity");

    let output = Command::new(env!("CARGO_BIN_EXE_trench"))
        .arg("--version")
        .output()
        .expect("run trench --version");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 version output"),
        format!("trench {}\n", expected.version)
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn version_probe_does_not_initialize_diagnostics() {
    let root = TempDir::new().unwrap();
    let state_home = root.path().join("state");

    let output = Command::new(env!("CARGO_BIN_EXE_trench"))
        .arg("--version")
        .env("XDG_STATE_HOME", &state_home)
        .output()
        .expect("run trench --version");

    assert!(output.status.success());
    assert!(
        !state_home.exists(),
        "version probe created diagnostic state at {}",
        state_home.display()
    );
}
