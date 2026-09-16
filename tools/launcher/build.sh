#!/bin/bash
# Build the launcher, which runs a class with the framework's JNI natives
# registered. See launcher.c for why it exists and why it is a shared object.
#
#   ./build.sh <bundle> [output-dir]
#
# The registrar list is generated from the bundle's libandroid_runtime.so rather
# than hardcoded, so it follows whichever image the bundle came from.

set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
bundle="${1:?usage: build.sh <bundle> [output-dir]}"
out="${2:-$here/out}"
runtime="$bundle/lib64/libandroid_runtime.so"

if [ ! -f "$runtime" ]; then
  echo "no $runtime; is this a Mosaic runtime bundle?" >&2
  exit 1
fi

mkdir -p "$out"

# The registrars AndroidRuntime::startReg calls, in mangled form, which is what
# dlsym needs. Both the free functions and the ones in the android namespace
# match: they are the register_* names in the export table.
readelf --dyn-syms -W "$runtime" \
  | awk '$7 != "UND" && $4 == "FUNC" { print $8 }' \
  | grep -E '^_Z(N7android)?[0-9]+register_' \
  | sort -u \
  | sed 's/^/  "/; s/$/",/' > "$here/registrars.inc"

count=$(grep -c '"' "$here/registrars.inc" || true)
echo "generated $count registrars from $(basename "$runtime")"

target="${MOSAIC_ANDROID_TARGET:-x86_64-linux-android21}"
clang --target="$target" -shared -fPIC -nostdlib -O2 \
  -Wall -Wextra -Wno-unused-parameter \
  -I"$here" \
  -o "$out/launcher.so" "$here/launcher.c"

echo "built $out/launcher.so"
