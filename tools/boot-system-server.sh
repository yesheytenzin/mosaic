#!/bin/bash
# Boot com.android.server.SystemServer on a runtime bundle and summarise what
# happened, so a run is one command and its evidence is in one place.
#
#   tools/boot-system-server.sh [bundle] [logfile]
#
# This is the system of record for "how far does the framework get": every wall
# recorded in docs/remaining-work.md was found by running this and reading the
# `SystemServerTiming:` markers, the fatal signals, and the shim's own
# diagnostics. A crash here leaves a core (coredumpctl list), which is how the
# crash address is recovered for the allocation trace below.
#
# Two traces exist for the questions logs cannot answer:
#
#   MOSAIC_BINDER_TRACE=1  every service-manager call and transaction, raw
#                          syscalls, to /tmp/mosaic-sm-trace.log
#   MOSAIC_ALLOC_TRACE=1   malloc/free of small blocks, to
#                          /tmp/mosaic-alloc-trace.log, for correlating a crash
#                          address with the allocation that reused it
#                          (MOSAIC_ALLOC_TRACE_MIN/MAX widen the size window)

set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
bundle=${1:-$HOME/.local/share/mosaic/bundle}
log=${2:-/tmp/mosaic-boot.log}
socket=${MOSAIC_SOCKET:-/tmp/mosaic-broker.sock}
broker_log=/tmp/mosaic-boot-broker.log

if [ ! -x "$bundle/run.sh" ]; then
  echo "$bundle does not look like a runtime bundle (no run.sh)" >&2
  echo "build one with: tools/bundle/bundle.sh build /var/lib/mosaic/images/system.img $bundle" >&2
  exit 2
fi

# The preload list, in order and explicit: the launcher first (it interposes
# JNI_CreateJavaVM), then the shim's pieces. A new one is a one-line change here,
# reviewable in the history, rather than an edit to a scratch script.
shim="$root/tools/binder-shim/out"
launcher="$root/tools/launcher/out/launcher.so"
preload="$launcher $shim/probe.so $shim/pretend-nice.so $shim/pretend-cgroups.so"
preload="$preload $shim/android-binder.so $shim/android-properties.so $shim/alloc-trace.so"

pkill -f "mosaic.*daemon" 2>/dev/null
rm -f "$socket" "$broker_log"
MOSAIC_SOCKET="$socket" "$root/target/debug/mosaic" -l "$broker_log" -w /tmp/mosaic-bootwork \
  daemon >/dev/null 2>&1 &
broker=$!
for _ in $(seq 1 100); do [ -S "$socket" ] && break; sleep 0.05; done
if [ ! -S "$socket" ]; then
  echo "the broker did not come up; see $broker_log" >&2
  exit 1
fi

cd "$bundle" || exit 1
MOSAIC_BINDER_BROKER=1 MOSAIC_BINDER_SOCKET="$socket" \
MOSAIC_ANDROID_ROOT=$PWD MOSAIC_PROPERTY_DIR=$PWD/properties \
MOSAIC_TIMEOUT=${MOSAIC_TIMEOUT:-180} MOSAIC_MAX_OUTPUT=${MOSAIC_MAX_OUTPUT:-6000000} \
MOSAIC_PRELOAD="$preload" \
MOSAIC_LAUNCH_CLASS=com.android.server.SystemServer \
MOSAIC_LAUNCH_RUNTIME=$PWD/lib64/libandroid_runtime.so \
"$root/tools/bundle/with-logd.sh" "$PWD/run.sh" dalvikvm64 \
  -Xbootclasspath:"$(cat bootclasspath.txt)" -cp "$(cat systemserverclasspath.txt)" \
  > "$log" 2>&1
status=$?
kill $broker 2>/dev/null

clean=$(tr -d '\000' < "$log")
echo "exit=$status  lines=$(echo "$clean" | wc -l)  log=$log"
echo "--- last boot stages ---"
echo "$clean" | grep -a "SystemServerTiming: Start" | tail -6
echo "--- fatal ---"
echo "$clean" | grep -a "Assertion failed\|Fatal signal\|SIGSEGV\|Aborting\|ServiceNotFoundException\|Failed to get" | head -8
echo "--- binder diagnostics ---"
for pattern in "BAD_TYPE" "NOT_ENOUGH_DATA" "BAD COMMAND" "no longer looks like an object" \
               "served node" "sent a transaction" "handing back .* as a handle" \
               "answered code .* empty reply" "forwarded code" "dropped an answer"; do
  printf "%3s  %s\n" "$(echo "$clean" | grep -ac "$pattern")" "$pattern"
done
exit $status
