#!/bin/sh

set -eu

repository=${TRENCH_REPOSITORY:-sadiksaifi/trench}
release_base_url=${TRENCH_RELEASE_BASE_URL:-https://github.com/$repository/releases/latest/download}
install_dir=${TRENCH_INSTALL_DIR:-$HOME/.local/bin}
modify_shell=1
temp_dir=
atomic_candidate=
receipt_temp=

xdg_dir_or_default() {
  candidate=$1
  fallback=$2
  case "$candidate" in
    /*) printf '%s\n' "$candidate" ;;
    *) printf '%s\n' "$fallback" ;;
  esac
}

die() {
  printf 'trench installer: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [ -n "$atomic_candidate" ] && [ -e "$atomic_candidate" ]; then
    rm -f "$atomic_candidate"
  fi
  if [ -n "$receipt_temp" ] && [ -e "$receipt_temp" ]; then
    rm -f "$receipt_temp"
  fi
  if [ -n "$temp_dir" ] && [ -d "$temp_dir" ]; then
    rm -rf "$temp_dir"
  fi
}

trap cleanup 0
trap 'exit 1' HUP INT TERM

while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-modify-shell) modify_shell=0 ;;
    -h | --help)
      printf '%s\n' 'Usage: trench-installer.sh [--no-modify-shell]'
      exit 0
      ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done

os=$(uname -s) || die 'could not detect the operating system'
[ "$os" = Darwin ] || die "unsupported operating system: $os (macOS is required)"

machine=$(uname -m) || die 'could not detect the machine architecture'
case "$machine" in
  arm64) target=aarch64-apple-darwin ;;
  x86_64) target=x86_64-apple-darwin ;;
  *) die "unsupported macOS architecture: $machine" ;;
esac
archive=trench-$target.tar.gz

resolve_shell_configuration() {
  shell_name=${SHELL:-/bin/zsh}
  shell_name=${shell_name##*/}
  case "$shell_name" in
    zsh)
      selected_zdotdir=${ZDOTDIR-}
      if [ -z "$selected_zdotdir" ] && command -v zsh >/dev/null 2>&1; then
        selected_zdotdir=$(zsh -lc 'printf %s "${ZDOTDIR-}"') ||
          die 'could not evaluate zsh startup environment'
      fi
      if [ -n "$selected_zdotdir" ]; then
        case "$selected_zdotdir" in
          *'
'*) die 'ZDOTDIR must contain one absolute path' ;;
        esac
        case "$selected_zdotdir" in
          /*) ;;
          *) die "ZDOTDIR must be an absolute path: $selected_zdotdir" ;;
        esac
        config_file=$selected_zdotdir/.zshrc
      else
        config_file=$HOME/.zshrc
      fi
      # This command is written literally into .zshrc.
      # shellcheck disable=SC2016
      shell_init='eval "$(trench shell-init zsh)"'
      ;;
    bash)
      if [ -f "$HOME/.bash_profile" ]; then
        config_file=$HOME/.bash_profile
      elif [ -f "$HOME/.bash_login" ]; then
        config_file=$HOME/.bash_login
      elif [ -f "$HOME/.profile" ]; then
        config_file=$HOME/.profile
      else
        config_file=$HOME/.bash_profile
      fi
      # This command is written literally into the bash profile.
      # shellcheck disable=SC2016
      shell_init='eval "$(trench shell-init bash)"'
      ;;
    fish)
      fish_config_home=$(xdg_dir_or_default "${XDG_CONFIG_HOME-}" "$HOME/.config")
      config_file=$fish_config_home/fish/config.fish
      shell_init='trench shell-init fish | source'
      ;;
    *) die "unsupported shell for configuration: $shell_name (use zsh, bash, fish, or --no-modify-shell)" ;;
  esac
}

validate_managed_block() {
  [ -f "$config_file" ] || return 0
  awk '
      $0 == "# >>> trench >>>" {
          if (managed || ++open_count > 1) exit 1
          managed = 1
          next
      }
      $0 == "# <<< trench <<<" {
          if (!managed || ++close_count > 1) exit 1
          managed = 0
      }
      END {
          if (managed || open_count != close_count) exit 1
      }
  ' "$config_file" || die "shell configuration contains an invalid managed Trench block: $config_file"
}

if [ "$modify_shell" -eq 1 ]; then
  resolve_shell_configuration
  validate_managed_block
fi

command -v curl >/dev/null 2>&1 || die 'curl is required'
command -v tar >/dev/null 2>&1 || die 'tar is required'
command -v shasum >/dev/null 2>&1 || die 'shasum is required'

temp_dir=$(mktemp -d "${TMPDIR:-/tmp}/trench-installer.XXXXXX") || die 'could not create a temporary directory'

download() {
  asset=$1
  destination=$2
  curl --proto '=https' --tlsv1.2 -fLsS "$release_base_url/$asset" -o "$destination" ||
    die "failed to download $asset"
}

download trench-release.json "$temp_dir/trench-release.json"
download trench-checksums.txt "$temp_dir/trench-checksums.txt"

verify_checksum() {
  asset=$1
  file=$2
  expected_sha=$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset { print $1; exit }' "$temp_dir/trench-checksums.txt")
  [ -n "$expected_sha" ] || die "checksum is missing for $asset"
  printf '%s\n' "$expected_sha" | grep -Eq '^[0123456789abcdefABCDEF]{64}$' ||
    die "checksum is invalid for $asset"
  actual_sha=$(shasum -a 256 "$file" | awk '{print $1}')
  [ "$actual_sha" = "$expected_sha" ] || die "checksum verification failed for $asset"
}

verify_checksum trench-release.json "$temp_dir/trench-release.json"

manifest_compact=$temp_dir/trench-release.compact.json
tr -d '[:space:]' <"$temp_dir/trench-release.json" >"$manifest_compact"
grep -F '"schema":1' "$manifest_compact" >/dev/null 2>&1 || die 'release manifest has an unsupported schema'
manifest_tag=$(sed -n 's/.*"tag":"\([^"]*\)".*/\1/p' "$manifest_compact")
manifest_version=$(sed -n 's/.*"version":"\([^"]*\)".*/\1/p' "$manifest_compact")
printf '%s\n' "$manifest_tag" | grep -Eq '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' ||
  die 'release manifest has an invalid canonical tag'
[ "$manifest_tag" = "v$manifest_version" ] || die 'release manifest tag and version do not match'

download "$archive" "$temp_dir/$archive"
verify_checksum "$archive" "$temp_dir/$archive"
grep -F "\"name\":\"$archive\",\"sha256\":\"$expected_sha\"" "$manifest_compact" >/dev/null 2>&1 ||
  die "release manifest checksum does not match $archive"

mkdir "$temp_dir/extracted"
archive_listing=$temp_dir/archive-listing
tar -tzf "$temp_dir/$archive" >"$archive_listing" || die "failed to inspect $archive"
awk '
    $0 == "trench" { trench += 1; next }
    $0 == "LICENSE" { license += 1; next }
    $0 == "README.md" { readme += 1; next }
    { exit 1 }
    END {
        if (trench != 1 || license != 1 || readme != 1) exit 1
    }
' "$archive_listing" || die 'archive contains an unsafe or unexpected layout'
archive_details=$temp_dir/archive-details
LC_ALL=C tar -tvzf "$temp_dir/$archive" >"$archive_details" || die "failed to inspect $archive"
awk '
    substr($1, 1, 1) != "-" { exit 1 }
    END { if (NR != 3) exit 1 }
' "$archive_details" || die 'archive contains an unsafe or unexpected layout'
tar -xzf "$temp_dir/$archive" -C "$temp_dir/extracted" || die "failed to extract $archive"
[ -f "$temp_dir/extracted/trench" ] && [ ! -L "$temp_dir/extracted/trench" ] ||
  die 'archive contains an unsafe or unexpected layout'
chmod 755 "$temp_dir/extracted/trench" || die 'could not make trench executable'
candidate_version=$("$temp_dir/extracted/trench" --version 2>/dev/null) || die 'downloaded trench failed its smoke test'
[ "$candidate_version" = "trench $manifest_version" ] ||
  die "downloaded trench version does not match release manifest (expected $manifest_version)"

mkdir -p "$install_dir" || die "could not create installation directory: $install_dir"
canonical_install_dir=$(CDPATH='' cd -P -- "$install_dir" && pwd -P) ||
  die "could not resolve installation directory: $install_dir"
installed_executable=$canonical_install_dir/trench

data_home=$(xdg_dir_or_default "${XDG_DATA_HOME-}" "$HOME/.local/share")
receipt_dir=$data_home/trench
receipt=$receipt_dir/install-receipt.json
escaped_executable=$(printf '%s' "$installed_executable" | sed 's/\\/\\\\/g; s/"/\\"/g')
expected_receipt=$(printf '{"schema":1,"manager":"standalone","executable":"%s"}' "$escaped_executable")

if { [ -e "$receipt" ] || [ -L "$receipt" ]; } &&
  { [ ! -f "$receipt" ] || [ -L "$receipt" ]; }; then
  die "ownership receipt target is not a regular file: $receipt"
fi

if [ -e "$installed_executable" ] || [ -L "$installed_executable" ]; then
  if [ ! -f "$installed_executable" ] || [ -L "$installed_executable" ] ||
    [ ! -f "$receipt" ] || [ "$(cat "$receipt")" != "$expected_receipt" ]; then
    die "refusing to replace an existing executable without a matching standalone installation receipt: $installed_executable"
  fi
fi

mkdir -p "$receipt_dir" || die "could not create receipt directory: $receipt_dir"
receipt_temp=$receipt_dir/.install-receipt.$$.json
printf '%s\n' "$expected_receipt" >"$receipt_temp" ||
  die 'could not write installation receipt'
mv -f "$receipt_temp" "$receipt" || die 'could not install ownership receipt'
receipt_temp=

atomic_candidate=$canonical_install_dir/.trench.install.$$
cp "$temp_dir/extracted/trench" "$atomic_candidate" || die 'could not stage trench for installation'
chmod 755 "$atomic_candidate" || die 'could not set executable permissions'
mv -f "$atomic_candidate" "$installed_executable" || die 'could not install trench atomically'
atomic_candidate=

modified_config=
config_backup=
shell_unchanged=
path_already_present=0
path_precedence_added=0

path_is_first() {
  case "${PATH-}" in
    "$canonical_install_dir" | "$canonical_install_dir":*) return 0 ;;
    *) return 1 ;;
  esac
}

path_is_present() {
  case ":${PATH-}:" in
    *":$canonical_install_dir:"*) return 0 ;;
    *) return 1 ;;
  esac
}

single_quote() {
  printf '%s' "$1" | sed "s/'/'\\\\''/g"
}

configure_shell() {
  config_parent=${config_file%/*}
  mkdir -p "$config_parent" || die "could not create shell configuration directory: $config_parent"
  quoted_install_dir=$(single_quote "$canonical_install_dir")
  path_line="export PATH='$quoted_install_dir':\"\$PATH\""
  if [ "$shell_name" = fish ]; then
    config_path_line="fish_add_path --prepend --path '$quoted_install_dir'"
  else
    config_path_line=$path_line
  fi
  need_path=1
  if [ -f "$config_file" ] && grep -F "$config_path_line" "$config_file" >/dev/null 2>&1; then
    need_path=1
  elif path_is_first; then
    need_path=0
    path_already_present=1
  elif path_is_present; then
    need_path=1
    path_precedence_added=1
  fi

  block_file=$temp_dir/shell-block
  {
    printf '%s\n' '# >>> trench >>>'
    if [ "$need_path" -eq 1 ]; then
      printf '%s\n' "$config_path_line"
    fi
    printf '%s\n' "$shell_init"
    printf '%s\n' '# <<< trench <<<'
  } >"$block_file"

  stripped_file=$temp_dir/shell-config-stripped
  if [ -f "$config_file" ]; then
    awk '
            $0 == "# >>> trench >>>" { managed = 1; next }
            $0 == "# <<< trench <<<" { managed = 0; next }
            !managed { print }
        ' "$config_file" >"$stripped_file"
  else
    : >"$stripped_file"
  fi
  while [ -s "$stripped_file" ] && [ "$(tail -n 1 "$stripped_file")" = '' ]; do
    sed '$d' "$stripped_file" >"$stripped_file.next"
    mv "$stripped_file.next" "$stripped_file"
  done

  desired_file=$temp_dir/shell-config-desired
  if [ -s "$stripped_file" ]; then
    cat "$stripped_file" >"$desired_file"
    printf '\n' >>"$desired_file"
  else
    : >"$desired_file"
  fi
  cat "$block_file" >>"$desired_file"

  if [ -f "$config_file" ] && cmp -s "$config_file" "$desired_file"; then
    shell_unchanged=$config_file
    return
  fi

  if [ -f "$config_file" ]; then
    config_backup=$config_file.trench.bak.$(date +%Y%m%d%H%M%S).$$
    cp -p "$config_file" "$config_backup" || die "could not back up shell configuration: $config_file"
  fi
  config_temp=$config_file.trench.tmp.$$
  if [ -f "$config_file" ]; then
    cp -p "$config_file" "$config_temp" || die "could not stage shell configuration: $config_file"
  fi
  cat "$desired_file" >"$config_temp" || die "could not write shell configuration: $config_file"
  mv -f "$config_temp" "$config_file" || die "could not update shell configuration: $config_file"
  modified_config=$config_file
}

if [ "$modify_shell" -eq 1 ]; then
  configure_shell
fi

# Quarantine removal is intentionally best effort for unsigned release artifacts.
quarantine_removed=0
if command -v xattr >/dev/null 2>&1 && xattr -d com.apple.quarantine "$installed_executable" >/dev/null 2>&1; then
  quarantine_removed=1
fi

printf '%s\n' 'Trench installation complete.'
printf '  Architecture: %s\n' "$target"
printf '  Installed executable: %s\n' "$installed_executable"
printf '  Receipt: %s\n' "$receipt"
printf '  Verification: release manifest and SHA-256 checksum verified\n'
if [ -n "$modified_config" ]; then
  printf '  Modified shell configuration: %s\n' "$modified_config"
fi
if [ -n "$config_backup" ]; then
  printf '  Backup: %s\n' "$config_backup"
fi
if [ -n "$shell_unchanged" ]; then
  printf '  Shell configuration: %s already contained the managed Trench block; left unchanged\n' "$shell_unchanged"
fi
if [ "$path_already_present" -eq 1 ]; then
  printf '  PATH: %s was already present; no PATH entry was added\n' "$canonical_install_dir"
fi
if [ "$path_precedence_added" -eq 1 ]; then
  printf '  PATH: %s was present but not first; configured Trench to take precedence\n' "$canonical_install_dir"
fi
if [ "$quarantine_removed" -eq 1 ]; then
  printf '  Quarantine: removed from %s\n' "$installed_executable"
fi
if [ "$modify_shell" -eq 0 ]; then
  printf '%s\n' '  Shell configuration: left unchanged (--no-modify-shell)'
else
  printf '  Shell reload: not performed by this process; run exec "%s" -l to apply changes\n' "${SHELL:-/bin/zsh}"
fi
