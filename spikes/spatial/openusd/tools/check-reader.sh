#!/bin/sh
# Run on the destination Mac, directly from an unpacked reader distribution.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rmdir "$tmp"' EXIT
check() {
    result=$(/usr/bin/env -i PATH=/usr/bin:/bin HOME="$tmp" TMPDIR="$tmp" "$root/bin/inspect_stage" "$root/checks/$1" "$2")
    if ! printf '%s\n' "$result" | /usr/bin/grep -Fxq "$3"; then
        printf '%s\n' "$result" >&2
        exit 1
    fi
    printf '%s\n' "$result"
}
check hinge.usda /Rig '1 triangles, 3 rigid parts, 1 material slots, 0 images'
check seam.usdz /Card '2 triangles, 1 rigid parts, 1 material slots, 1 images'
