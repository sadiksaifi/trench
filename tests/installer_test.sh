#!/usr/bin/env bash

set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
installer="$repo_root/scripts/trench-installer.sh"
tests_run=0

fail() {
  printf 'not ok %d - %s\n' "$tests_run" "$1" >&2
  exit 1
}

assert_contains() {
  haystack=$1
  needle=$2
  case "$haystack" in
    *"$needle"*) ;;
    *) fail "expected output to contain: $needle" ;;
  esac
}

write_mock_commands() {
  mock_bin=$1
  mkdir -p "$mock_bin"

  cat >"$mock_bin/uname" <<'EOF'
#!/bin/sh
case "${1-}" in
    -s) printf '%s\n' "${MOCK_UNAME_S:-Darwin}" ;;
    -m) printf '%s\n' "${MOCK_UNAME_M:-arm64}" ;;
    *) printf '%s\n' "${MOCK_UNAME_S:-Darwin}" ;;
esac
EOF
  chmod +x "$mock_bin/uname"

  cat >"$mock_bin/curl" <<'EOF'
#!/bin/sh
output=
url=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) output=$2; shift 2 ;;
        *) url=$1; shift ;;
    esac
done
asset=${url##*/}
if [ "${MOCK_CURL_FAIL_ASSET-}" = "$asset" ]; then
    printf '%s\n' "partial" >"$output"
    exit 22
fi
cp "$MOCK_RELEASE_DIR/$asset" "$output"
EOF
  chmod +x "$mock_bin/curl"

  cat >"$mock_bin/xattr" <<'EOF'
#!/bin/sh
exit "${MOCK_XATTR_STATUS:-1}"
EOF
  chmod +x "$mock_bin/xattr"
}

make_release() {
  release_dir=$1
  asset=$2
  target=$3
  staging=$release_dir/staging
  mkdir -p "$staging"
  cat >"$staging/trench" <<EOF
#!/bin/sh
case "\${1-}" in
    --version) printf '%s\n' 'trench 0.1.0' ;;
    shell-init) printf '%s\n' '# shell init for \${2-}' ;;
esac
EOF
  chmod +x "$staging/trench"
  cp "$repo_root/LICENSE" "$staging/LICENSE"
  cp "$repo_root/README.md" "$staging/README.md"
  tar -C "$staging" -czf "$release_dir/$asset" trench LICENSE README.md
  archive_sha=$(shasum -a 256 "$release_dir/$asset" | awk '{print $1}')
  cat >"$release_dir/trench-release.json" <<EOF
{"schema":1,"tag":"v0.1.0","version":"0.1.0","commit":"0123456789abcdef0123456789abcdef01234567","assets":[{"name":"$asset","sha256":"$archive_sha","target":"$target"}]}
EOF
  manifest_sha=$(shasum -a 256 "$release_dir/trench-release.json" | awk '{print $1}')
  printf '%s  %s\n%s  %s\n' \
    "$archive_sha" "$asset" \
    "$manifest_sha" trench-release.json \
    >"$release_dir/trench-checksums.txt"
}

run_installer() {
  sandbox=$1
  shift
  if [ "${TEST_ZDOTDIR+x}" = x ]; then
    zdotdir_env=("ZDOTDIR=$TEST_ZDOTDIR")
  else
    zdotdir_env=(-u ZDOTDIR)
  fi
  env "${zdotdir_env[@]}" \
    HOME="$sandbox/home" \
    XDG_DATA_HOME="${TEST_XDG_DATA_HOME:-$sandbox/data}" \
    SHELL=/bin/zsh \
    PATH="${TEST_PATH:-$sandbox/mock-bin:/usr/bin:/bin}" \
    MOCK_RELEASE_DIR="$sandbox/release" \
    MOCK_UNAME_S="${MOCK_UNAME_S:-Darwin}" \
    MOCK_UNAME_M="${MOCK_UNAME_M:-arm64}" \
    MOCK_CURL_FAIL_ASSET="${MOCK_CURL_FAIL_ASSET-}" \
    MOCK_XATTR_STATUS="${MOCK_XATTR_STATUS:-1}" \
    TMPDIR="${TEST_TMPDIR:-${TMPDIR:-/tmp}}" \
    TRENCH_INSTALL_DIR="$sandbox/install/bin" \
    sh "$installer" --no-modify-shell "$@"
}

run_installer_with_shell() {
  sandbox=$1
  shell_path=$2
  shift 2
  if [ "${TEST_ZDOTDIR+x}" = x ]; then
    zdotdir_env=("ZDOTDIR=$TEST_ZDOTDIR")
  else
    zdotdir_env=(-u ZDOTDIR)
  fi
  env "${zdotdir_env[@]}" \
    HOME="$sandbox/home" \
    XDG_DATA_HOME="${TEST_XDG_DATA_HOME:-$sandbox/data}" \
    SHELL="$shell_path" \
    PATH="${TEST_PATH:-$sandbox/mock-bin:/usr/bin:/bin}" \
    MOCK_RELEASE_DIR="$sandbox/release" \
    MOCK_UNAME_S="${MOCK_UNAME_S:-Darwin}" \
    MOCK_UNAME_M="${MOCK_UNAME_M:-arm64}" \
    MOCK_CURL_FAIL_ASSET="${MOCK_CURL_FAIL_ASSET-}" \
    MOCK_XATTR_STATUS="${MOCK_XATTR_STATUS:-1}" \
    TMPDIR="${TEST_TMPDIR:-${TMPDIR:-/tmp}}" \
    TRENCH_INSTALL_DIR="$sandbox/install/bin" \
    sh "$installer" "$@"
}

run_installer_with_defaults() {
  sandbox=$1
  env -u XDG_DATA_HOME -u TRENCH_INSTALL_DIR \
    HOME="$sandbox/home" \
    SHELL=/bin/zsh \
    PATH="$sandbox/mock-bin:/usr/bin:/bin" \
    MOCK_RELEASE_DIR="$sandbox/release" \
    MOCK_UNAME_S=Darwin \
    MOCK_UNAME_M=arm64 \
    ZDOTDIR= \
    sh "$installer" --no-modify-shell
}

test_arm64_selects_apple_silicon_archive() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin

  output=$(MOCK_UNAME_M=arm64 run_installer "$sandbox") || fail "installer failed: $output"

  [ -x "$sandbox/install/bin/trench" ] || fail "installed executable is missing"
  assert_contains "$output" "Architecture: aarch64-apple-darwin"
  printf 'ok %d - arm64 selects Apple Silicon archive\n' "$tests_run"
}

test_zsh_configuration_adds_path_and_shell_init() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' '# keep this line' >"$sandbox/home/.zshrc"

  output=$(run_installer_with_shell "$sandbox" /bin/zsh) || fail "installer failed: $output"

  canonical_install_dir=$(CDPATH='' cd -P -- "$sandbox/install/bin" && pwd -P)
  config=$(cat "$sandbox/home/.zshrc")
  assert_contains "$config" '# keep this line'
  assert_contains "$config" '# >>> trench >>>'
  assert_contains "$config" "export PATH='$canonical_install_dir':\"\$PATH\""
  # Assert the literal managed-shell command.
  # shellcheck disable=SC2016
  assert_contains "$config" 'eval "$(trench shell-init zsh)"'
  backups=$(printf '%s\n' "$sandbox"/home/.zshrc.trench.bak.*)
  [ -f "$backups" ] || fail 'zsh configuration backup is missing'
  assert_contains "$output" "Modified shell configuration: $sandbox/home/.zshrc"
  assert_contains "$output" "Backup: $backups"
  printf 'ok %d - zsh configuration adds PATH and shell init\n' "$tests_run"
}

test_rejects_tampered_release_manifest() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' 'tampered' >>"$sandbox/release/trench-release.json"

  if output=$(run_installer "$sandbox" 2>&1); then
    fail 'installer accepted a tampered release manifest'
  fi

  assert_contains "$output" 'checksum verification failed for trench-release.json'
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'tampered release installed an executable'
  printf 'ok %d - rejects tampered release manifest\n' "$tests_run"
}

test_manifest_digest_must_belong_to_selected_archive() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  asset=trench-aarch64-apple-darwin.tar.gz
  make_release "$sandbox/release" "$asset" aarch64-apple-darwin
  archive_sha=$(shasum -a 256 "$sandbox/release/$asset" | awk '{print $1}')
  wrong_sha=$(printf '0%.0s' {1..64})
  cat >"$sandbox/release/trench-release.json" <<EOF
{"schema":1,"tag":"v0.1.0","version":"0.1.0","commit":"0123456789abcdef0123456789abcdef01234567","assets":[{"name":"$asset","sha256":"$wrong_sha"},{"name":"other","sha256":"$archive_sha"}]}
EOF
  manifest_sha=$(shasum -a 256 "$sandbox/release/trench-release.json" | awk '{print $1}')
  printf '%s  %s\n%s  %s\n' \
    "$archive_sha" "$asset" \
    "$manifest_sha" trench-release.json \
    >"$sandbox/release/trench-checksums.txt"

  if output=$(run_installer "$sandbox" 2>&1); then
    fail 'installer accepted a manifest whose selected asset had a different digest'
  fi
  assert_contains "$output" "release manifest checksum does not match $asset"
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'cross-wired manifest installed an executable'
  printf 'ok %d - selected archive owns its manifest digest\n' "$tests_run"
}

test_x86_64_selects_intel_archive() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-x86_64-apple-darwin.tar.gz \
    x86_64-apple-darwin

  output=$(MOCK_UNAME_M=x86_64 run_installer "$sandbox") || fail "installer failed: $output"

  assert_contains "$output" 'Architecture: x86_64-apple-darwin'
  printf 'ok %d - x86_64 selects Intel archive\n' "$tests_run"
}

test_rejects_unsupported_platforms() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"

  if output=$(MOCK_UNAME_S=Linux run_installer "$sandbox" 2>&1); then
    fail 'installer accepted Linux'
  fi
  assert_contains "$output" 'unsupported operating system: Linux'

  if output=$(MOCK_UNAME_M=powerpc run_installer "$sandbox" 2>&1); then
    fail 'installer accepted an unsupported macOS architecture'
  fi
  assert_contains "$output" 'unsupported macOS architecture: powerpc'
  printf 'ok %d - rejects unsupported platforms\n' "$tests_run"
}

test_failed_download_cleans_temporary_files() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release" "$sandbox/tmp"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin

  if output=$(MOCK_CURL_FAIL_ASSET=trench-aarch64-apple-darwin.tar.gz TEST_TMPDIR="$sandbox/tmp" run_installer "$sandbox" 2>&1); then
    fail 'installer accepted an interrupted archive download'
  fi

  assert_contains "$output" 'failed to download trench-aarch64-apple-darwin.tar.gz'
  [ -z "$(ls -A "$sandbox/tmp")" ] || fail 'temporary download files were not cleaned'
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'failed download replaced the executable'
  printf 'ok %d - failed download cleans temporary files\n' "$tests_run"
}

test_checksum_failure_preserves_existing_installation() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release" "$sandbox/install/bin"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' 'corrupt' >>"$sandbox/release/trench-aarch64-apple-darwin.tar.gz"
  printf '%s\n' 'existing trench' >"$sandbox/install/bin/trench"

  if output=$(run_installer "$sandbox" 2>&1); then
    fail 'installer accepted an archive with a bad checksum'
  fi

  assert_contains "$output" 'checksum verification failed for trench-aarch64-apple-darwin.tar.gz'
  [ "$(cat "$sandbox/install/bin/trench")" = 'existing trench' ] || fail 'failed verification replaced the existing executable'
  printf 'ok %d - checksum failure preserves existing installation\n' "$tests_run"
}

test_refuses_to_replace_an_unowned_existing_executable() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release" "$sandbox/install/bin"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' 'manual trench' >"$sandbox/install/bin/trench"

  if output=$(run_installer "$sandbox" 2>&1); then
    fail 'installer replaced an unowned existing executable'
  fi

  assert_contains "$output" 'refusing to replace an existing executable without a matching standalone installation receipt'
  [ "$(cat "$sandbox/install/bin/trench")" = 'manual trench' ] || fail 'unowned executable was modified'
  printf 'ok %d - refuses to replace an unowned existing executable\n' "$tests_run"
}

test_refuses_to_replace_an_executable_with_a_stale_receipt() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release" "$sandbox/install/bin" "$sandbox/data/trench"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' 'manual trench' >"$sandbox/install/bin/trench"
  printf '%s\n' '{"schema":1,"manager":"standalone","executable":"/different/trench"}' \
    >"$sandbox/data/trench/install-receipt.json"

  if output=$(run_installer "$sandbox" 2>&1); then
    fail 'installer accepted a stale ownership receipt'
  fi

  assert_contains "$output" 'refusing to replace an existing executable without a matching standalone installation receipt'
  [ "$(cat "$sandbox/install/bin/trench")" = 'manual trench' ] || fail 'stale receipt authorized replacement'
  printf 'ok %d - refuses to replace an executable with a stale receipt\n' "$tests_run"
}

test_matching_receipt_authorizes_a_standalone_reinstall() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release" "$sandbox/install/bin" "$sandbox/data/trench"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' 'old standalone trench' >"$sandbox/install/bin/trench"
  canonical_executable=$(CDPATH='' cd -P -- "$sandbox/install/bin" && printf '%s/trench' "$(pwd -P)")
  printf '{"schema":1,"manager":"standalone","executable":"%s"}\n' "$canonical_executable" \
    >"$sandbox/data/trench/install-receipt.json"

  output=$(run_installer "$sandbox") || fail "matching receipt did not authorize reinstall: $output"

  [ -x "$sandbox/install/bin/trench" ] || fail 'reinstalled executable is missing'
  [ "$("$sandbox/install/bin/trench" --version)" = 'trench 0.1.0' ] || fail 'standalone executable was not replaced'
  printf 'ok %d - matching receipt authorizes a standalone reinstall\n' "$tests_run"
}

test_invalid_receipt_target_fails_before_installing() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release" "$sandbox/data/trench/install-receipt.json"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin

  if output=$(run_installer "$sandbox" 2>&1); then
    fail 'installer accepted a directory as the ownership receipt'
  fi

  assert_contains "$output" 'ownership receipt target is not a regular file'
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'invalid receipt target was rejected after installation'
  printf 'ok %d - invalid receipt target fails before installing\n' "$tests_run"
}

test_receipt_staging_failure_preserves_a_fresh_destination() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release" "$sandbox/data/trench"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  chmod 500 "$sandbox/data/trench"

  if output=$(run_installer "$sandbox" 2>&1); then
    chmod 700 "$sandbox/data/trench"
    fail 'installer succeeded without staging its ownership receipt'
  fi
  chmod 700 "$sandbox/data/trench"

  assert_contains "$output" 'could not write installation receipt'
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'receipt staging failed after installation'
  printf 'ok %d - receipt staging failure preserves a fresh destination\n' "$tests_run"
}

test_default_install_writes_minimal_xdg_fallback_receipt() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin

  run_installer_with_defaults "$sandbox" >/dev/null || fail 'default installation failed'

  executable="$sandbox/home/.local/bin/trench"
  [ -x "$executable" ] || fail 'default executable is missing or not executable'
  canonical_executable=$(CDPATH='' cd -P -- "$(dirname "$executable")" && printf '%s/trench' "$(pwd -P)")
  receipt="$sandbox/home/.local/share/trench/install-receipt.json"
  expected_receipt="{\"schema\":1,\"manager\":\"standalone\",\"executable\":\"$canonical_executable\"}"
  [ "$(cat "$receipt")" = "$expected_receipt" ] || fail 'ownership receipt contains unexpected state'
  printf 'ok %d - default install writes minimal XDG fallback receipt\n' "$tests_run"
}

test_zshenv_discovered_zdotdir_is_respected() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  # Fixture must expand HOME when zsh reads it.
  # shellcheck disable=SC2016
  printf '%s\n' 'export ZDOTDIR="$HOME/.config/zsh"' >"$sandbox/home/.zshenv"

  output=$(run_installer_with_shell "$sandbox" /bin/zsh) || fail "installer failed: $output"

  config="$sandbox/home/.config/zsh/.zshrc"
  [ -f "$config" ] || fail 'installer did not discover ZDOTDIR from .zshenv'
  assert_contains "$(cat "$config")" 'trench shell-init zsh'
  [ ! -e "$sandbox/home/.zshrc" ] || fail 'installer also modified the default .zshrc'
  printf 'ok %d - zshenv-discovered ZDOTDIR is respected\n' "$tests_run"
}

test_invalid_inherited_zdotdir_fails_before_installing() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin

  if output=$(TEST_ZDOTDIR=relative/zsh run_installer_with_shell "$sandbox" /bin/zsh 2>&1); then
    fail 'installer accepted a relative ZDOTDIR'
  fi

  assert_contains "$output" 'ZDOTDIR must be an absolute path: relative/zsh'
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'invalid ZDOTDIR was rejected only after installation'
  printf 'ok %d - invalid inherited ZDOTDIR fails before installing\n' "$tests_run"
}

test_multiline_zdotdir_is_rejected() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  invalid_zdotdir=$(printf '%s\n%s' "$sandbox/one" "$sandbox/two")

  if output=$(TEST_ZDOTDIR="$invalid_zdotdir" run_installer_with_shell "$sandbox" /bin/zsh 2>&1); then
    fail 'installer accepted multiple ZDOTDIR values'
  fi

  assert_contains "$output" 'ZDOTDIR must contain one absolute path'
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'multiline ZDOTDIR was rejected only after installation'
  printf 'ok %d - multiline ZDOTDIR is rejected\n' "$tests_run"
}

test_inherited_zdotdir_is_preferred() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  inherited_zdotdir=$sandbox/inherited-zsh

  TEST_ZDOTDIR="$inherited_zdotdir" run_installer_with_shell "$sandbox" /bin/zsh >/dev/null ||
    fail 'installation with inherited ZDOTDIR failed'

  [ -f "$inherited_zdotdir/.zshrc" ] || fail 'inherited ZDOTDIR was not used'
  [ ! -e "$sandbox/home/.zshrc" ] || fail 'default .zshrc was modified despite inherited ZDOTDIR'
  printf 'ok %d - inherited ZDOTDIR is preferred\n' "$tests_run"
}

test_bash_and_fish_use_native_login_configuration() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' '# bash profile' >"$sandbox/home/.bash_profile"

  run_installer_with_shell "$sandbox" /bin/bash >/dev/null || fail 'bash configuration failed'
  bash_config=$(cat "$sandbox/home/.bash_profile")
  # Assert the literal managed-shell command.
  # shellcheck disable=SC2016
  assert_contains "$bash_config" 'eval "$(trench shell-init bash)"'
  assert_contains "$bash_config" 'export PATH='

  XDG_CONFIG_HOME="$sandbox/fish-config" run_installer_with_shell "$sandbox" /usr/local/bin/fish >/dev/null ||
    fail 'fish configuration failed'
  fish_config=$(cat "$sandbox/fish-config/fish/config.fish")
  assert_contains "$fish_config" 'fish_add_path --prepend --path'
  assert_contains "$fish_config" 'trench shell-init fish | source'
  printf 'ok %d - bash and fish use native login configuration\n' "$tests_run"
}

test_rerun_leaves_managed_shell_block_unchanged() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' '# unrelated content' >"$sandbox/home/.zshrc"

  run_installer_with_shell "$sandbox" /bin/zsh >/dev/null || fail 'first installation failed'
  first_config=$(cat "$sandbox/home/.zshrc")
  first_backup_count=$(printf '%s\n' "$sandbox"/home/.zshrc.trench.bak.* | wc -l | tr -d ' ')
  canonical_install_dir=$(CDPATH='' cd -P -- "$sandbox/install/bin" && pwd -P)
  reloaded_path="$canonical_install_dir:$sandbox/mock-bin:/usr/bin:/bin"
  output=$(TEST_PATH="$reloaded_path" run_installer_with_shell "$sandbox" /bin/zsh) || fail 'second installation failed'

  [ "$(cat "$sandbox/home/.zshrc")" = "$first_config" ] || fail 'rerun changed an identical managed block'
  second_backup_count=$(printf '%s\n' "$sandbox"/home/.zshrc.trench.bak.* | wc -l | tr -d ' ')
  [ "$second_backup_count" = "$first_backup_count" ] || fail 'rerun created an unnecessary backup'
  [ "$(grep -c '^# >>> trench >>>$' "$sandbox/home/.zshrc")" = 1 ] || fail 'rerun duplicated the managed block'
  assert_contains "$output" 'already contained the managed Trench block; left unchanged'
  printf 'ok %d - rerun leaves managed shell block unchanged\n' "$tests_run"
}

test_existing_path_entry_is_not_duplicated() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release" "$sandbox/install/bin"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  canonical_install_dir=$(CDPATH='' cd -P -- "$sandbox/install/bin" && pwd -P)
  existing_path="$canonical_install_dir:$sandbox/mock-bin:/usr/bin:/bin"

  output=$(TEST_PATH="$existing_path" run_installer_with_shell "$sandbox" /bin/zsh) || fail 'installation failed'

  config=$(cat "$sandbox/home/.zshrc")
  assert_contains "$config" 'trench shell-init zsh'
  case "$config" in
    *'export PATH='*) fail 'installer duplicated an existing PATH entry' ;;
  esac
  assert_contains "$output" "PATH: $canonical_install_dir was already present; no PATH entry was added"
  printf 'ok %d - existing PATH entry is not duplicated\n' "$tests_run"
}

test_install_directory_is_prepended_when_present_later_in_path() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release" "$sandbox/install/bin"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  canonical_install_dir=$(CDPATH='' cd -P -- "$sandbox/install/bin" && pwd -P)
  later_path="$sandbox/mock-bin:/usr/bin:$canonical_install_dir:/bin"

  output=$(TEST_PATH="$later_path" run_installer_with_shell "$sandbox" /bin/zsh) || fail 'installation failed'

  config=$(cat "$sandbox/home/.zshrc")
  assert_contains "$config" "export PATH='$canonical_install_dir':\"\$PATH\""
  assert_contains "$output" "PATH: $canonical_install_dir was present but not first; configured Trench to take precedence"
  printf 'ok %d - install directory is prepended when present later in PATH\n' "$tests_run"
}

test_managed_block_is_replaced_without_touching_other_content() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  cat >"$sandbox/home/.zshrc" <<'EOF'
# before
# >>> trench >>>
old trench setup
# <<< trench <<<
# after
EOF
  original=$(cat "$sandbox/home/.zshrc")

  run_installer_with_shell "$sandbox" /bin/zsh >/dev/null || fail 'managed-block replacement failed'

  config=$(cat "$sandbox/home/.zshrc")
  assert_contains "$config" '# before'
  assert_contains "$config" '# after'
  assert_contains "$config" 'trench shell-init zsh'
  case "$config" in
    *'old trench setup'*) fail 'old managed-block content was retained' ;;
  esac
  [ "$(grep -c '^# >>> trench >>>$' "$sandbox/home/.zshrc")" = 1 ] || fail 'managed block was duplicated'
  backup=$(printf '%s\n' "$sandbox"/home/.zshrc.trench.bak.*)
  [ "$(cat "$backup")" = "$original" ] || fail 'backup did not preserve the original configuration'
  printf 'ok %d - managed block replacement preserves unrelated content\n' "$tests_run"
}

test_no_modify_shell_and_quarantine_summaries_are_factual() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' '# untouched' >"$sandbox/home/.zshrc"

  output=$(MOCK_XATTR_STATUS=0 run_installer "$sandbox") || fail 'no-modify-shell installation failed'

  [ "$(cat "$sandbox/home/.zshrc")" = '# untouched' ] || fail '--no-modify-shell changed shell configuration'
  [ -z "$(printf '%s\n' "$sandbox"/home/.zshrc.trench.bak.* | sed -n '/[*]/!p')" ] || fail '--no-modify-shell created a backup'
  assert_contains "$output" 'Shell configuration: left unchanged (--no-modify-shell)'
  assert_contains "$output" 'Quarantine: removed from'
  printf 'ok %d - no-modify-shell and quarantine summaries are factual\n' "$tests_run"
}

test_rejects_unbalanced_managed_shell_markers_without_modifying_config() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  cat >"$sandbox/home/.zshrc" <<'EOF'
# unrelated content before
# >>> trench >>>
old trench setup
# unrelated content after
EOF
  original=$(cat "$sandbox/home/.zshrc")

  if output=$(run_installer_with_shell "$sandbox" /bin/zsh 2>&1); then
    fail 'installer accepted unbalanced managed markers'
  fi

  assert_contains "$output" 'shell configuration contains an invalid managed Trench block'
  [ "$(cat "$sandbox/home/.zshrc")" = "$original" ] || fail 'invalid managed block changed shell configuration'
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'invalid managed block was rejected only after installation'
  printf 'ok %d - rejects unbalanced managed markers without modifying config\n' "$tests_run"
}

test_rejects_archive_with_unexpected_layout_before_extraction() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  printf '%s\n' 'unexpected' >"$sandbox/release/staging/unexpected"
  tar -C "$sandbox/release/staging" -czf "$sandbox/release/trench-aarch64-apple-darwin.tar.gz" \
    trench LICENSE README.md unexpected
  archive_sha=$(shasum -a 256 "$sandbox/release/trench-aarch64-apple-darwin.tar.gz" | awk '{print $1}')
  sed -E "s/[0123456789abcdef]{64}/$archive_sha/" "$sandbox/release/trench-release.json" >"$sandbox/release/manifest.next"
  mv "$sandbox/release/manifest.next" "$sandbox/release/trench-release.json"
  manifest_sha=$(shasum -a 256 "$sandbox/release/trench-release.json" | awk '{print $1}')
  printf '%s  %s\n%s  %s\n' \
    "$archive_sha" trench-aarch64-apple-darwin.tar.gz \
    "$manifest_sha" trench-release.json \
    >"$sandbox/release/trench-checksums.txt"

  if output=$(run_installer "$sandbox" 2>&1); then
    fail 'installer accepted an archive with an unexpected entry'
  fi

  assert_contains "$output" 'archive contains an unsafe or unexpected layout'
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'unsafe archive installed an executable'
  printf 'ok %d - rejects archive with unexpected layout before extraction\n' "$tests_run"
}

test_rejects_archive_with_a_linked_executable() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin
  cat >"$sandbox/outside-trench" <<'EOF'
#!/bin/sh
printf '%s\n' 'trench 0.1.0'
EOF
  chmod +x "$sandbox/outside-trench"
  unlink "$sandbox/release/staging/trench"
  ln -s "$sandbox/outside-trench" "$sandbox/release/staging/trench"
  tar -C "$sandbox/release/staging" -czf "$sandbox/release/trench-aarch64-apple-darwin.tar.gz" \
    trench LICENSE README.md
  archive_sha=$(shasum -a 256 "$sandbox/release/trench-aarch64-apple-darwin.tar.gz" | awk '{print $1}')
  sed -E "s/[0123456789abcdef]{64}/$archive_sha/" "$sandbox/release/trench-release.json" >"$sandbox/release/manifest.next"
  mv "$sandbox/release/manifest.next" "$sandbox/release/trench-release.json"
  manifest_sha=$(shasum -a 256 "$sandbox/release/trench-release.json" | awk '{print $1}')
  printf '%s  %s\n%s  %s\n' \
    "$archive_sha" trench-aarch64-apple-darwin.tar.gz \
    "$manifest_sha" trench-release.json \
    >"$sandbox/release/trench-checksums.txt"

  if output=$(run_installer "$sandbox" 2>&1); then
    fail 'installer accepted an archive containing a linked executable'
  fi

  assert_contains "$output" 'archive contains an unsafe or unexpected layout'
  [ ! -e "$sandbox/install/bin/trench" ] || fail 'linked executable was installed'
  printf 'ok %d - rejects archive with a linked executable\n' "$tests_run"
}

test_relative_xdg_directories_fall_back_to_home() {
  tests_run=$((tests_run + 1))
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer-test.XXXXXX")
  trap 'rm -rf "$sandbox"' RETURN
  mkdir -p "$sandbox/home" "$sandbox/release"
  write_mock_commands "$sandbox/mock-bin"
  make_release "$sandbox/release" \
    trench-aarch64-apple-darwin.tar.gz \
    aarch64-apple-darwin

  TEST_XDG_DATA_HOME=relative/data run_installer "$sandbox" >/dev/null ||
    fail 'installation with relative XDG_DATA_HOME failed'
  [ -f "$sandbox/home/.local/share/trench/install-receipt.json" ] ||
    fail 'relative XDG_DATA_HOME did not fall back to HOME'
  [ ! -e relative/data/trench/install-receipt.json ] || fail 'relative XDG_DATA_HOME was used'

  XDG_CONFIG_HOME=relative/config TEST_XDG_DATA_HOME=relative/data \
    run_installer_with_shell "$sandbox" /usr/local/bin/fish >/dev/null ||
    fail 'fish installation with relative XDG_CONFIG_HOME failed'
  [ -f "$sandbox/home/.config/fish/config.fish" ] || fail 'relative XDG_CONFIG_HOME did not fall back to HOME'
  [ ! -e relative/config/fish/config.fish ] || fail 'relative XDG_CONFIG_HOME was used'
  printf 'ok %d - relative XDG directories fall back to HOME\n' "$tests_run"
}

test_arm64_selects_apple_silicon_archive
test_zsh_configuration_adds_path_and_shell_init
test_rejects_tampered_release_manifest
test_manifest_digest_must_belong_to_selected_archive
test_x86_64_selects_intel_archive
test_rejects_unsupported_platforms
test_failed_download_cleans_temporary_files
test_checksum_failure_preserves_existing_installation
test_refuses_to_replace_an_unowned_existing_executable
test_refuses_to_replace_an_executable_with_a_stale_receipt
test_matching_receipt_authorizes_a_standalone_reinstall
test_invalid_receipt_target_fails_before_installing
test_receipt_staging_failure_preserves_a_fresh_destination
test_default_install_writes_minimal_xdg_fallback_receipt
test_zshenv_discovered_zdotdir_is_respected
test_invalid_inherited_zdotdir_fails_before_installing
test_multiline_zdotdir_is_rejected
test_inherited_zdotdir_is_preferred
test_bash_and_fish_use_native_login_configuration
test_rerun_leaves_managed_shell_block_unchanged
test_existing_path_entry_is_not_duplicated
test_install_directory_is_prepended_when_present_later_in_path
test_managed_block_is_replaced_without_touching_other_content
test_no_modify_shell_and_quarantine_summaries_are_factual
test_rejects_unbalanced_managed_shell_markers_without_modifying_config
test_rejects_archive_with_unexpected_layout_before_extraction
test_rejects_archive_with_a_linked_executable
test_relative_xdg_directories_fall_back_to_home
