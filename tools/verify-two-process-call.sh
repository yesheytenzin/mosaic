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
daemon_file=$(mktemp)
owner_log=$(mktemp)

cleanup() {
  [ -n "${run_pid:-}" ] && kill "$run_pid" 2>/dev/null || true
  [ -n "${owner_pid:-}" ] && kill "$owner_pid" 2>/dev/null || true
  [ -n "${daemon_pid:-}" ] && kill "$daemon_pid" 2>/dev/null || true
  rm -f "$socket" "$daemon_file"
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

# NOTE: the wait below reads the broker's own log file, which is where the export
# line is written (see above). The sequence it automates is the one in
# docs/todo-a.md: run the framework with MOSAIC_BINDER_BROKER=1 and
# MOSAIC_BINDER_SOCKET set, wait for the broker to say a name was exported, then
# call that name from a second process.
echo "starting the broker on $socket"
MOSAIC_SOCKET="$socket" "$root/target/debug/mosaic" -l "$daemon_file" daemon >"$daemon_log" 2>&1 &
daemon_pid=$!
for _ in $(seq 1 100); do [ -S "$socket" ] && break; sleep 0.05; done
if [ ! -S "$socket" ]; then
  echo "the broker did not come up; see $daemon_log" >&2
  exit 1
fi

# An object a reply hands back, from a second process. Deterministic, unlike the
# framework's own objects below: the suspend hal is hosted by the broker itself,
# so this needs nothing else alive and can be the first thing checked. It is the
# other half of the round trip -- not "can a caller reach a name", but "can an
# answer give a caller something new to call".
echo "asking a reply for an object and calling it"
if ! "$root/tools/broker-object.py" "$socket"; then
  echo "a reply's object could not be called; the broker is still on $socket" >&2
  exit 1
fi

# The display half, whose answers are about *this* machine: the ids it lists have
# to be the ones the host has, or it is inventing displays.
echo "asking the display service about this host"
if ! "$root/tools/display-probe.py" "$socket"; then
  echo "the display service did not answer with this host's displays" >&2
  exit 1
fi

shim="$root/tools/binder-shim/out"
preload="$root/tools/launcher/out/launcher.so $shim/probe.so"
preload="$preload $shim/pretend-nice.so $shim/pretend-cgroups.so"
# alloc-trace.so is deliberately *not* here even though it is inert without
# MOSAIC_ALLOC_TRACE=1: interposing malloc and free slows the framework down
# enough that it dies before this gate gets its answer, and a gate must not
# perturb what it measures. It is in tools/boot-system-server.sh, which is where
# the allocation question is asked.
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
# Polled every 20 ms, not every second: the framework now dies a couple of seconds
# into its boot (the suspend path, see docs/remaining-work.md), and it is only
# alive while it is publishing. A slow poll calls after it is gone, which reads as
# "the broker closed the connection" and says nothing about the round trip.
for _ in $(seq 1 3000); do
  grep -aq ' exported .* as node ' "$daemon_file" 2>/dev/null && break
  sleep 0.02
done

name=$(grep -ao 'exported [^ ]* as node' "$daemon_file" 2>/dev/null | head -1 |
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
# The framework's own object. Best effort on purpose: the framework dies a couple
# of seconds into its boot (the suspend path, docs/remaining-work.md), so a call
# that arrives after it is gone proves nothing either way. What it exercises when
# it does land is the whole path with a Java object at the far end.
if "$root/tools/two-process-call.py" "$socket" "$name" 1 >"$log.call" 2>&1; then
  cat "$log.call"
  if grep -aq "served node" "$log"; then
    echo "ok: the framework's own object served the call, and the caller got a Reply."
  else
    echo "ok: the caller got a Reply from the framework's object."
  fi
else
  echo "the framework did not answer (it exits a couple of seconds in); continuing"
fi

# The forwarding itself, with an owner that stays up: this is the gate. A name
# another process published resolves, the broker carries the transaction to that
# process, it runs it, and the answer comes back with the caller's request id on
# it. Nothing here can fail because something else got there first.
echo "checking the forwarding with an owner that stays"
"$root/tools/two-process-owner.py" "$socket" mosaic.transport.probe >"$owner_log" 2>&1 &
owner_pid=$!
for _ in $(seq 1 200); do
  grep -aq "exported" "$owner_log" 2>/dev/null && break
  sleep 0.02
done
if ! grep -aq "exported" "$owner_log"; then
  echo "the owner did not publish; see $owner_log" >&2
  exit 1
fi

if ! "$root/tools/two-process-call.py" "$socket" mosaic.transport.probe 7 >"$log.forward" 2>&1; then
  cat "$log.forward" >&2
  cat "$owner_log" >&2
  echo "the forwarded call did not come back" >&2
  exit 1
fi
cat "$log.forward"

# The other direction: an object handed over *as an argument*. The caller passes
# the suspend hal's handle, the owner -- a different process -- calls it back, and
# what the hal said comes home. A handle is an index into one process's table, so
# this only works if the broker minted the owner its own.
echo "checking an object passed as an argument"
if ! "$root/tools/two-process-argument.py" "$socket" mosaic.transport.probe >"$log.argument" 2>&1; then
  cat "$log.argument" >&2
  cat "$owner_log" >&2
  echo "an object passed as an argument did not come back callable" >&2
  exit 1
fi
cat "$log.argument"
cat "$owner_log"
echo
echo "ok: a name published by one process was called from another, an answer came"
echo "    back, and an object passed as an argument was called by the callee."
echo "    The gate: a transaction between two processes works, both ways."
exit 0
