#!/bin/bash
# Start the framework with the HIDL service manager beside it, in one namespace.
#
#   boot-with-hwservicemanager.sh <bundle>
#
# Both processes have to share the namespace: `hwservicemanager` announces itself
# by setting the `hwservicemanager.ready` property, and that property lives in this
# namespace's property area and property socket. A daemon started outside it sets
# the property where the framework cannot see it. The framework waits for it
# before it will build a HIDL service manager at all:
#
#   HidlServiceManagement: Waited for hwservicemanager.ready for a second,
#                          waiting another...
#
# This is what `tools/boot-system-server.sh` runs through `with-logd.sh`. It is a
# file rather than a `bash -c` string because the string needs quotes of its own,
# and a single-quoted string inside a single-quoted string is not a string: the
# outer shell then reads `$hwservicemanager` and, with `set -u`, stops.
set -uo pipefail
bundle=${1:?bundle}
cd "$bundle"

# Trace the harness itself (`MOSAIC_HARNESS_TRACE=1`) when a run stops somewhere the
# script's own messages do not explain. It found the wait that ran in the wrong
# place once, which three hypotheses had failed to.
[ "${MOSAIC_HARNESS_TRACE:-0}" = 1 ] && set -x
echo "boot: running $0 (md5 $(md5sum "$0" | cut -d' ' -f1))" >&2

daemon_log=/tmp/mosaic-hwservicemanager.log
rm -f "$daemon_log"

# The system server's own class path is `SYSTEMSERVERCLASSPATH`, which is what a
# device does too: the zygote forks the system server with the class loader
# `ZygoteInit.getOrCreateSystemServerClassLoader` built from that variable. Passing
# a *different* list here -- even one with the same jars under the same names --
# makes a second class loader, and the apex jars then fail the identity check the
# framework makes of every service it loads from one:
#
#   java.lang.RuntimeException: Failed to create
#   com.android.server.NetworkStatsServiceInitializer: service must extend
#   com.android.server.SystemService
#
# `systemserverclasspath.txt` stays what it is for the bundle's own tools; the
# kernel of this is that the *same string* has to be behind both.
system_server_classpath="${SYSTEMSERVERCLASSPATH:-$(cat systemserverclasspath.txt)}"

# The daemon and the waits that belong to it. A chrooted daemon logs through logd
# rather than to its own stderr, so the log-line wait is a diagnostic; the wait that
# decides is the property one, which is what the framework itself waits on.
start_daemon() {
  if [ "${1:-pure}" = chrooted ]; then
    env -i PATH=/bin ANDROID_ROOT=/ ANDROID_DATA=/data ANDROID_TMP=/tmp \
      LD_CONFIG_FILE=/linkerconfig/ld.config.txt LD_LIBRARY_PATH=/lib64:/lib64/bionic \
      LD_PRELOAD="$MOSAIC_PRELOAD_INSIDE" MOSAIC_ANDROID_ROOT=/ MOSAIC_PROPERTY_DIR=/properties \
      MOSAIC_BINDER_BROKER=1 MOSAIC_BINDER_SOCKET="$MOSAIC_BINDER_SOCKET" \
      MOSAIC_BINDER_DEBUG="${MOSAIC_BINDER_DEBUG:-0}" MOSAIC_UID=1000 \
      chroot "$PWD" /linker64 /bin/hwservicemanager > "$daemon_log" 2>&1 &
  else
    env -u MOSAIC_LAUNCH_CLASS -u MOSAIC_LAUNCH_RUNTIME \
      ./run.sh hwservicemanager > "$daemon_log" 2>&1 &
  fi
  hwservicemanager=$!

  # Readiness is the daemon's own record rather than a socket, so wait for it to say
  # so instead of for a fixed time: a slow start does not look like a client failure,
  # and a fast one costs nothing.
  for _ in $(seq 1 600); do
    grep -aq 'hwservicemanager is ready now' "$daemon_log" 2>/dev/null && break
    sleep 0.05
  done
  # Reported, not fatal. A daemon that runs *inside* the chroot logs through logd --
  # its readiness line goes to logd's socket rather than to this file -- so an empty
  # log here says nothing about whether it is ready.
  if ! grep -aq 'hwservicemanager is ready now' "$daemon_log" 2>/dev/null; then
    echo "boot-with-hwservicemanager: no readiness line in $daemon_log; waiting on the property instead" >&2
    head -20 "$daemon_log" >&2
  fi

  # And then for the *property*, which is what the framework actually waits on: the
  # daemon logs its readiness and sets `hwservicemanager.ready` a moment later, and a
  # process that maps the property area in between reads the area as it was. Waiting
  # for the value here removes that window, and it is the same wait the framework
  # would do -- one that can succeed, which is the difference.
  ready=0
  for _ in $(seq 1 600); do
    python3 - <<'PY' >/dev/null 2>&1 && ready=1 && break
import os, struct
# The index rows are `name<TAB>file<TAB>offset`. The middle column names the area
# file *inside* the directory -- for a `default_prop` that file is
# `u:object_r:default_prop:s0`, and the probe found it on disk with `properties_serial`
# still holding a zero serial. The record is `prop_info`: a `uint32_t` serial followed
# by 92 bytes of value.
area = "/dev/__properties__"
try:
    entry = None
    for line in open(os.path.join(area, "index.tsv")):
        parts = line.rstrip("\n").split("\t")
        if len(parts) >= 3 and parts[0] == "hwservicemanager.ready":
            entry = (parts[1], int(parts[2]))
            break
    if entry is None:
        raise SystemExit(1)
    # The index offset is relative to the data, and the area header sits in front of
    # it: the record for a set property is at `offset + 128`, which is where a probe
    # found `true` with serial 0x04000002 while `offset` alone was still zero.
    with open(os.path.join(area, entry[0]), "rb") as fh:
        fh.seek(entry[1] + 128)
        record = fh.read(96)
    if len(record) >= 96:
        serial, = struct.unpack_from("<I", record, 0)
        value = record[4:96].split(b"\0", 1)[0]
        if serial and value == b"true":
            raise SystemExit(0)
except SystemExit:
    raise
except Exception:
    pass
raise SystemExit(1)
PY
    sleep 0.05
  done
  # Loudly, not silently: a framework started while the property still says "false"
  # waits on it one second at a time, which reads as "the boot hung" and sends you
  # looking at the framework.
  # Reported, not fatal: this reader has been wrong twice while the value itself was
  # set (the index rows are `name<TAB>context<TAB>offset`, the offset is data-relative
  # and the area header sits in front of it), and a reader's bug must not read as the
  # daemon's. The framework's own wait is the authority; the dump below prints what
  # this file holds either way.
  if [ "${ready:-0}" != 1 ]; then
    echo "boot-with-hwservicemanager: this harness could not read hwservicemanager.ready as true; starting the framework anyway" >&2
  fi
}

# In the chroot layout the daemon is started inside that branch, after the staging it
# needs (the linker configuration, the mounted directories, the environment).
[ "${MOSAIC_CHROOT:-0}" = 1 ] || start_daemon

# `MOSAIC_CHROOT=1` runs the framework with the *bundle* as its root, so that every
# path it builds from `ANDROID_ROOT=/` lands on the bundle: `/apex/<module>` for the
# apex packages -- which is the path the connectivity module checks a package's
# sourceDir against. A chroot is allowed inside the harness's user namespace, whose
# root holds CAP_SYS_CHROOT, so this needs no real root.
if [ "${MOSAIC_CHROOT:-0}" = 1 ]; then
  # `--rbind`, not `--bind`: inside the user namespace a plain bind of the host's
  # `/proc` and `/dev` is refused ("wrong fs type, bad option, bad superblock on
  # /proc"), and the recursive form is allowed. Found by running it, not by reading.
  for dir in dev proc tmp; do
    mkdir -p "$PWD/$dir"
    mount --rbind "/$dir" "$PWD/$dir" || echo "mosaic: could not mount /$dir into the chroot" >&2
  done
  # The paths first, then the bare root: with the other order every `<bundle>/lib64`
  # becomes `//lib64`, and a section whose `dir.` is `//` matches no executable, so no
  # namespace and no search path is created. `dir.system = /` parses as an *empty*
  # value ("warning: property value is empty"), so the section matches no executable
  # and the namespace it describes is never created. `/bin` is a real directory and
  # the executable is under it.
  #
  # The shim's directory has to be in the namespace's search and permitted paths: the
  # preload list names `/shim/*.so` by absolute path, and android's linker refuses an
  # absolute path that no `dir.`/`namespace` entry covers -- which is what
  # `/shim/launcher.so` reports as "cannot be preloaded (cannot open shared object
  # file)" while the file is right there in the root.
  sed -e "s|$PWD/|/|g" -e "s|$PWD|/|g" -e "s|^dir\.system = .*|dir.system = /bin|" \
    -e "s|^\(namespace\.default\.search\.paths = .*\)|\1:/shim|" \
    -e "s|^\(namespace\.default\.permitted\.paths = .*\)|\1:/shim|" \
    ld.config.txt > ld.config.chroot.txt || exit 1
  # android's linker reads its configuration from `/linkerconfig/ld.config.txt` under
  # the root it is given; `LD_CONFIG_FILE` is not consulted by it (the run says so:
  # 'failed to find generated linker configuration from "/linkerconfig/ld.config.txt"').
  mkdir -p "$PWD/linkerconfig"
  cp -f "$PWD/ld.config.chroot.txt" "$PWD/linkerconfig/ld.config.txt"


  start_daemon chrooted

  # `LD_PRELOAD`, not `MOSAIC_PRELOAD`: `run.sh` is what normally turns the latter
  # into the former, and this bypasses `run.sh` because there is no shell inside the
  # bundle. Without it no shim loads, so no path is redirected and ART calls every
  # boot classpath entry a non-existent dex file.
  #
  # No comment lines *inside* the continuation below: a `#` in a continued command
  # ends the command, so the rest of the environment and the `chroot` itself are
  # silently dropped -- which is how the first version of this looked like it ran a
  # chroot and did not.
  env -i PATH=/bin ANDROID_ROOT=/ ANDROID_DATA=/data ANDROID_TMP=/tmp \
    LD_CONFIG_FILE=/linkerconfig/ld.config.txt LD_LIBRARY_PATH=/lib64:/lib64/bionic \
    LD_PRELOAD="$MOSAIC_PRELOAD_INSIDE" MOSAIC_ANDROID_ROOT=/ MOSAIC_PROPERTY_DIR=/properties \
    MOSAIC_BINDER_BROKER=1 MOSAIC_BINDER_SOCKET="$MOSAIC_BINDER_SOCKET" \
    MOSAIC_BINDER_DEBUG="${MOSAIC_BINDER_DEBUG:-0}" MOSAIC_UID=1000 \
    MOSAIC_LAUNCH_CLASS="${MOSAIC_LAUNCH_CLASS:-com.android.server.SystemServer}" \
    MOSAIC_LAUNCH_RUNTIME="${MOSAIC_LAUNCH_RUNTIME:-/lib64/libandroid_runtime.so}" \
    chroot "$PWD" /linker64 /bin/dalvikvm64 \
    -Xbootclasspath:"$(sed "s|$PWD/|/|g;s|$PWD|/|g" bootclasspath.txt | tr '\n' ':')" \
    -cp "$(printf '%s' "$system_server_classpath" | sed "s|$PWD/|/|g;s|$PWD|/|g")" &
else
  ./run.sh dalvikvm64 \
    -Xbootclasspath:"$(cat bootclasspath.txt)" -cp "$system_server_classpath" &
fi
framework=$!
trap 'kill "$framework" "$hwservicemanager" 2>/dev/null || true' EXIT

# What the entry holds *while* the framework runs. A framework that is waiting on
# this property and a framework that is past it look the same from outside, and the
# difference between them has been seen to change between runs of the same bundle.
# This prints the file's half at five seconds, which is early in a boot:
#
#   property dump: hwservicemanager.ready serial=0x... dirty=0 value=b'true'
sleep 5
python3 - <<'PY' 2>&1 || true
import os, struct
area = "/dev/__properties__"
try:
    entry = None
    for line in open(os.path.join(area, "index.tsv")):
        parts = line.rstrip("\n").split("\t")
        if len(parts) >= 3 and parts[0] == "hwservicemanager.ready":
            entry = (parts[1], int(parts[2]))
            break
    if entry is None:
        raise SystemExit("no index row")
    with open(os.path.join(area, entry[0]), "rb") as fh:
        fh.seek(entry[1] + 128)
        record = fh.read(96)
    serial, = struct.unpack_from("<I", record, 0)
    value = record[4:96].split(b"\0", 1)[0]
    print(f"property dump: hwservicemanager.ready serial=0x{serial:08x} value={value!r} name=b'hwservicemanager.ready\\x00'")
except SystemExit:
    raise
except Exception as exc:
    print(f"property dump: unreadable: {exc}")
PY

# The framework runs until it stops or until the harness's own timeout. This is the
# same wait the wall readings in `docs/remaining-work.md` were taken under.
timeout="${MOSAIC_TIMEOUT:-250}"
waited=0
while kill -0 "$framework" 2>/dev/null && [ "$waited" -lt "$timeout" ]; do
  sleep 1
  waited=$((waited + 1))
done
if kill -0 "$framework" 2>/dev/null; then
  echo "boot: framework still running after ${timeout}s" >&2
  kill "$framework" 2>/dev/null || true
fi
wait "$framework" 2>/dev/null
status=$?
echo "boot: framework exit status $status after ${waited}s" >&2
exit "$status"
