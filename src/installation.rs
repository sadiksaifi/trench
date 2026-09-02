use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::paths;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallationManager {
    Standalone,
    Homebrew,
    ManualOrUnknown,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallReceipt {
    schema: u8,
    manager: String,
    executable: PathBuf,
}

pub fn detect(
    running_executable: &Path,
    receipt_path: &Path,
    brew_prefix: impl FnOnce() -> Option<PathBuf>,
) -> std::io::Result<InstallationManager> {
    let running_executable = running_executable.canonicalize()?;

    if standalone_receipt_matches(&running_executable, receipt_path) {
        return Ok(InstallationManager::Standalone);
    }

    if brew_prefix().is_some_and(|prefix| {
        prefix
            .join("bin/trench")
            .canonicalize()
            .is_ok_and(|executable| executable == running_executable)
    }) {
        return Ok(InstallationManager::Homebrew);
    }

    Ok(InstallationManager::ManualOrUnknown)
}

fn standalone_receipt_matches(running_executable: &Path, receipt_path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(receipt_path) else {
        return false;
    };
    let Ok(receipt) = serde_json::from_str::<InstallReceipt>(&contents) else {
        return false;
    };
    receipt.schema == 1
        && receipt.manager == "standalone"
        && receipt.executable.is_absolute()
        && receipt
            .executable
            .canonicalize()
            .is_ok_and(|executable| executable == running_executable)
}

pub fn detect_current() -> anyhow::Result<InstallationManager> {
    let executable = std::env::current_exe()?;
    let receipt = paths::install_receipt_path()?;
    detect(&executable, &receipt, homebrew_prefix).map_err(Into::into)
}

fn homebrew_prefix() -> Option<PathBuf> {
    let output = std::process::Command::new("brew")
        .args(["--prefix", "trench"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let prefix = String::from_utf8(output.stdout).ok()?;
    let prefix = prefix.trim();
    (!prefix.is_empty()).then(|| PathBuf::from(prefix))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn matching_receipt_identifies_a_standalone_installation() {
        let root = TempDir::new().unwrap();
        let executable = root.path().join("bin/trench");
        let receipt = root.path().join("data/trench/install-receipt.json");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        fs::write(&executable, "binary").unwrap();
        fs::write(
            &receipt,
            format!(
                r#"{{"schema":1,"manager":"standalone","executable":"{}"}}"#,
                executable.display()
            ),
        )
        .unwrap();

        let ownership = detect(&executable, &receipt, || None).unwrap();

        assert_eq!(ownership, InstallationManager::Standalone);
    }

    #[test]
    fn stale_or_malformed_receipts_never_authorize_replacement() {
        let root = TempDir::new().unwrap();
        let executable = root.path().join("actual/trench");
        let stale = root.path().join("old/trench");
        let receipt = root.path().join("install-receipt.json");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&executable, "actual").unwrap();
        fs::write(&stale, "old").unwrap();

        assert_eq!(
            detect(&executable, &receipt, || None).unwrap(),
            InstallationManager::ManualOrUnknown
        );

        fs::write(
            &receipt,
            format!(
                r#"{{"schema":1,"manager":"standalone","executable":"{}"}}"#,
                stale.display()
            ),
        )
        .unwrap();

        assert_eq!(
            detect(&executable, &receipt, || None).unwrap(),
            InstallationManager::ManualOrUnknown
        );

        fs::write(&receipt, "not json").unwrap();
        assert_eq!(
            detect(&executable, &receipt, || None).unwrap(),
            InstallationManager::ManualOrUnknown
        );
    }

    #[cfg(unix)]
    #[test]
    fn receipt_and_homebrew_detection_compare_canonical_executables() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let cellar = root.path().join("Cellar/trench/1.0.0");
        let executable = cellar.join("bin/trench");
        let opt_prefix = root.path().join("opt/trench");
        let receipt = root.path().join("install-receipt.json");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::create_dir_all(opt_prefix.parent().unwrap()).unwrap();
        fs::write(&executable, "brew binary").unwrap();
        symlink(&cellar, &opt_prefix).unwrap();

        assert_eq!(
            detect(&executable, &receipt, || Some(opt_prefix)).unwrap(),
            InstallationManager::Homebrew
        );

        let standalone_link = root.path().join("standalone-trench");
        symlink(&executable, &standalone_link).unwrap();
        fs::write(
            &receipt,
            format!(
                r#"{{"schema":1,"manager":"standalone","executable":"{}"}}"#,
                standalone_link.display()
            ),
        )
        .unwrap();
        assert_eq!(
            detect(&executable, &receipt, || None).unwrap(),
            InstallationManager::Standalone
        );
    }

    #[test]
    fn receipt_rejects_extra_or_noncanonical_schema_fields() {
        let root = TempDir::new().unwrap();
        let executable = root.path().join("trench");
        let receipt = root.path().join("install-receipt.json");
        fs::write(&executable, "binary").unwrap();
        fs::write(
            &receipt,
            format!(
                r#"{{"schema":1,"manager":"standalone","executable":"{}","version":"1.0.0"}}"#,
                executable.display()
            ),
        )
        .unwrap();

        assert_eq!(
            detect(&executable, &receipt, || None).unwrap(),
            InstallationManager::ManualOrUnknown
        );
    }
}
