#!/usr/bin/env python3
"""Export a name on the broker and answer what is asked of it, as a second process.

An incoming transaction may carry objects among its arguments -- a callback the
caller wants called back -- and this calls the first one and reports what it said.
That is the whole of what an argument object is for, and the broker has already
done the part this side cannot: the handle in the word is *this* process's, minted
for it, not the caller's number.

The two-process gate used the framework as the owner, which dies a couple of
seconds into its boot (the suspend path, see docs/remaining-work.md). A gate whose
owner can vanish mid-call says nothing about the transport: a failure is as likely
to be the race as the code. This owner stays until it is told to stop, so what it
proves is the forwarding itself -- the name resolves, the broker carries the
transaction to the owner, the owner runs it, and the answer comes back with the
caller's request id on it.

    two-process-owner.py /path/to/broker.sock <service name> [node]
"""

import os
import socket
import struct
import sys

MAGIC = b"MSBD"

KIND_INCOMING = 7
KIND_BYE = 8
KIND_EXPORT = 9
KIND_INCOMING_REPLY = 12

HEADER = ">BBHIIIQII"
# A node id carries the exporting process's pid in its high half, which is how the
# broker stops one process publishing another's object (`src/binder/broker.rs`).
NODE = (os.getpid() << 32) | 1

KIND_TRANSACTION = 0
INTERFACE_TRANSACTION = 0x5F4E5446
# A request id of this side's own, for the call it makes back into the argument.
CALLBACK_REQUEST = 0x5151


def string16(body, at=0):
    """The string16 at `at`: a count of UTF-16 units, then the units."""
    (count,) = struct.unpack_from("<i", body, at)
    if count < 0:
        return None, at + 4
    text = body[at + 4 : at + 4 + count * 2].decode("utf-16-le").rstrip("\0")
    return text, at + 4 + count * 2


def send(sock, kind, a=0, b=0, c=0, node=0, data=b"", fd_count=0):
    sock.sendall(
        struct.pack(HEADER, kind, 1, 0, a, b, c, node, len(data), fd_count) + data
    )


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


def main(path, name, node):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    sock.sendall(MAGIC)
    send(sock, KIND_EXPORT, node=node, data=name.encode())
    print(f"exported {name} as node {node:#x}", flush=True)

    while True:
        kind, a, b, c, node, body, _ = recv(sock)
        if kind == KIND_INCOMING:
            # a is the caller's request id and must come back with the answer;
            # b is the transaction code, c the flags with the object count in its
            # upper half; the objects themselves are appended to the body.
            flags, count = c & 0xFFFF, c >> 16
            print(
                f"asked node {node:#x} code {b} request {a} objects {count}",
                flush=True,
            )
            answer = f"served code {b} for request {a}".encode()
            if count:
                data = body[: len(body) - 4 * count]
                offset = struct.unpack_from(">I", body, len(data))[0]
                kind_word, _, handle, _ = struct.unpack_from("<IIQQ", data, offset)
                print(
                    f"  its argument is a handle: type {kind_word:#x} value {handle}",
                    flush=True,
                )
                # Calling it back: a transaction of its own, on the handle the
                # broker minted for this process.
                send(
                    sock,
                    KIND_TRANSACTION,
                    a=handle,
                    b=INTERFACE_TRANSACTION,
                    node=CALLBACK_REQUEST,
                )
                back_kind, back_status, back_id, _, _, back_body, _ = recv(sock)
                text, _ = string16(back_body)
                print(f"  it says it is {text!r}", flush=True)
                answer = f"served code {b}; its argument says {text}".encode()
            send(sock, KIND_INCOMING_REPLY, a=0, b=a, node=node, data=answer)
        elif kind == KIND_BYE:
            return 0


if __name__ == "__main__":
    if len(sys.argv) < 3:
        raise SystemExit("usage: two-process-owner.py /path/to/broker.sock NAME [node]")
    raise SystemExit(
        main(sys.argv[1], sys.argv[2], int(sys.argv[3], 0) if len(sys.argv) > 3 else NODE)
    )
