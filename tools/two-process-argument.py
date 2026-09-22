#!/usr/bin/env python3
"""Pass an object as an *argument*, and have the callee call it back.

The other half of `broker-object.py`. A reply can hand an object back; this is a
caller handing one over, which is what a callback registration is. Three parties,
and the middle one is the point:

    1. this process looks up the owner's service and the suspend hal
    2. it calls the owner, passing the hal's handle as an argument
    3. the owner -- a *different* process -- calls that handle back
    4. and the answer here says what the hal told it

Step three is the whole of it. A handle is an index into one process's table, so
the number this process holds for the hal means nothing to the owner: what the
owner gets has to be minted for the owner, and only the broker can do that.

    two-process-argument.py /path/to/broker.sock <owner's service name>
"""

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

HAL_NAME = "android.system.suspend.ISystemSuspend/default"
HAL_DESCRIPTOR = "android.system.suspend.ISystemSuspend"

BINDER_TYPE_HANDLE = 0x73682A85

REQUEST_ID = 0x7A7A


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


def look_up(sock, name):
    send(sock, KIND_LOOKUP, data=name.encode())
    kind, handle, owner, _, node, _ = recv(sock)
    if kind != KIND_FOUND or handle == NO_HANDLE:
        raise SystemExit(f"{name} is not published in the broker")
    return handle, owner, node


def main(path, owner_name):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    sock.sendall(MAGIC)

    handle, owner, node = look_up(sock, owner_name)
    print(f"{owner_name}: handle={handle} node={node:#x} owner={owner}")
    hal, hal_owner, hal_node = look_up(sock, HAL_NAME)
    print(f"{HAL_NAME}: handle={hal} node={hal_node:#x} owner={hal_owner}")

    # The request: a word of whatever the callee expects, then the object word.
    # The contents of the word do not matter -- the broker writes the object at
    # that offset for the callee -- but the space does: the callee reads an object
    # there, and an object is 28 bytes.
    data = bytearray(4 + 28)
    struct.pack_into("<I", data, 4, BINDER_TYPE_HANDLE)
    struct.pack_into("<Q", data, 12, hal)
    # One ref: the object at offset four is a handle in *this* process's table, so
    # the node is zero and the broker resolves it from the handle it finds there.
    refs = struct.pack(">IQ", 4, 0)
    count = 1

    send(
        sock,
        KIND_TRANSACTION,
        a=handle,
        b=7,
        c=count << 16,
        node=REQUEST_ID,
        data=bytes(data) + refs,
    )
    kind, status, answer_id, _, _, body = recv(sock)
    text = body.decode(errors="replace")
    print(f"the owner answered: {text!r} (kind={kind} status={status} id={answer_id:#x})")
    if answer_id != REQUEST_ID:
        raise SystemExit("the answer does not carry this request's id")
    if HAL_DESCRIPTOR not in text:
        raise SystemExit(
            f"the owner could not reach the object it was passed: {text!r}"
        )

    send(sock, KIND_BYE)
    print("an object passed as an argument was called back by the callee")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(
            "usage: two-process-argument.py /path/to/broker.sock <owner's service name>"
        )
    raise SystemExit(main(sys.argv[1], sys.argv[2]))
