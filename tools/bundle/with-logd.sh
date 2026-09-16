#!/bin/bash
# Run a Bionic process with a working logd, so its LOG(...) output is readable
# instead of silently dropped.
#
#   with-logd.sh <command> [args...]
#
# liblog looks for logd at /dev/socket/logdw, and creating /dev/socket needs
# root on the host. `unshare -rm` gives us a private /dev without touching the
# real one. Device nodes cannot be carried across (a user namespace may not
# bind-mount devtmpfs or create device nodes), and processes run fine without
# the ones they do not strictly need.
#
# This is a debugging aid. The product runs app processes in the host's own
# namespace (ADR-0001); it does not wrap them in one.

set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
logd="$here/android-logd.py"
properties="$here/android-property-service.py"

if [ $# -lt 1 ]; then
  sed -n '2,14p' "$0"
  exit 2
fi

# A preload library that is not there makes the process fail to link, which looks
# like the program stopping for its own reasons. That cost two wrong readings, so
# it is checked here rather than noticed later.
if [ -n "${MOSAIC_PRELOAD:-}" ]; then
  for library in $MOSAIC_PRELOAD; do
    if [ ! -f "$library" ]; then
      echo "with-logd: MOSAIC_PRELOAD names $library, which does not exist." >&2
      echo "           Build the native artifacts first: tools/build-native.sh <bundle>" >&2
      exit 2
    fi
  done
fi

exec unshare -rm --propagation private bash -c '
  set -uo pipefail
  mount -t tmpfs none /dev || exit 1
  mkdir -p /dev/socket || exit 1

  # Android has a filesystem layout and the framework refers to it by absolute
  # path in places that cannot be configured -- AssetManager inlines
  # "/system/framework/framework-res.apk" as a compile-time constant. Presenting
  # those paths with a mount namespace needs root, and a user namespace cannot
  # even create /system, so the harness redirects the opens instead (see
  # tools/binder-shim/android-paths.c). A product can do either; the plan says so.
  if [ -n "${MOSAIC_ANDROID_ROOT:-}" ]; then
    echo "android root: $MOSAIC_ANDROID_ROOT, paths redirected by android-paths.so" >&2
  fi

  # The Android framework raises its own thread priorities (Process
  # .setThreadPriority, which wants a negative nice), and an unprivileged process
  # may only do that within RLIMIT_NICE. On a desktop that limit is 0, so the
  # framework throws SecurityException; systemd can raise it per unit with
  # LimitNICE, and this is the same thing for the harness.
  ulimit -e 40 2>/dev/null || true

  # A Bionic process reads system properties from /dev/__properties__, which
  # libc hardcodes, and writes them over a socket at /dev/socket/property_service.
  # Point MOSAIC_PROPERTY_DIR at a directory built by make-property-area.py to
  # have both appear. Inside the namespace we are uid 0, which is what libc
  # requires of the mapped files.
  if [ -n "${MOSAIC_PROPERTY_DIR:-}" ]; then
    mkdir -p /dev/__properties__
    cp -f "$MOSAIC_PROPERTY_DIR"/* /dev/__properties__/ || exit 1
    chown 0:0 /dev/__properties__/* || exit 1
    chmod 0644 /dev/__properties__/*
    echo "properties: $(ls /dev/__properties__ | wc -l) files in /dev/__properties__" >&2

    ANDROID_PROPERTY_DIR=/dev/__properties__ python3 "$2" >&2 &
    properties_pid=$!
    for _ in $(seq 1 200); do
      [ -S /dev/socket/property_service ] && break
      sleep 0.02
    done
  fi

  python3 "$1" >&2 &
  logd_pid=$!
  for _ in $(seq 1 200); do
    [ -S /dev/socket/logdw ] && break
    sleep 0.02
  done

  shift 2
  # A process that spins on a malformed driver reply fills a disk with log
  # output, so runs are bounded and their output is capped.
  timeout "${MOSAIC_TIMEOUT:-120}" "$@"
  status=$?
  if [ $status -eq 124 ]; then
    echo "with-logd: timed out after ${MOSAIC_TIMEOUT:-120}s" >&2
  fi

  kill "$logd_pid" 2>/dev/null || true
  [ -n "${properties_pid:-}" ] && kill "$properties_pid" 2>/dev/null
  exit "$status"
' _ "$logd" "$properties" "$@"
