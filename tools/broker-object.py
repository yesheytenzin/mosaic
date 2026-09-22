#!/usr/bin/env python3
"""Take an object a reply handed back, and call it.

A transaction can answer with more than bytes: `ISystemSuspend.acquireWakeLock`
answers with an `IWakeLock`, which the caller holds and calls. That object is not
in any name table -- it exists because of the call that returned it -- so the
answer has to carry it, and this checks the whole of that:

    1. the hal is looked up by name, as any service is
    2. `acquireWakeLock` answers with a handle this process has never seen
    3. that handle answers with the *lock's* descriptor, not the hal's

Step three is the point. A handle that resolved to nothing, or to the object that
handed it out, would fail it.

    broker-object.py /path/to/broker.sock
"""

import os
import struct
import socket
import sys

MAGIC = b"MSBD"
NO_HANDLE = 0xFFFFFFFF

KIND_TRANSACTION = 0
KIND_REPLY = 1
KIND_BYE = 8
KIND_LOOKUP = 10
KIND_FOUND = 11

HEADER = ">BBHIIIQII"

HAL_NAME = "android.system.suspend.ISystemSuspend/default"
HAL_DESCRIPTOR = "android.system.suspend.ISystemSuspend"
LOCK_DESCRIPTOR = "android.system.suspend.IWakeLock"

ACQUIRE_WAKE_LOCK = 1
RELEASE = 1
INTERFACE_TRANSACTION = 0x5F4E5446

BINDER_TYPE_HANDLE = 0x73682A85


def send(sock, kind, a=0, b=0, c=0, node=0, data=b""):
    sock.sendall(struct.pack(HEADER, kind, 1, 0, a, b, c, node, len(data), 0) + data)


def recv(sock):
    header = b""
    while len(header) < 32:
        chunk = sock.recv(32 - len(header))
        if not chunk:
            raise SystemExit("the broker closed the connection")
        header += chunk
    kind, _, _, a, b, c, node, length, fd_count = struct.unpack(HEADER, header)
    body = b""
    while len(body) < length:
        body += sock.recv(length - len(body))
    return kind, a, b, c, node, body


def string16(body, at=0):
    """The string16 at `at`: a count of UTF-16 units, then the units."""
    (count,) = struct.unpack_from("<i", body, at)
    if count < 0:
        return None, at + 4
    text = body[at + 4 : at + 4 + count * 2].decode("utf-16-le").rstrip("\0")
    return text, at + 4 + count * 2


def main(path):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    sock.sendall(MAGIC)

    send(sock, KIND_LOOKUP, data=HAL_NAME.encode())
    kind, handle, owner, _, node, body = recv(sock)
    if kind != KIND_FOUND or handle == NO_HANDLE:
        raise SystemExit(f"{HAL_NAME} is not published in the broker")
    print(f"the hal: handle={handle} node={node:#x} owner={owner}")

    # A descriptor query first: an AIDL interface is asked what it is before it is
    # called, and an object that cannot answer is an object nothing will use.
    send(sock, KIND_TRANSACTION, a=handle, b=INTERFACE_TRANSACTION, node=1)
    kind, status, _, count, _, body = recv(sock)
    text, _ = string16(body)
    print(f"the hal says it is {text!r} ({len(body)} bytes, {count} objects)")
    if text != HAL_DESCRIPTOR:
        raise SystemExit(f"the hal answered with {text!r}, not {HAL_DESCRIPTOR!r}")

    send(sock, KIND_TRANSACTION, a=handle, b=ACQUIRE_WAKE_LOCK, node=2)
    kind, status, _, count, _, body = recv(sock)
    if kind != KIND_REPLY or status != 0:
        raise SystemExit(f"acquireWakeLock failed: kind={kind} status={status}")
    if count != 1:
        raise SystemExit(f"the answer carries {count} objects, not one")
    # The object offsets follow the data, so the data stops four bytes short of
    # the body for each one.
    data = body[: len(body) - 4 * count]
    (offset,) = struct.unpack_from(">I", body, len(data))
    kind_word, _, lock, _ = struct.unpack_from("<IIII", data, offset)
    if kind_word != BINDER_TYPE_HANDLE:
        raise SystemExit(f"the object is type {kind_word:#x}, not a handle")
    print(f"the wake lock: handle={lock} at offset {offset} of {len(data)} data bytes")

    # The handle has never been seen here before, and it reaches the lock.
    send(sock, KIND_TRANSACTION, a=lock, b=INTERFACE_TRANSACTION, node=3)
    kind, status, _, count, _, body = recv(sock)
    text, _ = string16(body)
    print(f"the lock says it is {text!r}")
    if text != LOCK_DESCRIPTOR:
        raise SystemExit(f"the handle reached {text!r}, not {LOCK_DESCRIPTOR!r}")

    # `release` is oneway: the caller sends it and does not wait for an answer.
    send(sock, KIND_TRANSACTION, a=lock, b=RELEASE, c=1, node=4)

    send(sock, KIND_BYE)
    print("the object a reply handed back was called")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: broker-object.py /path/to/broker.sock")
    raise SystemExit(main(sys.argv[1]))
