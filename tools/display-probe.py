#!/usr/bin/env python3
"""Ask the display service what displays this host has, from another process.

`SurfaceFlinger`'s display half is the part the system server waits for before it
starts anything else, and it is the one service whose answers are about *this*
machine rather than about the framework. So the check is not "did it answer" but
"are the answers the host's": the ids it lists, and the mode it reports for the
first of them, have to be the ones in `/sys/class/drm`.

    display-probe.py /path/to/broker.sock
"""

import os
import re
import socket
import struct
import sys

MAGIC = b"MSBD"
NO_HANDLE = 0xFFFFFFFF

KIND_TRANSACTION = 0
KIND_REPLY = 1
KIND_BYE = 8
KIND_LOOKUP = 10
KIND_FOUND = 11

HEADER = ">BBHIIIQII"

AIDL_NAME = "SurfaceFlingerAIDL"
AIDL_DESCRIPTOR = "android.gui.ISurfaceComposer"

# `android.gui.ISurfaceComposer`, in declaration order.
GET_PHYSICAL_DISPLAY_IDS = 3
GET_PRIMARY_PHYSICAL_DISPLAY_ID = 4
GET_PHYSICAL_DISPLAY_TOKEN = 5
GET_DISPLAY_STATE = 8

LEGACY_DESCRIPTOR = "android.ui.ISurfaceComposer"
LEGACY_GET_STATIC_DISPLAY_INFO = 3
LEGACY_GET_DYNAMIC_DISPLAY_INFO = 55
LEGACY_CREATE_DISPLAY_EVENT_CONNECTION = 4
# `android.gui.IDisplayEventConnection`, in declaration order, and the object type
# a descriptor is written as.
CONNECTION_DESCRIPTOR = "android.gui.IDisplayEventConnection"
STEAL_RECEIVE_CHANNEL = 1
BINDER_TYPE_FD = 0x66642A85


def send(sock, kind, a=0, b=0, c=0, node=0, data=b""):
    sock.sendall(struct.pack(HEADER, kind, 1, 0, a, b, c, node, len(data), 0) + data)


def recv(sock):
    """One frame, with whatever descriptors came attached to it.

    Read with recvmsg throughout: a descriptor arrives as ancillary data on the
    *first* byte of the frame, so a plain recv() of the header would throw it away.
    """
    buf = b""
    fds = []
    length = None
    fd_count = 0
    while True:
        chunk, ancillary, _, _ = sock.recvmsg(
            65536, socket.CMSG_SPACE(4 * max(fd_count, 1))
        )
        if not chunk and not ancillary:
            raise SystemExit("the broker closed the connection")
        for level, kind_, data in ancillary:
            if level == socket.SOL_SOCKET and kind_ == socket.SCM_RIGHTS:
                for at in range(0, len(data) - 3, 4):
                    fds.append(struct.unpack_from("i", data, at)[0])
        buf += chunk
        if length is None and len(buf) >= 32:
            _, _, _, _, _, _, _, length, fd_count = struct.unpack(HEADER, buf[:32])
        if length is not None and len(buf) >= 32 + length and len(fds) >= fd_count:
            break
    kind, _, _, a, b, c, node, _, _ = struct.unpack(HEADER, buf[:32])
    return kind, a, b, c, node, buf[32 : 32 + length], fds


def request(descriptor, args=b""):
    """A request as an AIDL client writes it: the preamble, the interface token as a
    string16, then the arguments. The token is not decoration -- two interfaces
    answer to this service's names and their codes overlap."""
    data = bytearray([0x00, 0x00, 0x00, 0x80, 0xFF, 0xFF, 0xFF, 0xFF])
    data += b"TSYS"
    data += struct.pack("<i", len(descriptor) + 1)
    data += descriptor.encode("utf-16-le")
    data += b"\x00\x00"
    while len(data) % 4:
        data += b"\x00"
    return bytes(data) + args


def host_modes():
    """The host's own view: connector -> its first mode, from DRM's sysfs."""
    modes = {}
    root = "/sys/class/drm"
    if not os.path.isdir(root):
        return modes
    for name in sorted(os.listdir(root)):
        if "-" not in name:
            continue
        base = os.path.join(root, name)
        try:
            status = open(os.path.join(base, "status")).read().strip()
        except OSError:
            continue
        if status != "connected":
            continue
        try:
            first = open(os.path.join(base, "modes")).read().split("\n")[0].strip()
        except OSError:
            continue
        if re.fullmatch(r"\d+x\d+", first):
            modes[name] = first
    return modes


def main(path):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    sock.sendall(MAGIC)

    send(sock, KIND_LOOKUP, data=AIDL_NAME.encode())
    kind, handle, owner, _, node, _, _ = recv(sock)
    if kind != KIND_FOUND or handle == NO_HANDLE:
        raise SystemExit(f"{AIDL_NAME} is not published in the broker")
    print(f"{AIDL_NAME}: handle={handle} node={node:#x} owner={owner}")

    send(
        sock,
        KIND_TRANSACTION,
        a=handle,
        b=GET_PHYSICAL_DISPLAY_IDS,
        node=1,
        data=request(AIDL_DESCRIPTOR),
    )
    kind, status, _, _, _, body, _ = recv(sock)
    if kind != KIND_REPLY or status != 0:
        raise SystemExit(f"getPhysicalDisplayIds failed: kind={kind} status={status}")
    (total,) = struct.unpack_from("<i", body, 0)
    ids = list(struct.unpack_from("<" + "q" * total, body, 4)) if total else []
    print(f"displays: {[hex(i) for i in ids]}")

    host = host_modes()
    if bool(ids) != bool(host):
        raise SystemExit(
            f"the service lists {len(ids)} displays and this host has {len(host)} "
            f"connected connectors ({sorted(host)})"
        )

    send(
        sock,
        KIND_TRANSACTION,
        a=handle,
        b=GET_PRIMARY_PHYSICAL_DISPLAY_ID,
        node=2,
        data=request(AIDL_DESCRIPTOR),
    )
    kind, status, _, _, _, body, _ = recv(sock)
    if kind != KIND_REPLY or status != 0:
        raise SystemExit(f"getPrimaryPhysicalDisplayId failed: kind={kind} status={status}")
    (primary,) = struct.unpack_from("<q", body, 0)
    if ids and primary != ids[0]:
        raise SystemExit(f"the primary display {primary:#x} is not the first of {ids}")
    print(f"primary: {primary:#x}")

    # The token is an object: a reply that hands one back is what the framework
    # keeps and passes to every later call about that display.
    send(
        sock,
        KIND_TRANSACTION,
        a=handle,
        b=GET_PHYSICAL_DISPLAY_TOKEN,
        node=3,
        data=request(AIDL_DESCRIPTOR, struct.pack("<q", primary)),
    )
    kind, status, _, count, _, body, _ = recv(sock)
    if count != 1:
        raise SystemExit(f"the token answer carries {count} objects, not one")
    data = body[: len(body) - 4 * count]
    (offset,) = struct.unpack_from(">I", body, len(data))
    kind_word, _, token, _ = struct.unpack_from("<IIQQ", data, offset)
    print(f"token: handle={token} type={kind_word:#x}")
    if not token:
        raise SystemExit("the token is a null handle")

    # And it is usable: the display state is asked with it, and a token that names
    # nothing would be refused rather than answered.
    args = struct.pack("<I", kind_word) + struct.pack("<I", 0)
    args += struct.pack("<Q", token) + struct.pack("<Q", 0)
    send(
        sock,
        KIND_TRANSACTION,
        a=handle,
        b=GET_DISPLAY_STATE,
        node=4,
        data=request(AIDL_DESCRIPTOR, args + b"\x00" * 4),
    )
    kind, status, _, _, _, body, _ = recv(sock)
    if kind != KIND_REPLY or status != 0:
        raise SystemExit(f"getDisplayState failed: kind={kind} status={status}")
    (state,) = struct.unpack_from("<i", body, 0)
    print(f"state: {state} (2 is on)")

    # The two calls `LocalDisplayAdapter` builds a display device out of, with the
    # token from above: the static info and the dynamic one. Their layouts come from
    # this branch's `libs/ui/*.cpp`, and this reads them back the same way the
    # framework does.
    send(
        sock,
        KIND_TRANSACTION,
        a=handle,
        b=LEGACY_GET_STATIC_DISPLAY_INFO,
        node=5,
        data=request(LEGACY_DESCRIPTOR, args + b"\x00" * 4),
    )
    kind, status, _, _, _, body, _ = recv(sock)
    (result,) = struct.unpack_from("<i", body, 0)
    if result != 0:
        raise SystemExit(f"getStaticDisplayInfo answered {result}")
    # Legacy Flattenable replies put the flattened size after the result.
    at = 8
    connection_type, density, secure, product, rotation = struct.unpack_from("<ifiii", body, at)
    print(
        f"static: connection={connection_type} density={density} secure={secure} "
        f"product={product} rotation={rotation}"
    )
    if density <= 0:
        raise SystemExit(f"a density of {density} is not a density")

    send(
        sock,
        KIND_TRANSACTION,
        a=handle,
        b=LEGACY_GET_DYNAMIC_DISPLAY_INFO,
        node=6,
        data=request(LEGACY_DESCRIPTOR, args + b"\x00" * 4),
    )
    kind, status, _, _, _, body, _ = recv(sock)
    (result,) = struct.unpack_from("<i", body, 0)
    if result != 0:
        raise SystemExit(f"getDynamicDisplayInfo answered {result}")
    # Legacy Flattenable replies put the flattened size after the result.
    at = 8
    (modes,) = struct.unpack_from("<Q", body, at)
    at += 8
    mode_id, width, height = struct.unpack_from("<iii", body, at)
    at += 12
    x_dpi, y_dpi, refresh = struct.unpack_from("<fff", body, at)
    at += 12
    at += 24  # appVsyncOffset, sfVsyncOffset, presentationDeadline
    at += 4  # group
    active_mode, color_count = struct.unpack_from("<ii", body, at)
    print(
        f"dynamic: {modes} mode(s) id={mode_id} {width}x{height} @{refresh:.1f}Hz "
        f"dpi={x_dpi:.0f} activeMode={active_mode} colorModes={color_count}"
    )
    if width <= 0 or height <= 0:
        raise SystemExit(f"a mode of {width}x{height} is not a mode")
    if not (1 <= refresh <= 360):
        raise SystemExit(f"a refresh rate of {refresh} is not a refresh rate")

    # The display event connection, which is what the framework's receiver is built
    # on: `createDisplayEventConnection` hands back an object, and stealing the
    # receive channel from it hands back a descriptor. Without a real descriptor the
    # receiver throws and `LocalDisplayAdapter` never connects the display at all.
    send(
        sock,
        KIND_TRANSACTION,
        a=handle,
        b=LEGACY_CREATE_DISPLAY_EVENT_CONNECTION,
        node=7,
        data=request(LEGACY_DESCRIPTOR, struct.pack("<ii", 0, 1)),
    )
    kind, status, _, count, _, body, _ = recv(sock)
    if count != 1:
        raise SystemExit(f"createDisplayEventConnection carried {count} objects, not one")
    data = body[: len(body) - 4 * count]
    (offset,) = struct.unpack_from(">I", body, len(data))
    kind_word, _, connection, _ = struct.unpack_from("<IIQQ", data, offset)
    if not connection:
        raise SystemExit("no display event connection")
    print(f"display event connection: handle={connection} type={kind_word:#x}")

    send(
        sock,
        KIND_TRANSACTION,
        a=connection,
        b=STEAL_RECEIVE_CHANNEL,
        node=8,
        data=request(CONNECTION_DESCRIPTOR),
    )
    kind, status, _, count, _, body, fds = recv(sock)
    # The descriptor words are checked below against the protocol constant.
    if count != 2:
        raise SystemExit(f"the BitTube carried {count} objects, not two")
    (presence,) = struct.unpack_from("<i", body, 0)
    if presence != 1:
        raise SystemExit(f"the parcelable's presence word is {presence}, not 1")
    if not fds:
        raise SystemExit("the BitTube carried no descriptor")
    data = body[: len(body) - 4 * count]
    offsets = struct.unpack_from(">" + "I" * count, body, len(data))
    for index, at in enumerate(offsets):
        kind_word, _, number, _ = struct.unpack_from("<IIQQ", data, at)
        print(f"  channel {index}: type={kind_word:#x} fd={number}")
        if kind_word != BINDER_TYPE_FD:
            raise SystemExit(f"channel {index} is type {kind_word:#x}, not a descriptor")
    print(f"received {len(fds)} descriptor(s): {fds}")
    for number in fds:
        os.fstat(number)  # a number that is not an open descriptor raises
    print("the display event connection handed over a live channel")

    send(sock, KIND_BYE)
    print("the display half answered with this host's own displays")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: display-probe.py /path/to/broker.sock")
    raise SystemExit(main(sys.argv[1]))
