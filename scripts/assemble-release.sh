#!/bin/sh
set -eu

usage() {
  echo "usage: assemble-release.sh <dist> <tag> <version> <commit> <installer>" >&2
  exit 2
}

test "$#" -eq 5 || usage
dist=$1
tag=$2
version=$3
commit=$4
installer=$5

case "$tag" in
  "v$version") ;;
  *)
    echo "release tag and version do not match: $tag / $version" >&2
    exit 2
    ;;
esac
printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' || {
  echo "release version is not canonical stable SemVer: $version" >&2
  exit 2
}
printf '%s\n' "$commit" | grep -Eq '^[0-9a-fA-F]{40}$' || {
  echo "release commit must be a full 40-character Git hash" >&2
  exit 2
}

digest_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  else
    shasum -a 256 "$1" | awk '{ print $1 }'
  fi
}

arm_archive=trench-aarch64-apple-darwin.tar.gz
intel_archive=trench-x86_64-apple-darwin.tar.gz
installer_name=trench-installer.sh
manifest_name=trench-release.json
checksums_name=trench-checksums.txt

test -f "$dist/$arm_archive" || {
  echo "missing $arm_archive" >&2
  exit 1
}
test -f "$dist/$intel_archive" || {
  echo "missing $intel_archive" >&2
  exit 1
}
test -f "$installer" || {
  echo "missing installer: $installer" >&2
  exit 1
}
install -m 0755 "$installer" "$dist/$installer_name"

assets=$({
  for name in "$arm_archive" "$intel_archive" "$installer_name"; do
    digest=$(digest_file "$dist/$name")
    jq -cn --arg name "$name" --arg sha256 "$digest" '{name: $name, sha256: $sha256}'
  done
} | jq -s '.')

jq -n \
  --arg tag "$tag" \
  --arg version "$version" \
  --arg commit "$(printf '%s' "$commit" | tr 'A-F' 'a-f')" \
  --argjson assets "$assets" \
  '{schema: 1, tag: $tag, version: $version, commit: $commit, assets: $assets}' \
  >"$dist/$manifest_name.tmp"
mv "$dist/$manifest_name.tmp" "$dist/$manifest_name"

checksums_tmp="$dist/$checksums_name.tmp"
: >"$checksums_tmp"
for name in "$arm_archive" "$intel_archive" "$installer_name" "$manifest_name"; do
  printf '%s  %s\n' "$(digest_file "$dist/$name")" "$name" >>"$checksums_tmp"
done
LC_ALL=C sort -k2 "$checksums_tmp" >"$dist/$checksums_name"
rm -f "$checksums_tmp"

for name in "$arm_archive" "$intel_archive" "$installer_name" "$manifest_name"; do
  expected=$(awk -v name="$name" '$2 == name { print $1 }' "$dist/$checksums_name")
  test -n "$expected" && test "$(digest_file "$dist/$name")" = "$expected" || {
    echo "checksum verification failed while assembling $name" >&2
    exit 1
  }
done
