#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  echo 'usage: release-make-latest.sh <candidate-tag> <current-latest-tag>' >&2
  exit 2
fi

candidate=$1
current=$2
canonical_tag='^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'

printf '%s\n' "$candidate" | grep -Eq "$canonical_tag" || {
  echo "candidate release tag is not canonical: $candidate" >&2
  exit 2
}
printf '%s\n' "$current" | grep -Eq "$canonical_tag" || {
  echo "current latest release tag is not canonical: $current" >&2
  exit 2
}

awk -v candidate="${candidate#v}" -v current="${current#v}" '
    BEGIN {
        split(candidate, candidate_parts, ".")
        split(current, current_parts, ".")
        for (component = 1; component <= 3; component += 1) {
            if (length(candidate_parts[component]) > length(current_parts[component])) {
                print "true"
                exit
            }
            if (length(candidate_parts[component]) < length(current_parts[component])) {
                print "false"
                exit
            }
            if ("x" candidate_parts[component] > "x" current_parts[component]) {
                print "true"
                exit
            }
            if ("x" candidate_parts[component] < "x" current_parts[component]) {
                print "false"
                exit
            }
        }
        print "false"
    }
'
