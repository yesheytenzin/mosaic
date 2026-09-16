#!/usr/bin/env python3
"""Speak the Binder data plane to a running broker, to check the wiring.

The header is fixed size and big-endian so the C shim can write it too:

    0   u8   kind
    1   u8   version (1)
    2   u16  reserved
    4   u32  a
    8   u32  b
    12  u32  c
    16  u64  node
    24  u32  data length
    28  u32  descriptor count
    32  ...  data

A connection begins with the magic MSBD.
"""

import os
import socket
import struct
import sys

MAGIC = b"MSBD"
KIND = {
    0: "transaction",
    1: "reply",
    2: "acquire",
    3: "release",
    4: "increfs",
    5: "decrefs",
    6: "dead",
    7: "incoming",
    8: "bye",
    9: "export",
    10: "lookup",
    11: "found",
    12: "incoming-reply",
    13: "link",
    14: "unlink",
}


def send(sock, kind, a=0, b=0, c=0, node=0, data=b"", fd_count=0):
    header = struct.pack(">BBHIIIQII", kind, 1, 0, a, b, c, node, len(data), fd_count)
    sock.sendall(header + data)


def recv(sock):
    header = b""
    while len(header) < 32:
        chunk = sock.recv(32 - len(header))
        if not chunk:
            raise SystemExit("closed")
        header += chunk
    kind, version, _, a, b, c, node, length, fd_count = struct.unpack(">BBHIIIQII", header)
    body = b""
    while len(body) < length:
        body += sock.recv(length - len(body))
    return kind, a, b, c, node, body, fd_count


def main(path):
    """Print what the broker answers for a connection's first few messages.

    The C shim writes the same bytes; this is the shortest way to check a
    change to the framing without starting an Android process.
    """
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    sock.sendall(MAGIC)

    send(sock, 10, data=b"nothing.here")  # lookup
    kind, a, b, c, node, body, fds = recv(sock)
    name = KIND.get(kind, kind)
    print(f"lookup of an absent name -> {name} handle={a:#x} fds={fds}")
    assert kind == 11, f"expected found, got {name}"
    assert a == 0xFFFFFFFF, "an absent name must have no handle"

    # A node id carries the owner's pid in its high half, so a process cannot
    # publish a node that belongs to another one.
    node = (os.getpid() << 32) | 1
    send(sock, 9, node=node, data=b"example.svc")  # export
    send(sock, 10, data=b"example.svc")  # lookup the name just exported
    kind, a, b, c, seen, body, fds = recv(sock)
    name = KIND.get(kind, kind)
    print(f"lookup of a name we own -> {name} handle={a:#x} node={seen:#x} owner={b}")
    assert kind == 11, f"expected found, got {name}"
    assert seen == node, "the name resolves to the node that was exported"
    assert a != 0xFFFFFFFF, "the owner gets a handle to its own object"

    send(sock, 9, node=0x1234, data=b"someone.elses")  # export a foreign node
    send(sock, 10, data=b"someone.elses")  # it must not be there
    kind, a, b, c, seen, body, fds = recv(sock)
    name = KIND.get(kind, kind)
    print(f"lookup after a foreign export -> {name} handle={a:#x}")
    assert a == 0xFFFFFFFF, "a node from another pid must not be published"

    send(sock, 8)  # bye
    print("the broker served the data plane")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: binder-probe.py /path/to/broker.sock")
    main(sys.argv[1])
