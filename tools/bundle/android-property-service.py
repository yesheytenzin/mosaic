#!/usr/bin/env python3
"""The property service, for writes.

Bionic reads properties from the mapped area (which make-property-area.py
generates) and *writes* them over a socket at /dev/socket/property_service.
Mosaic had only the read half, so `SystemProperties.set` failed, and the system
server needs it in its first block of work.

The protocol, from bionic's libc/bionic/system_property_set.cpp:

  struct prop_msg { unsigned cmd; char name[32]; char value[92]; };

sent as one 128-byte write on a SOCK_STREAM connection, with cmd =
PROP_MSG_SETPROP. The service acknowledges by closing the connection, and the
client polls for POLLHUP. There is no reply body in this protocol; the newer
PROP_MSG_SETPROP2 is only used when ro.property_service.version says so, and it
does not here.

A write to a property the area already defines is applied in place: the prop_info
entry gets a new serial (its top byte is the value length, which is what makes
reads wait-free) and a new value. A property the area does not define is reported
and dropped, because adding one means appending to the area and the property_info
trie, and the areas are mapped by running processes.

This is what init does on a device, minus init. ADR-0013 gives the job to the
broker; this is the same thing in the harness, and the shape the broker's version
should take.
"""

import importlib.util
import os
import socket
import struct
import subprocess
import sys

# The area builder knows how to append to a prop_area, so a property created at
# runtime is added with the same code that writes the area in the first place.
_spec = importlib.util.spec_from_file_location(
    "make_property_area", os.path.join(os.path.dirname(os.path.abspath(__file__)), "make-property-area.py")
)
make_property_area = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(make_property_area)

SOCKET_PATH = os.environ.get("ANDROID_PROPERTY_SOCKET", "/dev/socket/property_service")
# Where the areas and the index live. The harness points this at its private
# /dev/__properties__.
AREA_DIR = os.environ.get("ANDROID_PROPERTY_DIR", "/dev/__properties__")
INDEX_FILE = "index.tsv"

PROP_MSG_SETPROP = 1
PROP_NAME_SIZE = 32
PROP_VALUE_SIZE = 92
MESSAGE_SIZE = 4 + PROP_NAME_SIZE + PROP_VALUE_SIZE
PROP_INFO_VALUE_OFFSET = 4  # serial, then the value


def load_index() -> dict:
    """name -> (file, offset of its prop_info)."""
    index = {}
    path = os.path.join(AREA_DIR, INDEX_FILE)
    try:
        with open(path) as handle:
            for line in handle:
                parts = line.rstrip("\n").split("\t")
                if len(parts) == 3:
                    name, filename, offset = parts
                    index[name] = (filename, int(offset))
    except FileNotFoundError:
        pass
    return index


def context_file() -> str:
    """The file the areas live in, which is the only one that is not one of ours."""
    for entry in sorted(os.listdir(AREA_DIR)):
        if entry not in ("property_info", "properties_serial", INDEX_FILE):
            return entry
    return None


def add_property(index: dict, name: str, value: str) -> str:
    """Append a property the area does not define yet.

    The trie has a catch-all prefix, so a name created here is findable without
    rewriting property_info, which running processes have mapped. Only the area
    grows, and it grows after everything already in it.
    """
    filename = context_file()
    if filename is None:
        return "no area file to add to"
    path = os.path.join(AREA_DIR, filename)
    with open(path, "rb") as handle:
        existing = handle.read()
    try:
        area = make_property_area.PropArea(existing=existing)
        area.add(name, value)
        written = area.finish()
    except Exception as error:  # noqa: BLE001 - reported, not raised
        return f"could not add: {error}"
    # In place, never truncated. The area is mapped by every Bionic process in the
    # system, and `"wb"` truncates the file before writing: a reader that touches the
    # mapping while it is zero length gets `SIGBUS`, which is what
    # `prop_area::find_property` faults with on whichever thread happened to be
    # reading. The image is the area's full size, so overwriting it changes nothing
    # about the file's length.
    with open(path, "r+b") as handle:
        handle.seek(0)
        handle.write(written)
    os.chmod(path, 0o644)
    index[name] = (filename, area.index[name])
    # Persist the index, so a restart of the service still knows where the
    # property it added lives.
    try:
        with open(os.path.join(AREA_DIR, INDEX_FILE), "w") as handle:
            for known, (where, offset) in sorted(index.items()):
                handle.write(f"{known}\t{where}\t{offset}\n")
    except OSError as error:
        return f"added but could not record it: {error}"
    return "added"


# A daemon the framework asks for. On a device `init` watches `ctl.start` and
# `ctl.stop` and runs the named service; here the property service is what receives
# the write, so it is what has to act on it. The framework starts the overlay
# compiler this way and then waits five seconds for the `idmap` service it
# registers, so a write that is stored and not acted on is a boot that fails with
# "Failed to connect to 'idmap' in 5000 milliseconds".
_children = {}


def handle_control(name: str, value: str) -> str:
    if name not in ("ctl.start", "ctl.stop"):
        return ""
    root = os.environ.get("MOSAIC_ANDROID_ROOT")
    if not root:
        return "no bundle to start it from"
    if name == "ctl.stop":
        child = _children.pop(value, None)
        if child is not None:
            child.terminate()
            return "stopped"
        return "not running"
    if value in _children and _children[value].poll() is None:
        return "already running"
    program = os.path.join(root, "bin", value)
    if not os.path.exists(program):
        return f"no {value} in the bundle"
    # Through the bundle's `run.sh`, not directly: that is what puts the Android
    # linker and the preload in front of a Bionic program, and a program started
    # without them does not run at all. The environment is otherwise the one this
    # service was started in, which is the framework's.
    # Through the bundle's own runner, which reads the binary's ELF interpreter and
    # uses the Android linker for a Bionic program and a direct exec for a host one.
    # `run.sh` alone is not enough: it execs directly, and a Bionic binary exec'd
    # that way fails with "required file not found" because its interpreter is
    # `/system/bin/linker64`.
    runner = os.path.join(root, "bundle.sh")
    command = [runner, "run", root, value] if os.path.exists(runner) else [program]
    environment = dict(os.environ)
    # The shim has to be in front of it, or it opens /dev/binder -- which a host
    # does not have -- and aborts. The framework's own preload list is in the
    # environment this service was started in.
    preload = environment.get("MOSAIC_PRELOAD")
    if preload:
        environment["LD_PRELOAD"] = preload
    _children[value] = subprocess.Popen(command, env=environment)
    return "started"


# `persist.*` survives a restart. Android keeps them in
# `/data/property/persistent_properties`, and init loads the ones it declared at
# boot; this is the same idea with a plain-text body, one `name=value` per line,
# because the file is ours and nothing else reads it. Written to a temporary file
# and renamed, so a service killed mid-write leaves the old file rather than half
# of a new one.
PERSIST_PREFIX = "persist."


def persist_path() -> str:
    root = os.environ.get("MOSAIC_ANDROID_ROOT")
    if not root:
        return ""
    return os.path.join(root, "data", "property", "persistent_properties")


def persist_save(name: str, value: str) -> None:
    path = persist_path()
    if not path:
        return
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        known = persist_load()
        known[name] = value
        temporary = path + ".new"
        with open(temporary, "w") as handle:
            for key, stored in sorted(known.items()):
                handle.write(f"{key}={stored}\n")
        os.replace(temporary, path)
    except OSError as error:
        print(f"property-service: could not persist {name}: {error}", file=sys.stderr, flush=True)


def persist_load() -> dict:
    path = persist_path()
    if not path or not os.path.exists(path):
        return {}
    found = {}
    try:
        with open(path) as handle:
            for line in handle:
                line = line.strip()
                if line and "=" in line and line.startswith(PERSIST_PREFIX):
                    key, _, stored = line.partition("=")
                    found[key] = stored
    except OSError:
        return {}
    return found


def persist_restore(index: dict) -> None:
    """Put every persisted property back into the area at startup.

    Only names the area already declares: `add_property` appends, and appending
    for a name nothing declared at boot is how an area grows without its
    `property_info` knowing, which is the thing that cannot be rewritten while
    processes have it mapped.
    """
    for name, value in sorted(persist_load().items()):
        if name in index:
            apply_write(index, name, value)


def apply_write(index: dict, name: str, value: str) -> str:
    entry = index.get(name)
    if entry is None:
        result = add_property(index, name, value)
        if result in ("added",) and name.startswith(PERSIST_PREFIX):
            persist_save(name, value)
        return result
    filename, offset = entry
    raw = value.encode()
    if len(raw) >= PROP_VALUE_SIZE:
        return "value too long, dropped"

    path = os.path.join(AREA_DIR, filename)
    with open(path, "r+b") as area:
        # The index holds offsets *into the area's data*, which is what the
        # generator's writer works in; a file offset is that plus the area header.
        # The header is 128 bytes and the two differ by exactly that, which is the
        # whole bug this replaced: writing at the recorded offset wrote 128 bytes
        # before the entry, into whatever is there -- a `prop_bt` node, whose name
        # a reader compares when it walks the area -- and the property then reads as
        # absent for every process (`find -> NOT FOUND`, empty value) while the
        # service truthfully reports "applied" and a byte-level look at the entry
        # still shows the old bytes.
        at = make_property_area.AREA_HEADER + offset
        raw = value.encode()
        if len(raw) >= PROP_VALUE_SIZE:
            return "value too long, dropped"
        # Before anything is written: the entry at this offset must be *this*
        # property's. The name is stored after the value area, which is the layout
        # the generator writes (serial, value[92], name), and a write to an offset
        # that is not the entry does not fail -- it lands in the middle of whatever
        # is there. Refusing is the safe answer: a value that could not be set is a
        # property that reads as it did, and a corrupted area is one that takes
        # names with it.
        area.seek(at + make_property_area.PROP_INFO_SIZE)
        at_entry = area.read(len(name) + 1)
        if at_entry != name.encode() + b"\x00":
            return (
                f"refused: the entry at {at} in {filename} is {at_entry!r}, "
                f"not {name}"
            )
        # The serial is what a reader uses to know a value changed, and libc's own
        # writer (`prop_info::update`) moves it in three steps: mark it dirty, write
        # the value, then publish `(length << 24) | (serial + 1)`. Writing the
        # length byte alone -- which is what this did -- leaves the counter still,
        # so a *reader* that is polling for the change (libbase's `WaitForProperty`,
        # which is how a Bionic client waits for `hwservicemanager.ready`) can read
        # the old value forever, and the length in the top byte is all a fresh
        # reader would believe.
        #
        # The value area is zeroed past the value so a shorter one cannot leave a
        # longer one's tail behind.
        area.seek(at)
        current = struct.unpack("<I", area.read(4))[0]
        dirty = current | 1
        area.seek(at)
        area.write(struct.pack("<I", dirty))
        area.seek(at + 4)
        area.write(raw + b"\x00" * (PROP_VALUE_SIZE - len(raw)))
        # The published serial counts from the *dirty* one, not from `current`:
        # `(dirty + 1)` is what clears the low bit, and a serial left with it set
        # is a value every reader refuses, which is what this did first --
        # `serial=0x04000001`, `dirty=1` in the area, and a waiter that polls the
        # name forever.
        area.seek(at)
        area.write(
            struct.pack("<I", ((len(raw) & 0xFF) << 24) | ((dirty + 1) & 0xFFFFFF))
        )
    return "applied"


def main() -> int:
    index = load_index()
    persist_restore(index)
    parent = os.path.dirname(SOCKET_PATH)
    if parent:
        os.makedirs(parent, exist_ok=True)
    if os.path.exists(SOCKET_PATH):
        os.unlink(SOCKET_PATH)

    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(SOCKET_PATH)
    os.chmod(SOCKET_PATH, 0o666)
    server.listen(16)

    print(
        f"property-service: listening on {SOCKET_PATH}, {len(index)} properties known",
        file=sys.stderr,
        flush=True,
    )

    while True:
        connection, _ = server.accept()
        try:
            message = b""
            while len(message) < MESSAGE_SIZE:
                chunk = connection.recv(MESSAGE_SIZE - len(message))
                if not chunk:
                    break
                message += chunk
            if len(message) < MESSAGE_SIZE:
                continue

            command, = struct.unpack_from("<I", message, 0)
            name = message[4 : 4 + PROP_NAME_SIZE].split(b"\x00", 1)[0].decode("utf-8", "replace")
            value = (
                message[4 + PROP_NAME_SIZE : MESSAGE_SIZE]
                .split(b"\x00", 1)[0]
                .decode("utf-8", "replace")
            )
            if command != PROP_MSG_SETPROP:
                print(f"property-service: command {command} ignored", file=sys.stderr, flush=True)
                continue

            outcome = handle_control(name, value) or apply_write(index, name, value)
            print(f"property-service: set {name}={value!r} -> {outcome}", file=sys.stderr, flush=True)
        finally:
            # Closing is the acknowledgement this protocol uses.
            connection.close()


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(0)
