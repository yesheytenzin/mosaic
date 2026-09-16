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

# All exported registrars, mangled, which is what dlsym needs: both the free
# functions and the ones in the android namespace match.
readelf --dyn-syms -W "$runtime" \
  | awk '$7 != "UND" && $4 == "FUNC" { print $8 }' \
  | grep -E '^_Z(N7android)?[0-9]+register_' \
  | sort -u > "$out/mangled.txt"

# Order matters: see the header of registrar_order.txt. The mangled name embeds
# the plain one between a length prefix and the parameter list, so matching is a
# substring test rather than a demangle -- c++filt is not on every machine, and
# its absence silently degrades this to alphabetical order, which is exactly the
# bug this file exists to prevent.
: > "$here/registrars.inc"
while read -r name; do
  [ -n "$name" ] || continue
  case "$name" in \#*) continue ;; esac
  symbol=$(grep -m1 -E "${name}E?P7_JNIEnv$" "$out/mangled.txt" || true)
  [ -n "$symbol" ] && printf '  "%s",\n' "$symbol" >> "$here/registrars.inc"
done < "$here/registrar_order.txt"

# Anything the bundle exports that AOSP's list does not mention goes last, where
# it cannot disturb the ordering that matters.
while read -r mangled; do
  grep -qF "\"$mangled\"" "$here/registrars.inc" || printf '  "%s",\n' "$mangled" >> "$here/registrars.inc"
done < "$out/mangled.txt"

count=$(grep -c '"' "$here/registrars.inc" || true)
echo "generated $count registrars from $(basename "$runtime"), in AOSP order"

target="${MOSAIC_ANDROID_TARGET:-x86_64-linux-android21}"
clang --target="$target" -shared -fPIC -nostdlib -O2 \
  -Wall -Wextra -Wno-unused-parameter \
  -I"$here" \
  -o "$out/launcher.so" "$here/launcher.c"

echo "built $out/launcher.so"
