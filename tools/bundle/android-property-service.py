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
    with open(path, "wb") as handle:
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


def apply_write(index: dict, name: str, value: str) -> str:
    entry = index.get(name)
    if entry is None:
        return add_property(index, name, value)
    filename, offset = entry
    raw = value.encode()
    if len(raw) >= PROP_VALUE_SIZE:
        return "value too long, dropped"

    path = os.path.join(AREA_DIR, filename)
    with open(path, "r+b") as area:
        # serial's top byte is the length; the rest of the value area is zeroed
        # so a shorter value cannot leave a longer one's tail behind.
        area.seek(offset)
        area.write(struct.pack("<I", (len(raw) & 0xFF) << 24))
        area.write(raw + b"\x00" * (PROP_VALUE_SIZE - len(raw)))
    return "applied"


def main() -> int:
    index = load_index()
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

            outcome = apply_write(index, name, value)
            print(f"property-service: set {name}={value!r} -> {outcome}", file=sys.stderr, flush=True)
        finally:
            # Closing is the acknowledgement this protocol uses.
            connection.close()


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(0)
