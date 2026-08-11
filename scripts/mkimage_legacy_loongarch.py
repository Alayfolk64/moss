#!/usr/bin/env python3
"""Wrap a payload in a deterministic U-Boot legacy LoongArch image."""

from __future__ import annotations

import argparse
import struct
import zlib
from pathlib import Path


IH_MAGIC = 0x27051956
IH_OS_LINUX = 5
IH_ARCH_LOONGARCH = 0x1B
IH_TYPE_RAMDISK = 3
IH_COMP_GZIP = 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("payload", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument(
        "--name", default="MOSS LS2K1000 rescue initramfs", help="max 32 bytes"
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    payload = args.payload.read_bytes()
    image_name = args.name.encode("ascii")
    if len(image_name) > 32:
        raise SystemExit("image name exceeds the legacy 32-byte field")
    image_name = image_name.ljust(32, b"\0")

    data_crc = zlib.crc32(payload) & 0xFFFFFFFF
    header_without_crc = struct.pack(
        ">7I4B32s",
        IH_MAGIC,
        0,
        0,
        len(payload),
        0,
        0,
        data_crc,
        IH_OS_LINUX,
        IH_ARCH_LOONGARCH,
        IH_TYPE_RAMDISK,
        IH_COMP_GZIP,
        image_name,
    )
    header_crc = zlib.crc32(header_without_crc) & 0xFFFFFFFF
    header = bytearray(header_without_crc)
    struct.pack_into(">I", header, 4, header_crc)

    args.output.write_bytes(header + payload)


if __name__ == "__main__":
    main()
