#!/bin/bash
# Build the Bionic shared objects in this directory.
#
# No NDK is needed. clang can target Android directly, and a shared object may
# leave its libc symbols undefined for the Android linker to resolve when it
# loads the library, so -nostdlib is correct rather than a compromise. The
# sources declare the few libc functions they use and reach the kernel through
# syscall(), which keeps them free of headers.
#
#   ./build.sh <output-dir>
#
# The result is loaded into an app process with LD_PRELOAD; the bundle's run.sh
# applies MOSAIC_PRELOAD to the Bionic process and not to the shell around it.

set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
out="${1:-$here/out}"
mkdir -p "$out"

target="${MOSAIC_ANDROID_TARGET:-x86_64-linux-android21}"

for source in "$here"/*.c; do
  [ -e "$source" ] || continue
  name=$(basename "$source" .c)
  # -fno-emulated-tls: the default for Android is emulated thread-local storage,
  # which needs a helper Bionic does not export. The real thing is available and
  # is what a preload should use.
  clang --target="$target" -shared -fPIC -nostdlib -O2 -fno-emulated-tls \
    -Wall -Wextra -Wno-unused-parameter \
    -o "$out/$name.so" "$source"
  echo "built $out/$name.so"
done
