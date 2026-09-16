#!/usr/bin/env python3
# A minimal logd for host-side debugging.
#
# A Bionic process logs through liblog, which sends datagrams to
# /dev/socket/logdw and drops them when nothing is listening. That is why an
# Android process on a Linux host aborts with no explanation at all, and why
# "Fatal signal 6" is often the only thing you see.
#
# Point a private /dev/socket at this listener and the LOG(...) output of ART,
# the linker, and libnativebridge becomes readable. See with-logd.sh, which sets
# that up without root.
#
# The wire format is a fixed header followed by "tag NUL message". The header
# layout differs between Android releases and is not worth reconstructing here,
# so this finds the payload instead: it skips the binary header to the first run
# of printable ASCII, which is the tag. Set ANDROID_LOG_RAW=1 to also print the
# header bytes when a message looks wrong.
#
# It reads and prints. It does not forward, buffer, or implement any of logd's
# other duties.

import os
import socket
import sys

SOCKET_PATH = os.environ.get("ANDROID_LOG_SOCKET", "/dev/socket/logdw")
RAW = os.environ.get("ANDROID_LOG_RAW") == "1"


def payload_start(datagram: bytes, run: int = 4):
    """Index of the first run of `run` printable bytes, or None."""
    for i in range(0, max(0, len(datagram) - run) + 1):
        if all(32 <= b < 127 for b in datagram[i : i + run]):
            return i
    return None


def decode(datagram: bytes) -> str:
    start = payload_start(datagram)
    if start is None:
        return repr(datagram)
    tag, _, message = datagram[start:].partition(b"\x00")
    text = "{}: {}".format(
        tag.decode("utf-8", "replace"),
        message.decode("utf-8", "replace").rstrip("\n"),
    )
    if RAW:
        text = "[{}] {}".format(datagram[:start].hex(" "), text)
    return text


def main() -> int:
    parent = os.path.dirname(SOCKET_PATH)
    if parent:
        os.makedirs(parent, exist_ok=True)
    if os.path.exists(SOCKET_PATH):
        os.unlink(SOCKET_PATH)

    # liblog sends to logdw with SOCK_DGRAM.
    server = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    server.bind(SOCKET_PATH)
    os.chmod(SOCKET_PATH, 0o666)

    print(f"logd: listening on {SOCKET_PATH}", file=sys.stderr, flush=True)
    while True:
        print(decode(server.recv(65536)), flush=True)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(0)
