#!/usr/bin/env python3
"""Generate the Android property area that a Bionic process reads at startup.

Bionic does not ask a daemon for system properties. It maps two things:

  <dir>/property_info       a serialized trie mapping a property name to a
                            context index, so that each property lands in the
                            right shared-memory area
  <dir>/properties_serial   a prop_area, required for initialisation to succeed
  <dir>/<context>           one prop_area per context, holding the name -> value
                            entries

<dir> is /dev/__properties__ in libc, hardcoded, which is why provisioning this
needs one privileged step (ADR-0013). Everything about the formats is taken from
AOSP at the version the bundle's libraries come from:

  bionic/libc/system_properties/{prop_area,system_properties,prop_info}.cpp
  bionic/libc/system_properties/include/system_properties/{prop_area,prop_info}.h
  bionic/libc/bionic/system_property_api.cpp
  system/core/property_service/libpropertyinfoparser/property_info_parser.{h,cpp}
  system/core/property_service/libpropertyinfoserializer/

Two details are easy to miss. Every mapped file must be owned by uid 0 and gid 0
and must not be group- or other-writable, or libc refuses to map it. And the
property_info header's size field must equal the file size exactly.

The generated trie is deliberately minimal: one root node with no children and
one exact-match entry per property. The reader breaks out of its descent when a
node has no children and then compares the whole remaining name against the
exact matches, so a flat list is a valid trie. That keeps the generator small
enough to read, at the cost of a linear match per lookup, which is irrelevant for
the few hundred properties a bundle defines.
"""

import argparse
import os
import stat
import struct
import sys

# prop_area, from prop_area.cpp and prop_area.h.
PROP_AREA_MAGIC = 0x504F5250
PROP_AREA_VERSION = 0xFC6ED0AB
PROP_AREA_SIZE = 128 * 1024
PROP_VALUE_MAX = 92
# The value of PROP_NAME_MAX in sys/system_properties.h. It only bounds the
# write API; the read path has no limit, and real build.prop files contain names
# longer than it (dalvik.vm.dex2oat-max-image-block-size is 37 characters), so
# this is a sanity bound rather than a format limit.
PROP_NAME_MAX = 128
# prop_info pairs the value with the property's serial number: the top byte is
# the value length, and bit 16 marks a value too long for the inline buffer.
PROP_LONG_FLAG = 1 << 16
PROP_LONG_ERROR = b"Must use __system_property_read_callback() to read"
PROP_LONG_ERROR_SIZE = 56
AREA_HEADER = 128  # sizeof(prop_area): 4 + 4 + 4 + 4 + 28 * 4
PROP_BT_SIZE = 20  # namelen, prop, left, right, children
PROP_INFO_SIZE = 96  # serial, value[92]

# property_info, from property_info_parser.h and libpropertyinfoserializer.
INFO_CURRENT_VERSION = 1
INFO_MINIMUM_SUPPORTED_VERSION = 1
INFO_HEADER_SIZE = 24  # 6 * uint32
TRIE_NODE_SIZE = 28  # 7 * uint32
PROPERTY_ENTRY_SIZE = 16  # 4 * uint32
NO_INDEX = 0xFFFFFFFF

# One context for every property. The name becomes the area's file name, so it
# has to be a plain string; nothing in the read path interprets it.
DEFAULT_CONTEXT = "u:object_r:default_prop:s0"

# Where the generator records each property's offset, for the writer.
INDEX_FILE = "index.tsv"

# Enough for app_process and ART to start. ART reads dalvik.vm.* and the SDK
# level, app_process reads the ABI list and ro.zygote, and framework code reads
# the product identity.
DEFAULT_PROPERTIES = {
    "ro.build.version.sdk": "33",
    "ro.build.version.release": "13",
    "ro.build.version.codename": "REL",
    # Build.VERSION dereferences ALL_CODENAMES[0] before checking its length, so
    # an empty list aborts the framework's static initialisation.
    "ro.build.version.all_codenames": "REL",
    "ro.build.version.known_codenames": "REL",
    "ro.build.version.preview_sdk": "0",
    "ro.build.version.preview_sdk_fingerprint": "REL",
    "ro.build.version.min_supported_target_sdk": "23",
    "ro.build.version.security_patch": "2023-01-01",
    "ro.build.version.base_os": "",
    "ro.build.version.incremental": "1",
    "ro.build.type": "userdebug",
    "ro.build.tags": "dev-keys",
    "ro.build.id": "TQ1A",
    "ro.build.display.id": "TQ1A.230101.001",
    "ro.build.flavor": "mosaic_x86_64-userdebug",
    "ro.build.host": "mosaic",
    "ro.build.user": "mosaic",
    "ro.build.date": "Sun Jan  1 00:00:00 UTC 2023",
    "ro.build.date.utc": "1672531200",
    "ro.build.fingerprint": (
        "mosaic/mosaic_x86_64/mosaic:13/TQ1A/1:userdebug/dev-keys"
    ),
    "ro.product.cpu.abi": "x86_64",
    "ro.product.cpu.abi2": "x86",
    "ro.product.cpu.abilist": "x86_64,x86",
    "ro.product.cpu.abilist32": "x86",
    "ro.product.cpu.abilist64": "x86_64",
    "ro.product.model": "Mosaic",
    "ro.product.brand": "mosaic",
    "ro.product.name": "mosaic_x86_64",
    "ro.product.device": "mosaic",
    "ro.product.manufacturer": "Mosaic",
    "ro.product.board": "mosaic",
    "ro.debuggable": "0",
    "ro.secure": "1",
    "ro.zygote": "zygote64",
    "ro.kernel.qemu": "0",
    "ro.dalvik.vm.native.bridge": "0",
    "dalvik.vm.isa.x86_64.variant": "default",
    "dalvik.vm.isa.x86_64.features": "default",
    "dalvik.vm.isa.x86.variant": "default",
    "dalvik.vm.isa.x86.features": "default",
    "dalvik.vm.dex2oat-filter": "verify",
    "dalvik.vm.image-dex2oat-filter": "verify",
    "dalvik.vm.heapsize": "512m",
    "dalvik.vm.heapgrowthlimit": "256m",
    # Properties the system server writes during boot. A write to a property the
    # area does not define is dropped (see android-property-service.py), so the
    # ones it writes early have to be here.
    "persist.sys.language": "en",
    "persist.sys.country": "US",
    "persist.sys.locale": "en-US",
    "persist.sys.localevar": "",
    "persist.sys.timezone": "UTC",
    "sys.system_server.start_count": "1",
    "sys.system_server.start_elapsed": "0",
    "sys.system_server.start_uptime": "0",
}


def align(value: int, to: int = 4) -> int:
    return (value + to - 1) // to * to


def read_build_prop(path: str) -> dict:
    """Read NAME=VALUE lines from a build.prop."""
    values = {}
    with open(path, "r", errors="replace") as handle:
        for line in handle:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("import "):
                # build.prop files can pull in others by path; the caller passes
                # the ones it wants instead, so this is skipped deliberately.
                continue
            name, separator, value = line.partition("=")
            if separator and name:
                values[name.strip()] = value.strip()
    return values


class PropArea:
    """A prop_area, built by the same allocation order libc's writer uses.

    Readers walk a trie of property names split at '.', with siblings in a
    binary search tree ordered by (length, bytes) and values in prop_info
    entries addressed by offset from data_. Offsets are relative to data_, which
    starts after the 128 byte header.
    """

    def __init__(self) -> None:
        self.buf = bytearray(PROP_AREA_SIZE)
        # name -> offset of its prop_info, for the writer.
        self.index = {}
        # prop_area's constructor allocates the root node and then a dirty
        # backup area the size of one value.
        self.used = PROP_BT_SIZE + align(PROP_VALUE_MAX)

    def _data(self) -> memoryview:
        return memoryview(self.buf)[AREA_HEADER:]

    def _u32(self, offset: int) -> int:
        return struct.unpack_from("<I", self._data(), offset)[0]

    def _set_u32(self, offset: int, value: int) -> None:
        struct.pack_into("<I", self._data(), offset, value)

    def _alloc(self, size: int) -> int:
        size = align(size)
        offset = self.used
        self.used += size
        if AREA_HEADER + self.used > PROP_AREA_SIZE:
            raise ValueError("property area is full")
        return offset

    def _new_prop_bt(self, name: str) -> int:
        offset = self._alloc(PROP_BT_SIZE + len(name) + 1)
        raw = name.encode()
        data = self._data()
        struct.pack_into("<I", data, offset, len(raw))
        data[offset + PROP_BT_SIZE : offset + PROP_BT_SIZE + len(raw) + 1] = raw + b"\x00"
        return offset

    def _new_prop_info(self, name: str, value: str) -> int:
        offset = self._alloc(PROP_INFO_SIZE + len(name) + 1)
        raw_name = name.encode()
        raw_value = value.encode()
        data = self._data()
        data[offset + PROP_INFO_SIZE : offset + PROP_INFO_SIZE + len(raw_name) + 1] = (
            raw_name + b"\x00"
        )

        if len(raw_value) > PROP_VALUE_MAX:
            # libc keeps values longer than PROP_VALUE_MAX out of line: the
            # inline area holds an error message and an offset to the real
            # value, measured from this entry rather than from data_.
            value_offset = self._alloc(len(raw_value) + 1)
            data[value_offset : value_offset + len(raw_value)] = raw_value
            struct.pack_into(
                "<I", data, offset, (len(PROP_LONG_ERROR) << 24) | PROP_LONG_FLAG
            )
            data[offset + 4 : offset + 4 + PROP_LONG_ERROR_SIZE] = PROP_LONG_ERROR.ljust(
                PROP_LONG_ERROR_SIZE, b"\x00"
            )
            struct.pack_into("<I", data, offset + 4 + PROP_LONG_ERROR_SIZE,
                             value_offset - offset)
            return offset

        # serial's top byte is the value length; the reader uses it instead of
        # strlen, which is what makes a read wait-free.
        struct.pack_into("<I", data, offset, (len(raw_value) & 0xFF) << 24)
        data[offset + 4 : offset + 4 + len(raw_value)] = raw_value
        data[offset + 4 + len(raw_value)] = 0
        return offset

    def _prop_bt_name(self, offset: int) -> str:
        namelen = self._u32(offset)
        raw = bytes(self._data()[offset + PROP_BT_SIZE : offset + PROP_BT_SIZE + namelen])
        return raw.decode()

    @staticmethod
    def _cmp(one: str, two: str) -> int:
        """cmp_prop_name: length first, then bytes."""
        if len(one) != len(two):
            return -1 if len(one) < len(two) else 1
        return (one > two) - (one < two)

    def _find_or_add_bt(self, node: int, piece: str) -> int:
        current = node
        while True:
            other = self._prop_bt_name(current)
            order = self._cmp(piece, other)
            if order == 0:
                return current
            field = 8 if order < 0 else 12  # left, right
            child = self._u32(current + field)
            if child:
                current = child
                continue
            new = self._new_prop_bt(piece)
            self._set_u32(current + field, new)
            return new

    def add(self, name: str, value: str) -> None:
        if len(name) > PROP_NAME_MAX:
            raise ValueError(f"property name too long: {name}")

        # Mirrors prop_area::find_property with alloc_if_needed set.
        current = 0  # the root node, at data_ + 0
        remaining = name
        while True:
            sep = remaining.find(".")
            has_child = sep != -1
            piece = remaining[:sep] if has_child else remaining
            if not piece:
                raise ValueError(f"empty property name segment in {name}")

            children = self._u32(current + 16)
            if children:
                root = children
            else:
                root = self._new_prop_bt(piece)
                self._set_u32(current + 16, root)

            current = self._find_or_add_bt(root, piece)
            if not has_child:
                break
            remaining = remaining[sep + 1 :]

        info_offset = self._new_prop_info(name, value)
        self._set_u32(current + 4, info_offset)
        self.index[name] = info_offset

    def finish(self) -> bytes:
        struct.pack_into("<I", self.buf, 0, self.used)  # bytes_used_
        struct.pack_into("<I", self.buf, 4, 0)  # serial_
        struct.pack_into("<I", self.buf, 8, PROP_AREA_MAGIC)
        struct.pack_into("<I", self.buf, 12, PROP_AREA_VERSION)
        return bytes(self.buf)


class PropertyInfo:
    """The name -> context trie that libc reads from <dir>/property_info."""

    def __init__(self, contexts, types) -> None:
        self.contexts = list(contexts)
        self.types = list(types)
        self.buf = bytearray(INFO_HEADER_SIZE)
        self.buf += b"\x00" * TRIE_NODE_SIZE
        self.root_offset = INFO_HEADER_SIZE

    def _append(self, payload: bytes) -> int:
        offset = len(self.buf)
        self.buf += payload
        self.buf += b"\x00" * (align(len(self.buf)) - len(self.buf))
        return offset

    def _string_table(self, strings):
        """count, offsets, then the strings. Strings come first so the offsets
        are known in one pass."""
        base = len(self.buf) + 4 + 4 * len(strings)
        offsets = []
        blob = b""
        for item in strings:
            offsets.append(base + len(blob))
            blob += item.encode() + b"\x00"
        start = self._append(struct.pack(f"<{1 + len(strings)}I", len(strings), *offsets))
        self.buf += blob
        self.buf += b"\x00" * (align(len(self.buf)) - len(self.buf))
        return start

    def build(self, assignments) -> bytes:
        """assignments: list of (name, context_index, type_index)."""
        # The root's own name is empty and belongs to no context.
        root_entry_offset = self._append(
            struct.pack("<IIII", 0, 0, NO_INDEX, NO_INDEX)
        )

        # Exact-match entry offsets must be known before the array is written, so
        # the entries are laid out first and the array is patched afterwards.
        entry_offsets = []
        for name, context_index, type_index in assignments:
            name_offset = self._append(name.encode() + b"\x00")
            entry_offsets.append(
                self._append(
                    struct.pack(
                        "<IIII", name_offset, len(name.encode()), context_index, type_index
                    )
                )
            )

        array_offset = self._append(
            struct.pack(f"<{len(entry_offsets)}I", *entry_offsets)
            if entry_offsets
            else b""
        )

        contexts_offset = self._string_table(self.contexts)
        types_offset = self._string_table(self.types)

        struct.pack_into(
            "<IIIIII",
            self.buf,
            0,
            INFO_CURRENT_VERSION,
            INFO_MINIMUM_SUPPORTED_VERSION,
            len(self.buf),
            contexts_offset,
            types_offset,
            self.root_offset,
        )
        struct.pack_into(
            "<IIIIIII",
            self.buf,
            self.root_offset,
            root_entry_offset,
            0,  # num_child_nodes: none, so the reader stops at the root
            0,  # child_nodes
            0,  # num_prefixes
            0,  # prefix_entries
            len(entry_offsets),
            array_offset,
        )
        return bytes(self.buf)


def write_file(path: str, payload: bytes) -> None:
    with open(path, "wb") as handle:
        handle.write(payload)
    # libc refuses to map a file that is group or other writable, so the mode is
    # set explicitly rather than left to the umask.
    os.chmod(path, stat.S_IRUSR | stat.S_IRGRP | stat.S_IROTH)


def build(directory: str, properties, context: str = DEFAULT_CONTEXT) -> None:
    os.makedirs(directory, exist_ok=True)

    area = PropArea()
    for name, value in sorted(properties.items()):
        area.add(name, value)
    write_file(os.path.join(directory, context), area.finish())

    # Where each value lives, for android-property-service.py. Not part of the
    # area format, and ignored by libc, which opens only the files it knows.
    with open(os.path.join(directory, INDEX_FILE), "w") as index:
        for name, offset in sorted(area.index.items()):
            index.write(f"{name}\t{context}\t{offset}\n")

    # libc maps this one unconditionally and fails initialisation without it.
    write_file(os.path.join(directory, "properties_serial"), PropArea().finish())

    info = PropertyInfo(contexts=[context], types=["string"])
    assignments = [(name, 0, 0) for name in sorted(properties)]
    write_file(os.path.join(directory, "property_info"), info.build(assignments))

    print(f"wrote {len(properties)} properties into {directory}")
    print(f"  context {context}")
    for name in sorted(properties):
        print(f"  {name}={properties[name]}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("directory", help="where to write the area, e.g. /dev/__properties__")
    parser.add_argument(
        "--build-prop",
        action="append",
        default=[],
        metavar="FILE",
        help="seed values from a build.prop; repeatable, later files win",
    )
    parser.add_argument(
        "--set",
        action="append",
        default=[],
        metavar="NAME=VALUE",
        help="add or override a property; repeatable",
    )
    args = parser.parse_args()

    # The image's own build.prop is the authority on the product identity and
    # the SDK level, so where it defines a property Mosaic also defines, its
    # value wins. Only those keys are taken: an entire build.prop would not fit
    # in one 128 KiB area, and the rest read as unset, which is what a property
    # with no value does anyway.
    properties = dict(DEFAULT_PROPERTIES)
    for path in args.build_prop:
        from_image = read_build_prop(path)
        for name in properties:
            if name in from_image:
                properties[name] = from_image[name]
    for item in args.set:
        if "=" not in item:
            parser.error(f"expected NAME=VALUE, got {item!r}")
        name, value = item.split("=", 1)
        properties[name] = value

    build(args.directory, properties)
    return 0


if __name__ == "__main__":
    sys.exit(main())
