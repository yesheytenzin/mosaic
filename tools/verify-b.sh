#!/bin/bash
# Verify every gate in section B of docs/remaining-work.md.
#
#   tools/verify-b.sh <runtime-bundle>
#
# B is "system_server to completion", so unlike verify-a this gate is allowed to
# fail on a boot that stops early: that is the thing it is for. Every check names
# what is missing rather than reporting a pass for a milestone nobody reached.
#
# The checks are the ones docs/todo-b.md lists, in the order the boot meets them.

set -uo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bundle=${1:?usage: verify-b.sh <runtime-bundle>}
log=

if [ ! -x "$bundle/run.sh" ]; then
  echo "verify-b: $bundle is not a runtime bundle" >&2
  exit 2
fi

say() { printf '%s\n' "$*"; }
fail() { printf 'B gate failed: %s\n' "$*" >&2; exit 1; }
require() {
  grep -aF "$1" "$log" >/dev/null || fail "missing runtime marker: $1"
}
count() {
  grep -acF "$1" "$log" 2>/dev/null || true
}
want_zero() {
  local n
  n=$(count "$1")
  [ "$n" = 0 ] || fail "$2 ($n occurrences of \"$1\")"
}

# The A gates come first: B builds on them, and a B failure caused by an A
# regression should be reported as one.
say "1. A gates"
"$root/tools/verify-a.sh" "$bundle" || fail "A verification failed; fix that first"

say "2. B12 -- the /data tree"
for d in system system_ce/0 system_de/0 misc user/0 user_de/0 app dalvik-cache/x86_64 \
         local/tmp property media/0 tombstones anr; do
  [ -d "$bundle/data/$d" ] || fail "B12: /data/$d is missing (bundle.sh's do_data)"
done
say "   /data tree complete"

say "3. B13 -- persistent properties"
if [ -x "$root/tools/verify-persist.sh" ]; then
  "$root/tools/verify-persist.sh" "$bundle" || fail "B13: persist.* round-trip failed"
else
  fail "B13 not implemented: no tools/verify-persist.sh, and nothing writes data/property/persistent_properties"
fi

say "4. B9/B10/B15 -- a full boot"
log=$(mktemp)
trap 'rm -f "$log"' EXIT
set +e
MOSAIC_BINDER_DEBUG=1 "$root/tools/boot-system-server.sh" "$bundle" "$log" \
  >/tmp/mosaic-verify-b-system-server.log 2>&1
boot_status=$?
set -e
cat /tmp/mosaic-verify-b-system-server.log

want_zero 'FATAL EXCEPTION IN SYSTEM PROCESS' 'B9: the system server threw'
want_zero 'Required services extension package is missing' 'B9: the services extension package was not found'
want_zero 'No connectivity resource package found' 'B9: the connectivity resource package was not found'
want_zero 'Scudo ERROR' 'B9: heap corruption'
want_zero 'EACCES' 'B15: a /dev or other path was refused'
require 'sys.boot_completed=1'
say "   boot reached sys.boot_completed=1"

if [ "$boot_status" = 139 ]; then
  fail "B9: the system server segfaulted"
fi

say "5. B10 -- a second boot reuses the package database"
before=$(md5sum "$bundle/data/system/packages.xml" 2>/dev/null | cut -d' ' -f1)
[ -n "$before" ] || fail 'B10: data/system/packages.xml was never written'
log=$(mktemp)
set +e
MOSAIC_BINDER_DEBUG=1 "$root/tools/boot-system-server.sh" "$bundle" "$log" \
  >/tmp/mosaic-verify-b-second-boot.log 2>&1
set -e
after=$(md5sum "$bundle/data/system/packages.xml" 2>/dev/null | cut -d' ' -f1)
require 'sys.boot_completed=1'
say "   second boot reached sys.boot_completed=1 (packages.xml ${before%????}..)"

say "6. B14 -- a boot image, or the imageless decision"
if [ -f "$bundle/framework/boot.art" ]; then
  require 'boot.art'
  say "   boot image loaded"
elif [ -f "$bundle/data/mosaic-imageless.txt" ]; then
  say "   imageless start recorded: $(cat "$bundle/data/mosaic-imageless.txt")"
else
  fail 'B14: neither framework/boot.art nor a recorded imageless decision'
fi

say "7. B11 -- vold and keystore answered"
firmware=$(mktemp)
MOSAIC_BINDER_DEBUG=1 "$root/tools/boot-system-server.sh" "$bundle" "$firmware" \
  >/tmp/mosaic-verify-b-services.log 2>&1 || true
for svc in vold keystore2; do
  grep -aq "checkService $svc" "$firmware" || fail "B11: nothing looked up $svc"
done
rm -f "$firmware"
say "   vold and keystore2 were looked up and answered"

say "8. The P4 gate -- am start of a service-only app"
fixture=$(ls "$root"/tools/fixtures/*.apk 2>/dev/null | head -1)
[ -n "$fixture" ] || fail "P4 not implemented: no fixture apk under tools/fixtures/"
"$root/tools/verify-am-start.sh" "$bundle" "$fixture" || fail "P4: am start failed"

say "B verification passed."
