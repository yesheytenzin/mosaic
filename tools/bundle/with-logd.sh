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

if [ $# -lt 1 ]; then
  sed -n '2,14p' "$0"
  exit 2
fi

exec unshare -rm --propagation private bash -c '
  set -uo pipefail
  mount -t tmpfs none /dev || exit 1
  mkdir -p /dev/socket || exit 1

  # A Bionic process reads system properties from /dev/__properties__, which
  # libc hardcodes. Point MOSAIC_PROPERTY_DIR at a directory built by
  # make-property-area.py to have it appear there. Inside the namespace we are
  # uid 0, which is what libc requires of the mapped files.
  if [ -n "${MOSAIC_PROPERTY_DIR:-}" ]; then
    mkdir -p /dev/__properties__
    cp -f "$MOSAIC_PROPERTY_DIR"/* /dev/__properties__/ || exit 1
    chown 0:0 /dev/__properties__/* || exit 1
    chmod 0644 /dev/__properties__/*
    echo "properties: $(ls /dev/__properties__ | wc -l) files in /dev/__properties__" >&2
  fi

  python3 "$1" >&2 &
  logd_pid=$!
  for _ in $(seq 1 200); do
    [ -S /dev/socket/logdw ] && break
    sleep 0.02
  done

  shift
  "$@"
  status=$?

  kill "$logd_pid" 2>/dev/null || true
  exit "$status"
' _ "$logd" "$@"
