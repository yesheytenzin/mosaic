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

# Order matters: see the header of registrar_order.txt, and the definitions are
# looked up globally rather than in libandroid_runtime alone: some registrars live
# in libraries it merely needs, such as libhwui, which is where
# android.graphics.Typeface's natives are registered. startReg calls them by name
# and so does this.
#
# A registrar's mangled form is either a global function or one in the android
# namespace, so both candidates are emitted and the launcher takes whichever
# resolves.
: > "$here/registrars.inc"
while read -r name; do
  [ -n "$name" ] || continue
  case "$name" in \#*) continue ;; esac
  length=${#name}
  # Three forms, because the declaration decides: a plain name when the registrar
  # is extern "C" -- libhwui's register_android_graphics_classes is, and it is the
  # one that registers android.graphics.Typeface -- and the two C++ manglings
  # otherwise.
  printf '  "%s",\n' "$name" >> "$here/registrars.inc"
  printf '  "_Z%s%sP7_JNIEnv",\n' "$length" "$name" >> "$here/registrars.inc"
  printf '  "_ZN7android%s%sEP7_JNIEnv",\n' "$length" "$name" >> "$here/registrars.inc"
done < "$here/registrar_order.txt"

count=$(grep -c '"' "$here/registrars.inc" || true)
echo "generated $count registrar candidates, in AOSP order"

target="${MOSAIC_ANDROID_TARGET:-x86_64-linux-android21}"
clang --target="$target" -shared -fPIC -nostdlib -O2 \
  -Wall -Wextra -Wno-unused-parameter \
  -I"$here" \
  -o "$out/launcher.so" "$here/launcher.c"

echo "built $out/launcher.so"
