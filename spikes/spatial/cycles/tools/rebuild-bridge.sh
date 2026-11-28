#!/bin/bash
# After editing native/, rebuild only the bridge library and place it in the provisioned bundle.
# CYCLES_BUILD names a configured Cycles build tree (default .local/cycles/cycles-build).
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
BUILD="${CYCLES_BUILD:-$ROOT/.local/cycles/cycles-build}"
OUT="$ROOT/.local/cycles/install/libn4cycles.dylib"
cmake -S "$(grep -m1 '^CMAKE_HOME_DIRECTORY' "$BUILD/CMakeCache.txt" | cut -d= -f2)" -B "$BUILD" -DN4_BRIDGE_DIR="$HERE/native" >/dev/null
cmake --build "$BUILD" --target n4cycles
cp "$BUILD/lib/libn4cycles.dylib" "$OUT"
# The build tree's library points at the precompiled dependencies; the bundle carries its own.
for path in $(otool -l "$OUT" | awk '/LC_RPATH/{getline; getline; print $2}'); do
  install_name_tool -delete_rpath "$path" "$OUT"
done
install_name_tool -add_rpath @loader_path/lib "$OUT"
codesign -f -s - "$OUT"
echo "rebuilt $OUT"
