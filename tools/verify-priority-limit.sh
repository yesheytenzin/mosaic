#!/bin/bash
# Verify A6's gate: that the priority limit the framework needs is granted, and
# that a Bionic process then works without the harness stand-ins.
#
#   sudo tools/verify-priority-limit.sh [bundle]
#
# With a runtime bundle, it also runs the framework *without* pretend-nice.so and
# checks that SystemServer still reaches StartActivityManager. That is the gate
# itself rather than the setting behind it, and root can raise the limit for that
# run even when the session's own limit is still 0 -- so it does not need a
# re-login, which the session-wide setting does.
#
# Why this needs root: the framework raises thread priorities with setpriority,
# and RLIMIT_NICE caps how far. The hard limit is 0 on a desktop session, and a
# process cannot raise its own hard limit -- the kernel wants CAP_SYS_RESOURCE in
# the *initial* user namespace. A user namespace does not help:
#
#   $ unshare -r -- sh -c 'ulimit -e 40'
#   sh: ulimit: scheduling priority: cannot modify limit: Operation not permitted
#
# So the grant has to be made system side, once, and this checks that it was.
#
# What is being checked, in order:
#
#   1. the system-side grant is in place for the user manager
#   2. the broker's own unit repeats it (systemd/mosaic-broker.service)
#   3. the user manager reports a non-zero limit -- if this is 0, the two files
#      above are installed but the manager has not been restarted since, or the
#      drop-in did not land where systemd looks
#   4. a child of the broker can *lower its niceness*, which is what every
#      androidSetThreadPriority call amounts to, and which fails at limit 0
#
# Step 4 is the gate. What it does not do is run the framework: that needs a
# runtime bundle, and the way to do it is to take the preload list the harness
# uses, remove pretend-nice.so from it, and confirm the SystemServer markers
# still reach StartActivityManager. Until this limit exists, they do not -- the
# run stops at InitBeforeStartServices with a SecurityException.
#
#   MOSAIC_PRELOAD="launcher.so probe.so android-binder.so android-properties.so" \
#     tools/bundle/with-logd.sh "$bundle/run.sh" dalvikvm64 ... 

set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "this needs root: the priority limit cannot be raised from a user session" >&2
  echo "try: sudo $0" >&2
  exit 2
fi

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
user=${SUDO_USER:-$(logname 2>/dev/null || echo "${USER:-}")}

if [ -z "$user" ]; then
  echo "cannot tell which user's session to check; run as: sudo -u <you> ..." >&2
  exit 2
fi

fail=0

say() { printf '%s\n' "$*"; }
ok() { printf '  ok    %s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; fail=1; }

say "1. system-side grant for the user manager"
# systemd reads both the vendor directory and /etc; the package installs into the
# vendor one and this checks either, because which is used is a packaging choice.
dropin=
for candidate in /usr/lib/systemd/system/user@.service.d/mosaic.conf \
                 /etc/systemd/system/user@.service.d/mosaic.conf; do
  [ -f "$candidate" ] && dropin=$candidate && break
done
if [ -n "$dropin" ] && grep -q '^LimitNICE=' "$dropin"; then
  ok "$dropin ($(grep '^LimitNICE=' "$dropin"))"
else
  bad "no user@.service drop-in grants LimitNICE; install one with: make install"
fi

say "2. the broker's own unit"
unit=/usr/lib/systemd/user/mosaic-broker.service
installed=$(grep '^LimitNICE=' "$unit" 2>/dev/null || true)
packaged=$(grep '^LimitNICE=' "$root/systemd/mosaic-broker.service" 2>/dev/null || true)
if [ -n "$installed" ]; then
  ok "$installed in $unit"
elif [ -n "$packaged" ]; then
  bad "$unit is installed but without LimitNICE; the packaged unit has $packaged"
  say "        this is a stale install: reinstall with: sudo make install"
else
  bad "$unit is missing; install it with: sudo make install"
fi

say "3. what the user manager reports"
reported=$(runuser -u "$user" -- systemctl --user show mosaic-broker.service -p LimitNICE 2>/dev/null |
  sed -n 's/^LimitNICE=//p')
if [ "${reported:-0}" != "0" ] && [ -n "${reported:-}" ]; then
  ok "LimitNICE=$reported"
else
  bad "LimitNICE=${reported:-unknown}; the drop-in is not in effect for this session"
  say "        (it applies to sessions started after it was installed)"
fi

say "4. can a child of the broker lower its niceness?"
# setpriority wants root or CAP_SYS_NICE for a *negative* nice, so the check runs
# as the user the broker runs as, and asks for what the framework asks for.
if runuser -u "$user" -- python3 - "$reported" <<'PY'
import resource
import sys

limit = int(sys.argv[1] or 0)
soft, hard = resource.getrlimit(resource.RLIMIT_NICE)
if soft != limit and limit:
    resource.setrlimit(resource.RLIMIT_NICE, (limit, hard))
try:
    import os
    os.setpriority(os.PRIO_PROCESS, 0, -10)
except PermissionError:
    sys.exit(1)
except OSError:
    sys.exit(1)
sys.exit(0)
PY
then
  ok "a child of the broker can set nice -10"
else
  bad "nice -10 was refused; androidSetThreadPriority would fail the same way"
fi

bundle=${1:-}
if [ -n "$bundle" ]; then
  say "5. the framework, without the priority stand-ins"
  if [ ! -x "$bundle/run.sh" ]; then
    bad "$bundle does not look like a runtime bundle (no run.sh)"
  else
    # The stand-ins are the one thing being removed here. Everything else is what
    # the harness normally preloads.
    preload="$root/tools/launcher/out/launcher.so"
    for library in probe android-binder android-properties; do
      preload="$preload $root/tools/binder-shim/out/$library.so"
    done
    # If a preload is missing the harness builds it, which is why this does not
    # refuse on that: the artifacts are gitignored, so cleaning the tree deletes
    # them.

    log=$(mktemp)
    # Root raises the limit here and hands the process to the invoking user with
    # setpriv, rather than sudo -u: sudo resets resource limits to the target
    # user's defaults, which would undo exactly the thing under test.
    (
      ulimit -e 40
      exec setpriv --reuid="$(id -u "$user")" --regid="$(id -g "$user")" --init-groups \
        env "HOME=$(getent passwd "$user" | cut -d: -f6)" \
        "MOSAIC_TIMEOUT=${MOSAIC_TIMEOUT:-90}" "MOSAIC_MAX_OUTPUT=900000" \
        "MOSAIC_ANDROID_ROOT=$bundle" "MOSAIC_PROPERTY_DIR=$bundle/properties" \
        "MOSAIC_BINDER_BROKER=0" "MOSAIC_PRELOAD=$preload" \
        "MOSAIC_LAUNCH_CLASS=com.android.server.SystemServer" \
        "MOSAIC_LAUNCH_RUNTIME=$bundle/lib64/libandroid_runtime.so" \
        sh -c 'cd "$MOSAIC_ANDROID_ROOT" && exec "$0" "$MOSAIC_ANDROID_ROOT/run.sh" dalvikvm64 \
                 -Xbootclasspath:"$(cat bootclasspath.txt)" -cp "$(cat systemserverclasspath.txt)"' \
        "$root/tools/bundle/with-logd.sh"
    ) >"$log" 2>&1 || true

    reached=$(grep -a -o 'SystemServerTiming: StartActivityManager' "$log" | head -1)
    refused=$(grep -ac 'SecurityException' "$log" || true)
    if [ -n "$reached" ]; then
      ok "reached StartActivityManager with no priority stand-ins"
      say "        pretend-nice.c can be deleted; remove it from MOSAIC_PRELOAD"
      say "        (log: $log)"
    else
      bad "did not reach StartActivityManager ($refused SecurityException in $log)"
    fi
  fi
  say ""
fi

if [ "$fail" -eq 0 ]; then
  say "the limit is in place. The priority stand-ins in tools/binder-shim/pretend-nice.c"
  say "can go: rerun the framework with pretend-nice.so removed from MOSAIC_PRELOAD and"
  say "check that SystemServer still reaches StartActivityManager."
  exit 0
fi

say "something above is not in place."
say ""
say "With a bundle as an argument, checks 1-3 are about the session's own limit and"
say "need the package installed and a new session; check 4 and 5 are about the"
say "mechanism and are what root can prove right now. The stand-ins are load-bearing"
say "exactly as long as the limit is missing: without them the run stops at"
say "InitBeforeStartServices with a SecurityException from setThreadPriority."
exit 1
