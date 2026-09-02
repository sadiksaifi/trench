#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

const TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn fake_release_binary(root: &Path, otool_output: &str) -> (std::path::PathBuf, String) {
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let binary = root.join("trench");
    write_executable(&binary, "#!/bin/sh\nprintf 'trench 1.2.3\\n'\n");
    write_executable(
        &bin_dir.join("otool"),
        &format!("#!/bin/sh\nprintf '%s:\\n\\t{otool_output}\\n' \"$2\"\n"),
    );
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    (binary, path)
}

#[test]
fn packages_a_versionless_archive_with_the_portable_release_layout() {
    for target in TARGETS {
        let root = TempDir::new().unwrap();
        let output_dir = root.path().join("dist");
        let (binary, path) = fake_release_binary(
            root.path(),
            "/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1.0.0)",
        );
        let output =
            Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/package-release.sh"))
                .args([
                    target,
                    binary.to_str().unwrap(),
                    output_dir.to_str().unwrap(),
                ])
                .env("PATH", path)
                .env("TRENCH_RELEASE_VERSION", "1.2.3")
                .output()
                .expect("package script should execute");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let archive = output_dir.join(format!("trench-{target}.tar.gz"));
        assert!(archive.is_file());
        let listing = Command::new("tar")
            .args(["-tzf", archive.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(listing.status.success());
        assert_eq!(
            String::from_utf8(listing.stdout).unwrap(),
            "trench\nLICENSE\nREADME.md\n"
        );

        let extracted = root.path().join("extracted");
        fs::create_dir(&extracted).unwrap();
        assert!(Command::new("tar")
            .args([
                "-xzf",
                archive.to_str().unwrap(),
                "-C",
                extracted.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success());
        assert_eq!(
            fs::metadata(extracted.join("trench"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
}

#[test]
fn refuses_to_package_a_binary_linked_to_build_machine_or_non_system_paths() {
    for linked_path in [
        "/opt/homebrew/opt/openssl@3/lib/libssl.3.dylib",
        "/usr/local/opt/openssl@3/lib/libssl.3.dylib",
        "/Users/runner/work/trench/target/release/deps/libgit2.dylib",
        "/Users/runner/work/_temp/libssl.dylib",
        "/private/tmp/build/libunexpected.dylib",
    ] {
        let root = TempDir::new().unwrap();
        let output_dir = root.path().join("dist");
        let (binary, path) = fake_release_binary(
            root.path(),
            &format!("{linked_path} (compatibility version 3.0.0, current version 3.0.0)"),
        );

        let output =
            Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/package-release.sh"))
                .args([
                    "aarch64-apple-darwin",
                    binary.to_str().unwrap(),
                    output_dir.to_str().unwrap(),
                ])
                .env("PATH", path)
                .env("TRENCH_RELEASE_VERSION", "1.2.3")
                .env("GITHUB_WORKSPACE", "/Users/runner/work/trench")
                .env("RUNNER_TEMP", "/Users/runner/work/_temp")
                .output()
                .unwrap();
        assert!(!output.status.success(), "accepted {linked_path}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("forbidden build-machine path")
                || stderr.contains("non-system dynamic library"),
            "unexpected rejection for {linked_path}: {}",
            stderr
        );
        assert!(!output_dir
            .join("trench-aarch64-apple-darwin.tar.gz")
            .exists());
    }
}

#[test]
fn assembles_a_consistent_schema_one_manifest_and_checksum_set() {
    let root = TempDir::new().unwrap();
    let dist = root.path().join("dist");
    fs::create_dir(&dist).unwrap();
    fs::write(dist.join("trench-aarch64-apple-darwin.tar.gz"), "arm").unwrap();
    fs::write(dist.join("trench-x86_64-apple-darwin.tar.gz"), "intel").unwrap();
    let installer = root.path().join("installer.sh");
    write_executable(&installer, "#!/bin/sh\nexit 0\n");
    let commit = "0123456789abcdef0123456789abcdef01234567";

    let output =
        Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/assemble-release.sh"))
            .args([
                dist.to_str().unwrap(),
                "v1.2.3",
                "1.2.3",
                commit,
                installer.to_str().unwrap(),
            ])
            .output()
            .expect("assembly script should execute");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(dist.join("trench-release.json")).unwrap()).unwrap();
    assert_eq!(manifest["schema"], 1);
    assert_eq!(manifest["tag"], "v1.2.3");
    assert_eq!(manifest["version"], "1.2.3");
    assert_eq!(manifest["commit"], commit);
    let assets = manifest["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 3);
    let names = assets
        .iter()
        .map(|asset| asset["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "trench-aarch64-apple-darwin.tar.gz",
            "trench-x86_64-apple-darwin.tar.gz",
            "trench-installer.sh",
        ]
    );
    assert!(assets.iter().all(|asset| {
        asset["sha256"].as_str().is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }));

    for asset in assets {
        let name = asset["name"].as_str().unwrap();
        let expected = asset["sha256"].as_str().unwrap();
        assert_eq!(
            sha256(&dist.join(name)),
            expected,
            "manifest digest for {name}"
        );
    }

    let checksums = fs::read_to_string(dist.join("trench-checksums.txt")).unwrap();
    for name in [
        "trench-aarch64-apple-darwin.tar.gz",
        "trench-x86_64-apple-darwin.tar.gz",
        "trench-installer.sh",
        "trench-release.json",
    ] {
        assert_eq!(checksums.matches(name).count(), 1, "checksum for {name}");
    }
    assert!(!checksums.contains("trench-checksums.txt"));
    assert_eq!(checksums.lines().count(), 4);
    for line in checksums.lines() {
        let (expected, name) = line.split_once("  ").unwrap();
        assert_eq!(
            sha256(&dist.join(name)),
            expected,
            "checksum digest for {name}"
        );
    }
}

fn sha256(path: &Path) -> String {
    let output = Command::new("shasum")
        .args(["-a", "256", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}
