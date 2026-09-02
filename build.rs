#[path = "build/version.rs"]
mod version;

use std::path::{Path, PathBuf};
use std::process::Command;

const RELEASE_BUILD_ENV: &str = "TRENCH_RELEASE_BUILD";

fn main() -> Result<(), String> {
    let root = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| "CARGO_MANIFEST_DIR is unavailable".to_owned())?,
    );
    emit_rerun_triggers(&root);

    let official = match std::env::var(RELEASE_BUILD_ENV) {
        Ok(value) if value == "true" => true,
        Ok(value) if value == "false" => false,
        Ok(value) => {
            return Err(format!(
                "{RELEASE_BUILD_ENV} must be `true` or `false`, got `{value}`"
            ));
        }
        Err(std::env::VarError::NotPresent) => false,
        Err(error) => return Err(format!("could not read {RELEASE_BUILD_ENV}: {error}")),
    };
    let info = version::derive(&root, official)?;

    println!("cargo:rustc-env=TRENCH_BUILD_VERSION={}", info.version);
    println!("cargo:rustc-env=TRENCH_BUILD_OFFICIAL={}", info.official);
    if let Some(commit) = info.commit {
        println!("cargo:rustc-env=TRENCH_BUILD_COMMIT={commit}");
    }
    if let Some(tag) = info.exact_tag {
        println!("cargo:rustc-env=TRENCH_BUILD_EXACT_TAG={tag}");
    }
    if let Some(dirty) = info.dirty {
        println!("cargo:rustc-env=TRENCH_BUILD_DIRTY={dirty}");
    }

    Ok(())
}

fn emit_rerun_triggers(root: &Path) {
    println!("cargo:rerun-if-env-changed={RELEASE_BUILD_ENV}");

    let dot_git = root.join(".git");
    if !dot_git.exists() {
        // Cargo must notice when a source archive becomes a Git checkout.
        // Watching the missing path reruns metadata derivation until then.
        println!("cargo:rerun-if-changed={}", dot_git.display());
    }

    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name == ".git" || name == "target" {
                continue;
            }
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }

    for git_path in ["HEAD", "index", "packed-refs", "refs/heads", "refs/tags"] {
        let output = Command::new("git")
            .args(["rev-parse", "--git-path", git_path])
            .current_dir(root)
            .output();
        let Ok(output) = output else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if path.is_empty() {
            continue;
        }
        let path = Path::new(&path);
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            root.join(path)
        };
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
