#!/bin/bash
# Verify A5's gate: a transaction between two processes, with one of them the
# framework.
#
#   tools/verify-two-process-call.sh [bundle]
#
# The bundle defaults to /tmp/opencode/phase3; build one with
#
#   tools/bundle/bundle.sh build /var/lib/mosaic/images/system.img /tmp/opencode/phase3
#
# What it does, and why this is the gate rather than a unit test:
#
#   1. starts the broker (`mosaic daemon`) on a private socket
#   2. runs the framework with the shim's broker client on, so the services it
#      registers are published to the broker
#   3. from a *second* process, looks one of those names up and calls it
#   4. checks that the call came back as a Reply, and that the process that owns
#      the object logged that it served it
#
# The interesting part is that neither process knows the other: the name is
# resolved by the broker, the call is carried to the owner, and the answer comes
# back. That is ADR-0004's userspace binder doing what the kernel driver does on a
# device.
#
# No root, no bundle changes, nothing installed.

set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
bundle=${1:-/tmp/opencode/phase3}

if [ ! -x "$bundle/run.sh" ]; then
  echo "$bundle does not look like a runtime bundle (no run.sh)" >&2
  echo "build one with: tools/bundle/bundle.sh build /var/lib/mosaic/images/system.img $bundle" >&2
  exit 2
fi

socket=$(mktemp -u /tmp/mosaic-verify-XXXXXX.sock)
log=$(mktemp)
daemon_log=$(mktemp)

cleanup() {
  [ -n "${run_pid:-}" ] && kill "$run_pid" 2>/dev/null || true
  [ -n "${daemon_pid:-}" ] && kill "$daemon_pid" 2>/dev/null || true
  rm -f "$socket"
}
trap cleanup EXIT

echo "building the daemon and the native artifacts..."
(cd "$root" && cargo build --quiet)
# build-native.sh fills in what the bundle needs; the shim itself is built by its
# own script, and it has to be *rebuilt* rather than merely present. A stale
# build is what made this script's first run report that the framework published
# nothing: the preload was an older shim whose broker client still defaulted off.
MOSAIC_ANDROID_ROOT="$bundle" "$root/tools/build-native.sh" "$bundle" >/dev/null 2>&1 || true
"$root/tools/binder-shim/build.sh" >/dev/null
"$root/tools/launcher/build.sh" >/dev/null 2>&1 || true

# NOTE: the automatic form of this is not reliable yet. The framework's
# registrations do not always reach the broker inside the window this waits for,
# and when they do not, this reports that nothing was published. The sequence that
# is verified by hand is in docs/todo-a.md: run the framework with
# MOSAIC_BINDER_BROKER=1 and MOSAIC_BINDER_SOCKET set, wait for the broker's log to
# say a name was exported, then run tools/two-process-call.py against it.
echo "starting the broker on $socket"
MOSAIC_SOCKET="$socket" "$root/target/debug/mosaic" daemon >"$daemon_log" 2>&1 &
daemon_pid=$!
for _ in $(seq 1 100); do [ -S "$socket" ] && break; sleep 0.05; done
if [ ! -S "$socket" ]; then
  echo "the broker did not come up; see $daemon_log" >&2
  exit 1
fi

shim="$root/tools/binder-shim/out"
preload="$root/tools/launcher/out/launcher.so $shim/probe.so $shim/pretend-nice.so"
preload="$preload $shim/android-binder.so $shim/android-properties.so"

echo "running the framework, with the broker client on"
(
  cd "$bundle"
  MOSAIC_TIMEOUT=${MOSAIC_TIMEOUT:-150} MOSAIC_MAX_OUTPUT=900000 \
  MOSAIC_ANDROID_ROOT="$bundle" MOSAIC_PROPERTY_DIR="$bundle/properties" \
  MOSAIC_BINDER_BROKER=1 MOSAIC_BINDER_SOCKET="$socket" \
  MOSAIC_PRELOAD="$preload" \
  MOSAIC_LAUNCH_CLASS=com.android.server.SystemServer \
  MOSAIC_LAUNCH_RUNTIME="$bundle/lib64/libandroid_runtime.so" \
  "$root/tools/bundle/with-logd.sh" "$bundle/run.sh" dalvikvm64 \
    -Xbootclasspath:"$(cat bootclasspath.txt)" -cp "$(cat systemserverclasspath.txt)"
) >"$log" 2>&1 &
run_pid=$!

# Wait for the framework to publish something, and take the name from the broker
# rather than from the framework's own log: the broker is the one place both
# processes look, and its line is not interleaved with anything else. The
# framework takes about forty seconds to get that far, and how long moves with the
# machine, so the wait is generous.
for _ in $(seq 1 150); do
  grep -aq ' exported .* as node ' "$daemon_log" 2>/dev/null && break
  sleep 1
done

name=$(grep -ao 'exported [^ ]* as node' "$daemon_log" 2>/dev/null | head -1 |
  sed 's/^exported //; s/ as node$//' || true)
if [ -z "$name" ]; then
  echo "the framework published nothing to the broker." >&2
  echo "  framework log: $log" >&2
  echo "  broker log:    $daemon_log" >&2
  tail -3 "$log" >&2
  exit 1
fi
echo "the broker holds: $name"

echo "calling it from a second process"
if "$root/tools/two-process-call.py" "$socket" "$name" 1 >"$log.call" 2>&1; then
  cat "$log.call"
  if grep -aq "served node" "$log"; then
    echo
    echo "ok: the owner logged that it served the call, and the caller got a Reply."
    echo "    A5's gate: a transaction between two processes works."
    exit 0
  fi
  echo "the caller got an answer but the owner did not log serving it" >&2
  exit 1
fi

cat "$log.call" >&2
echo "the call did not come back; the framework's log is $log" >&2
exit 1
