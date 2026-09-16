#!/bin/bash
# Verify A6's gate: that the priority limit the framework needs is granted, and
# that a Bionic process then works without the harness stand-ins.
#
#   sudo tools/verify-priority-limit.sh
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
dropin=/etc/systemd/system/user@.service.d/mosaic.conf
if [ -f "$dropin" ]; then
  ok "$dropin"
else
  bad "$dropin is missing; install it with: make install"
fi

say "2. the broker's own unit"
unit=/usr/lib/systemd/user/mosaic-broker.service
if [ -f "$unit" ] && grep -q '^LimitNICE=' "$unit"; then
  ok "$(grep '^LimitNICE=' "$unit") in $unit"
else
  bad "$unit does not set LimitNICE; expected the packaged unit"
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

say ""
if [ "$fail" -eq 0 ]; then
  say "the limit is in place. The priority stand-ins in tools/binder-shim/pretend-nice.c"
  say "can go: rerun the framework with pretend-nice.so removed from MOSAIC_PRELOAD and"
  say "check that SystemServer still reaches StartActivityManager."
  exit 0
fi

say "the limit is not in place, so the stand-ins are still load-bearing: without them"
say "the run stops at InitBeforeStartServices with a SecurityException from"
say "setThreadPriority. Install the package (or restart the user session) and rerun."
exit 1
