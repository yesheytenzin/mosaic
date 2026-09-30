#!/usr/bin/env python3
"""Write the classpath environment the framework reads, from the apex configs.

On a device these are not files the framework opens: `init` runs
`derive_classpath`, which reads every `etc/classpaths/*.pb` in the system and in
each apex, merges them, and writes `/data/system/environ/classpath` -- a file of
`export NAME value` lines that `init` then loads into the environment of every
process it starts. The names are the ones `SystemConfig` reads:

  SYSTEMSERVERCLASSPATH       the jars the system server loads into its loader
  STANDALONE_SYSTEMSERVER_JARS  the jars it may load *later*, at which point
                                `SystemServerClassLoaderFactory` refuses anything
                                under `/apex/` that was not prefetched:

    Creating a ClassLoader from /apex/com.android.tethering/javalib/
    service-connectivity.jar is not allowed. Please make sure that the jar is
    listed in `PRODUCT_APEX_STANDALONE_SYSTEM_SERVER_JARS` ...

This runtime does not run `init`, so the same file is produced here -- from the
same configs, in the same format -- and the harness exports it before it starts
the framework.

    classpath-exports.py <directory of dumped configs> <output file>

The config directory is the bundle's `index/`, where `classpath_configs` in
`bundle.sh` has already dumped `system-<name>.pb` and `<apex>-<name>.pb`.

The lines are `export NAME="value"` because they are loaded by a shell: `export
NAME value` is two arguments to bash's `export`, which rejects the value as an
identifier and exports nothing.

The values are colon separated, which is how the framework splits them:
`ZygoteInit.prefetchStandaloneSystemServerJars` does `envStr.split(":")` and
`getOrCreateSystemServerClassLoader` hands `SYSTEMSERVERCLASSPATH` straight to
`PathClassLoader`. A space-separated list reads as one long path, the prefetch
fails per jar and logs it, and every apex jar stays refused."

"""
import glob
import os
import re
import sys

# The groups the protobufs use, read off the image's own files:
#   1  boot classpath, for the platform's own loader
#   2  system server classpath
#   3  boot classpath, for a process that wants the *system* copies of the
#      framework jars rather than the apex ones
#   4  standalone system server jars: loaded on demand, and refused on demand if
#      the framework was not told about them
BOOT = (1, 3)
SYSTEM_SERVER = 2
STANDALONE = 4


def entries(path):
    """(`path`, group) for every entry in one dumped config."""
    data = open(path, "rb").read()
    out = []
    at = 0
    while at + 1 < len(data):
        if data[at] != 0x0A:  # a repeated field 1, length delimited
            at += 1
            continue
        length = data[at + 1]
        inner = data[at + 2 : at + 2 + length]
        at += 2 + length
        if not inner or inner[0] != 0x0A:
            continue
        text_length = inner[1]
        text = inner[2 : 2 + text_length].decode("utf-8", "replace")
        group = inner[2 + text_length + 1] if len(inner) > 2 + text_length + 1 else 0
        if text:
            out.append((text, group))
    return out


def collect(directory, suffix, groups):
    """The jars in one config kind with any of `groups`, in a stable order."""
    found = []
    for path in sorted(glob.glob(os.path.join(directory, f"*-{suffix}.pb"))):
        for text, group in entries(path):
            if group in groups and text not in found:
                found.append(text)
    return found


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__.strip().splitlines()[0], file=sys.stderr)
        return 2
    directory, output = sys.argv[1], sys.argv[2]
    system_server = collect(directory, "systemserverclasspath", (SYSTEM_SERVER,))
    standalone = collect(directory, "systemserverclasspath", (STANDALONE,))
    os.makedirs(os.path.dirname(output), exist_ok=True)
    with open(output, "w") as out:
        out.write("# Written by classpath-exports.py; see its header.\n")
        if system_server:
            out.write(
                'export SYSTEMSERVERCLASSPATH="%s"\n' % ":".join(system_server)
            )
        if standalone:
            out.write(
                'export STANDALONE_SYSTEMSERVER_JARS="%s"\n' % ":".join(standalone)
            )
    print(
        f"  + {os.path.relpath(output)} "
        f"({len(system_server)} system server jars, {len(standalone)} standalone)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
