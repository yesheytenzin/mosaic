#!/usr/bin/env python3
"""Look a service up in the broker and call it, as a second process.

The point is the round trip: the name is published by a *different* process (the
shim inside an Android process), the broker hands this one a handle for it, the
call goes out as a transaction, the owner runs it and answers, and the answer
comes back here. That is what "a transaction between two processes" means.

    two-process-call.py /path/to/broker.sock <service name> [code]
"""

import os
import socket
import struct
import sys

MAGIC = b"MSBD"
NO_HANDLE = 0xFFFFFFFF

KIND_TRANSACTION = 0
KIND_REPLY = 1
KIND_INCOMING = 7
KIND_BYE = 8
KIND_LOOKUP = 10
KIND_FOUND = 11
KIND_INCOMING_REPLY = 12

HEADER = ">BBHIIIQII"


def send(sock, kind, a=0, b=0, c=0, node=0, data=b"", fd_count=0):
    sock.sendall(struct.pack(HEADER, kind, 1, 0, a, b, c, node, len(data), fd_count) + data)


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
    return kind, a, b, c, node, body, fd_count


def main(path, name, code):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    sock.sendall(MAGIC)

    send(sock, KIND_LOOKUP, data=name.encode())
    kind, handle, owner, _, node, body, _ = recv(sock)
    if kind != KIND_FOUND or handle == NO_HANDLE:
        raise SystemExit(f"{name} is not published in the broker")
    print(f"{name}: handle={handle} node={node:#x} owner={owner}")

    # A transaction with the code the caller was given. The owner is a Java
    # object in the other process, so an unknown code comes back as an error --
    # which is still the round trip finishing.
    send(sock, KIND_TRANSACTION, a=handle, b=code, c=0, data=b"")
    kind, status, _, _, _, body, fds = recv(sock)
    print(f"answer: kind={kind} status={status} bytes={len(body)} fds={fds}")

    send(sock, KIND_BYE)
    return 0 if kind == KIND_REPLY else 1


if __name__ == "__main__":
    if len(sys.argv) < 3:
        raise SystemExit("usage: two-process-call.py /path/to/broker.sock NAME [code]")
    raise SystemExit(main(sys.argv[1], sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 1))
