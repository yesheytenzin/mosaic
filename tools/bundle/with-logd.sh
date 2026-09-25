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
  missing_preload=0
  for library in $MOSAIC_PRELOAD; do
    [ -f "$library" ] || missing_preload=1
  done
  # Build them rather than refusing to start: the artifacts are gitignored, so
  # cleaning the tree deletes them, and three runs were wasted discovering that.
  if [ "$missing_preload" = 1 ] && [ -n "${MOSAIC_ANDROID_ROOT:-}" ]; then
    echo "with-logd: preload libraries are missing; building them" >&2
    "$here/../build-native.sh" "$MOSAIC_ANDROID_ROOT" >&2 || exit 2
  fi
  for library in $MOSAIC_PRELOAD; do
    if [ ! -f "$library" ]; then
      echo "with-logd: MOSAIC_PRELOAD names $library, which does not exist." >&2
      echo "           Build them with: tools/build-native.sh <bundle>" >&2
      exit 2
    fi
  done
fi

# The framework's `Process.myUid()` is this process's own uid, and it checks it
# against Android's `SYSTEM_UID` (1000). `unshare -r` maps the user to the
# namespace's *root*, so the framework sees 0 and every check fails:
#
#   PackageManager: Non System Server process reporting dex loads as system server. uid=0
#   java.lang.SecurityException: Non-system caller
#     at IPackageManagerBase.getSetupWizardPackageName(IPackageManagerBase.java:769)
#
# Mapping to 1000 instead is not a swap: the mapping is one uid, and with root
# given up `mount -t tmpfs none /dev` cannot be done at all -- the capability
# comes with being the namespace's root. Running the framework as a system uid
# therefore needs a private /dev that does not come from a mount, which is how
# the paths are already handled for /system (tools/binder-shim/android-paths.c).
exec unshare -rm --propagation private bash -c '
  set -uo pipefail
  mount -t tmpfs none /dev || exit 1
  mkdir -p /dev/socket || exit 1

  # The framework is told it is the system uid. Process.myUid() is getuid(), and the
  # framework checks that against SYSTEM_UID, 1000, in many places -- the first of
  # which refuses the boot outright:
  #
  #   PackageManager: Non System Server process reporting dex loads as system server. uid=0
  #   java.lang.SecurityException: Non-system caller
  #     at IPackageManagerBase.getSetupWizardPackageName, line 769 of that file
  #
  # It is 0 here because the namespace this runs in makes us root, and root is needed:
  # the private /dev above is a mount, and the property area files must be owned by
  # root for libc to read them. A user namespace maps one uid, so root and 1000 cannot
  # both be had. On a device the process is the system uid, so this is what the harness
  # means by running the framework.
  export MOSAIC_UID=1000

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
  # A process that spins on a malformed driver reply fills a disk with log output.
  # That has happened twice, at five gigabytes each time, on a tmpfs. Both the
  # runtime and the output are bounded; the first byte preserving the exit status
  # is the command, not the cap.
  timeout "${MOSAIC_TIMEOUT:-120}" "$@" 2>&1 | head -c "${MOSAIC_MAX_OUTPUT:-2000000}"
  status=${PIPESTATUS[0]}
  if [ "$status" -eq 124 ]; then :; fi
  if [ $status -eq 124 ]; then
    echo "with-logd: timed out after ${MOSAIC_TIMEOUT:-120}s" >&2
  fi

  kill "$logd_pid" 2>/dev/null || true
  [ -n "${properties_pid:-}" ] && kill "$properties_pid" 2>/dev/null
  exit "$status"
' _ "$logd" "$properties" "$@"
