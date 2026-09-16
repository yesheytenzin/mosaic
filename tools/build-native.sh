#!/bin/bash
# Build every Bionic artifact this project produces.
#
#   ./build-native.sh <bundle>
#
# The artifacts are the tools' own build outputs, and they are gitignored, so a
# `rm -rf */out` before committing silently deletes them. That happened: a
# measurement was taken with a missing launcher, the run failed to link, and the
# result looked like progress had stopped for a reason that did not exist. One
# command that builds all of them is the guard.
#
# Everything here targets Android with plain clang: no NDK, and -nostdlib because
# a shared object may leave its libc symbols for the Android linker to resolve.

set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
bundle="${1:?usage: build-native.sh <bundle>}"

"$here/binder-shim/build.sh"
"$here/launcher/build.sh" "$bundle"

echo
echo "built:"
ls -1 "$here/binder-shim/out" "$here/launcher/out" 2>/dev/null | sed 's/^/  /'
