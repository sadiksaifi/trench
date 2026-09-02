#!/bin/sh
set -eu

usage() {
  echo "usage: package-release.sh <target> <trench-binary> <output-directory>" >&2
  exit 2
}

test "$#" -eq 3 || usage
target=$1
binary=$2
output_dir=$3

case "$target" in
  aarch64-apple-darwin | x86_64-apple-darwin) ;;
  *)
    echo "unsupported release target: $target" >&2
    exit 2
    ;;
esac

test -n "${TRENCH_RELEASE_VERSION:-}" || {
  echo "TRENCH_RELEASE_VERSION is required" >&2
  exit 2
}
test -x "$binary" || {
  echo "release binary is not executable: $binary" >&2
  exit 1
}

reported_version=$($binary --version)
test "$reported_version" = "trench $TRENCH_RELEASE_VERSION" || {
  echo "release binary reported '$reported_version', expected 'trench $TRENCH_RELEASE_VERSION'" >&2
  exit 1
}

command -v otool >/dev/null 2>&1 || {
  echo "otool is required to verify macOS release portability" >&2
  exit 1
}
dependencies=$(otool -L "$binary" | tail -n +2)
for forbidden in /opt/homebrew /usr/local "${GITHUB_WORKSPACE:-}" "${RUNNER_TEMP:-}"; do
  test -n "$forbidden" || continue
  if printf '%s\n' "$dependencies" | grep -F "$forbidden" >/dev/null; then
    echo "forbidden build-machine path in release binary: $forbidden" >&2
    exit 1
  fi
done
if printf '%s\n' "$dependencies" | awk '{ print $1 }' | grep -Ev '^(/usr/lib/|/System/Library/)' >/dev/null; then
  echo "release binary links a non-system dynamic library" >&2
  printf '%s\n' "$dependencies" >&2
  exit 1
fi

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repository_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/trench-package.XXXXXX")
trap 'rm -rf "$stage"' EXIT HUP INT TERM

install -m 0755 "$binary" "$stage/trench"
install -m 0644 "$repository_root/LICENSE" "$repository_root/README.md" "$stage/"
mkdir -p "$output_dir"
archive="$output_dir/trench-$target.tar.gz"
tar -C "$stage" -czf "$archive.tmp" trench LICENSE README.md
mv "$archive.tmp" "$archive"
printf '%s\n' "$archive"
