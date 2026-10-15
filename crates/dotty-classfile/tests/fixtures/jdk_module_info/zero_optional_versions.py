#!/usr/bin/env python3
"""Set Module.module_version_index and all requires_version_index fields to zero."""

from pathlib import Path
import struct
import sys


def u2(data, offset):
    return struct.unpack_from(">H", data, offset)[0]


def u4(data, offset):
    return struct.unpack_from(">I", data, offset)[0]


def skip_member(data, offset):
    attribute_count = u2(data, offset + 6)
    offset += 8
    for _ in range(attribute_count):
        length = u4(data, offset + 2)
        offset += 6 + length
    return offset


def main(source, destination):
    data = bytearray(Path(source).read_bytes())
    if data[:4] != b"\xca\xfe\xba\xbe":
        raise ValueError("not a class file")

    constant_pool_count = u2(data, 8)
    offset = 10
    utf8 = {}
    index = 1
    while index < constant_pool_count:
        tag = data[offset]
        offset += 1
        if tag == 1:
            length = u2(data, offset)
            offset += 2
            utf8[index] = data[offset : offset + length].decode("ascii")
            offset += length
        elif tag in (3, 4):
            offset += 4
        elif tag in (5, 6):
            offset += 8
            index += 1
        elif tag in (7, 8, 16, 19, 20):
            offset += 2
        elif tag in (9, 10, 11, 12, 17, 18):
            offset += 4
        elif tag == 15:
            offset += 3
        else:
            raise ValueError(f"unexpected constant-pool tag {tag}")
        index += 1

    offset += 6
    interface_count = u2(data, offset)
    offset += 2 + 2 * interface_count
    field_count = u2(data, offset)
    offset += 2
    for _ in range(field_count):
        offset = skip_member(data, offset)
    method_count = u2(data, offset)
    offset += 2
    for _ in range(method_count):
        offset = skip_member(data, offset)

    attribute_count = u2(data, offset)
    offset += 2
    module_body = None
    for _ in range(attribute_count):
        name = utf8[u2(data, offset)]
        length = u4(data, offset + 2)
        body = offset + 6
        if name == "Module":
            module_body = body
            break
        offset = body + length
    if module_body is None:
        raise ValueError("class file has no Module attribute")

    struct.pack_into(">H", data, module_body + 4, 0)
    requires_count_offset = module_body + 6
    requires_count = u2(data, requires_count_offset)
    entry = requires_count_offset + 2
    for _ in range(requires_count):
        struct.pack_into(">H", data, entry + 4, 0)
        entry += 6

    Path(destination).write_bytes(data)


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]))
