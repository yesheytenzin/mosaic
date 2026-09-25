#!/bin/bash
# Verify every gate in section A of docs/remaining-work.md.
#
#   tools/verify-a.sh <runtime-bundle>
#
# The system-server smoke is allowed to stop later in B. This gate checks the A
# milestones and A-specific failure classes; it does not turn a later service
# crash into a false A failure. The A2/A4/A5 and A6 checks are separate because
# they need a live broker and the installed user-manager grant respectively.

set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bundle=${1:-}

if [ -z "$bundle" ] || [ ! -x "$bundle/run.sh" ]; then
  echo "usage: tools/verify-a.sh <runtime-bundle>" >&2
  echo "the bundle must contain an executable run.sh" >&2
  exit 2
fi

say() { printf '%s\n' "$*"; }
fail() { printf 'A gate failed: %s\n' "$*" >&2; exit 1; }
require() {
  local text=$1
  grep -aF "$text" "$log" >/dev/null || fail "missing runtime marker: $text"
}

say "1. Rust implementation and unit gates"
cargo fmt --check
cargo clippy -- -D warnings
cargo test

say "2. Native A shims and launcher"
"$root/tools/binder-shim/build.sh" >/tmp/mosaic-verify-a-shim.log 2>&1 || {
  cat /tmp/mosaic-verify-a-shim.log >&2
  exit 1
}
if grep -q 'warning:' /tmp/mosaic-verify-a-shim.log; then
  cat /tmp/mosaic-verify-a-shim.log >&2
  fail "binder shim build emitted a warning"
fi
"$root/tools/launcher/build.sh" "$bundle" >/tmp/mosaic-verify-a-launcher.log 2>&1 || {
  cat /tmp/mosaic-verify-a-launcher.log >&2
  exit 1
}
if grep -q 'warning:' /tmp/mosaic-verify-a-launcher.log; then
  cat /tmp/mosaic-verify-a-launcher.log >&2
  fail "launcher build emitted a warning"
fi

say "3. A2/A4/A5 and display-half live gates"
"$root/tools/verify-two-process-call.sh" "$bundle"

say "4. A6 installed priority gate"
"$root/tools/verify-priority-limit.sh" "$bundle"

say "5. A1/A2/A7/A8 system-server smoke"
log=$(mktemp)
trap 'rm -f "$log"' EXIT
set +e
MOSAIC_BINDER_DEBUG=1 "$root/tools/boot-system-server.sh" "$bundle" "$log" >/tmp/mosaic-verify-a-system-server.log 2>&1
boot_status=$?
set -e
cat /tmp/mosaic-verify-a-system-server.log

# These are the A-path milestones. Reaching them proves the font map, the
# hosted registry, the path redirection, and the generated property area are
# being used by the real framework.
require "SystemServerTiming: StartActivityManager"
require "SystemServerTiming: StartDisplayManager"
require "SystemServerTiming: StartPackageManagerService"
require "android-binder: published display"
require "android-binder: registered display"
require "DisplayDeviceRepository: Display device added"
require "property-service: listening"
require "android-properties: set"
require "property-service: set"

# These failures block an A milestone. `Can't find display mode with id -1`
# is not one of them: the framework immediately falls back to the active mode
# and `DisplayDeviceRepository: Display device added` below proves the display
# milestone completed.
if grep -aE 'PMS compiler filter settings are bad|Invalid path: /data|UnsatisfiedLinkError|Non-system caller|Failed to initialize display event receiver|No valid static info found' "$log" >/dev/null; then
  grep -aE 'PMS compiler filter settings are bad|Invalid path: /data|UnsatisfiedLinkError|Non-system caller|Failed to initialize display event receiver|No valid static info found' "$log" >&2
  fail "an A-specific system-server failure remains"
fi

if [ "$boot_status" -ne 0 ]; then
  say "A milestones passed; system-server stopped later in B (exit $boot_status)."
  say "The A gate is complete; the later B failure is recorded in the system-server log."
else
  say "A milestones passed; system-server exited cleanly."
fi

say "A verification passed."
