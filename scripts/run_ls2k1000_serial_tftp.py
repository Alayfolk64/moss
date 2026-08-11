#!/usr/bin/env python3
"""Reboot an LS2K1000 board and run one MOSS image through U-Boot/TFTP.

The factory U-Boot uses a zero-second autoboot delay and clears early serial
input.  A short continuous stream of the documented `c` stop key spans that
clear without changing the persistent U-Boot environment.
"""

from __future__ import annotations

import argparse
import re
import signal
import sys
import threading
import time
from typing import BinaryIO

import serial


class SerialRunner:
    def __init__(self, device: str, log_file: BinaryIO | None = None) -> None:
        self.serial = serial.Serial(
            device, 115200, timeout=0.01, write_timeout=1
        )
        self.log_file = log_file
        self.write_lock = threading.Lock()

    def close(self) -> None:
        self.serial.close()

    def write(self, data: bytes) -> None:
        with self.write_lock:
            self.serial.write(data)

    def emit(self, data: bytes) -> None:
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
        if self.log_file is not None:
            self.log_file.write(data)
            self.log_file.flush()

    def emit_line(self, message: str) -> None:
        self.emit(f"\n{message}\n".encode())

    def read_and_emit(self, size: int = 512) -> bytes:
        data = self.serial.read(size)
        if data:
            self.emit(data)
        return data

    def wait_for_linux_shell(self, timeout: float = 45) -> None:
        deadline = time.monotonic() + timeout
        tail = b""
        login_seen_at: float | None = None
        login_sent = False
        self.write(b"\r")
        while time.monotonic() < deadline:
            tail = (tail + self.read_and_emit())[-4096:]
            # Auto-login images can print the login banner and the final root
            # prompt in one serial read.  Accept the prompt before handling a
            # standalone login request so it is not discarded with `tail`.
            if b"[root@LS-GD" in tail and b"#" in tail:
                return
            if b"login:" in tail and login_seen_at is None:
                login_seen_at = time.monotonic()
            # This board normally auto-logs in root.  Give that service time
            # to print the prompt before injecting a username; otherwise the
            # queued "root" is executed as a shell command after auto-login.
            if (
                login_seen_at is not None
                and not login_sent
                and time.monotonic() - login_seen_at >= 2
            ):
                self.write(b"root\r")
                tail = b""
                login_sent = True
        raise TimeoutError("Linux root shell prompt not observed")

    def stop_uboot_autoboot(self, timeout: float = 20) -> None:
        stop_event = threading.Event()

        def send_stop_key() -> None:
            while not stop_event.is_set():
                self.write(b"c")
                time.sleep(0.002)

        sender = threading.Thread(target=send_stop_key, daemon=True)
        sender.start()
        deadline = time.monotonic() + timeout
        tail = b""
        try:
            while time.monotonic() < deadline:
                tail = (tail + self.read_and_emit())[-8192:]
                if b"=>" in tail:
                    return
            raise TimeoutError("U-Boot prompt not observed during stop request")
        finally:
            stop_event.set()
            sender.join(timeout=1)
            self.write(b"\x03\r")
            time.sleep(0.1)
            self.read_and_emit(4096)

    def linux_command(self, line: str, timeout: float = 20) -> bytes:
        marker = b"__SERIAL_RUN_DONE__"
        self.emit_line(f"[serial-run] >>> Linux: {line}")
        # Construct the marker in the shell so its literal bytes are absent
        # from terminal input echo.  Waiting on a literal appended marker can
        # otherwise return before the command has even started.
        self.write(
            line.encode("ascii")
            + b"; printf '\\n__SERIAL_RUN_%s__\\n' DONE\r"
        )
        deadline = time.monotonic() + timeout
        tail = b""
        while time.monotonic() < deadline:
            tail = (tail + self.read_and_emit())[-4096:]
            if marker in tail:
                return tail
        raise TimeoutError(f"Linux command timed out: {line}")

    def command(self, line: str, timeout: float = 20) -> bytes:
        self.emit_line(f"[serial-run] >>> {line}")
        self.write(line.encode("ascii") + b"\r")
        deadline = time.monotonic() + timeout
        tail = b""
        while time.monotonic() < deadline:
            tail = (tail + self.read_and_emit())[-65536:]
            if b"=> " in tail:
                return tail
        raise TimeoutError(f"U-Boot command timed out: {line}")

    def monitor(self, seconds: float) -> None:
        deadline = time.monotonic() + seconds
        previous_sigint = signal.getsignal(signal.SIGINT)

        def forward_sigint(_signum: int, _frame: object) -> None:
            # A terminal Ctrl+C normally kills the host-side Python process
            # before byte 0x03 reaches the UART. During the MOSS monitoring
            # phase it must target the board's foreground process group.
            self.emit_line("[serial-run] forwarding host Ctrl+C as UART 0x03")
            self.write(b"\x03")

        signal.signal(signal.SIGINT, forward_sigint)
        try:
            while time.monotonic() < deadline:
                self.read_and_emit()
        finally:
            signal.signal(signal.SIGINT, previous_sigint)


def discover_swap_extents(runner: SerialRunner) -> list[tuple[int, int, int]]:
    """Return /swapfile-moss extents as (logical_page, disk_lba, pages).

    Linux resolves the ext4 inode while it owns the filesystem.  MOSS receives
    only the resulting immutable sector map, so it never needs to interpret
    sda1 metadata and never guesses where the swap file happens to live.
    """
    output = runner.linux_command(
        "printf 'MOSS_SWAP_META sector=%s part_start=%s bytes=%s\\n' "
        '"$(blockdev --getss /dev/sda)" '
        '"$(cat /sys/class/block/sda1/start)" '
        '"$(stat -c %s /swapfile-moss)"; '
        "filefrag -b4096 -v /swapfile-moss",
        timeout=30,
    ).decode("ascii", errors="replace")

    meta = re.search(
        r"MOSS_SWAP_META sector=(\d+) part_start=(\d+) bytes=(\d+)", output
    )
    if meta is None:
        raise RuntimeError("cannot read /swapfile-moss geometry from factory Linux")
    sector_size, partition_start, file_bytes = map(int, meta.groups())
    if sector_size == 0 or 4096 % sector_size != 0:
        raise RuntimeError(f"unsupported SATA sector size {sector_size}")
    if file_bytes < 8192 or file_bytes % 4096:
        raise RuntimeError(f"invalid /swapfile-moss byte size {file_bytes}")

    extent_pattern = re.compile(
        r"^\s*\d+:\s+(\d+)\.\.\s*(\d+):\s+(\d+)\.\.\s*(\d+):\s+(\d+):",
        re.MULTILINE,
    )
    extents: list[tuple[int, int, int]] = []
    expected_logical = 0
    sectors_per_page = 4096 // sector_size
    for match in extent_pattern.finditer(output):
        logical_start, logical_end, physical_start, physical_end, length = map(
            int, match.groups()
        )
        if logical_start != expected_logical:
            raise RuntimeError(
                f"swapfile logical extent gap: expected {expected_logical}, "
                f"found {logical_start}"
            )
        if logical_end - logical_start + 1 != length:
            raise RuntimeError("swapfile logical extent length mismatch")
        if physical_end - physical_start + 1 != length:
            raise RuntimeError("swapfile physical extent length mismatch")
        disk_lba = partition_start + physical_start * sectors_per_page
        extents.append((logical_start, disk_lba, length))
        expected_logical += length

    file_pages = file_bytes // 4096
    if not extents or expected_logical != file_pages:
        raise RuntimeError(
            f"swapfile extent coverage {expected_logical} pages != file size {file_pages} pages"
        )
    runner.emit_line(
        f"[serial-run] validated /swapfile-moss: {file_pages} pages, "
        f"{len(extents)} extents; page 0 reserved for Linux swap header"
    )
    return extents


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("image", help="TFTP filename of the MOSS image")
    parser.add_argument("--device", default="/dev/ttyUSB0")
    parser.add_argument("--linux-timeout", type=float, default=600)
    parser.add_argument("--monitor-seconds", type=float, default=75)
    parser.add_argument(
        "--log",
        help="append both runner messages and raw serial bytes to this file",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    log_file = open(args.log, "ab", buffering=0) if args.log else None
    runner = SerialRunner(args.device, log_file)
    try:
        runner.wait_for_linux_shell(args.linux_timeout)
        # Keep the CPU feature line in every board log.  In particular, the
        # LoongArch ``ual`` flag decides whether userspace can execute
        # unaligned loads/stores directly or must take the ALE emulation path.
        runner.linux_command(
            "grep -i -m1 '^features' /proc/cpuinfo || true"
        )
        # Linux's board startup script may mount the competition partition.
        # Make the handoff explicit so MOSS never opens an ext4 filesystem that
        # Linux still has mounted or has not finished flushing.
        runner.linux_command("umount /dev/sda2 2>/dev/null || true; sync")
        # Capture the known-good Linux stmmac descriptor mode immediately
        # before handoff. CSR0 bit 7 is ATDS (eight-word descriptor stride),
        # while DMA_HW_FEATURE bit 24 advertises enhanced descriptors.
        runner.linux_command(
            "busybox devmem 0x40041000 32; busybox devmem 0x40041058 32"
        )
        swap_extents = discover_swap_extents(runner)
        runner.emit_line(
            "[serial-run] rebooting; continuous c stream spans U-Boot input flush"
        )
        runner.write(b"/sbin/reboot -f\r")
        runner.stop_uboot_autoboot()

        swap_fdt_commands = [
            (
                f"fdt set /chosen moss,swap-extents-{index} <"
                + " ".join(
                    f"0x{logical:x} 0x{lba:x} 0x{pages:x}"
                    for logical, lba, pages in swap_extents[start : start + 8]
                )
                + ">",
                5,
            )
            for index, start in enumerate(range(0, len(swap_extents), 8))
        ]
        commands = [
            # Cover TFTP and every FDT mutation as well as early MOSS boot.
            # The 125 MHz APB clock makes 0xdf847580 exactly 30 seconds.
            ("mw.l 0x1fe27030 0", 5),
            ("mw.l 0x1fe27038 0xdf847580", 5),
            ("mw.l 0x1fe27030 2", 5),
            ("mw.l 0x1fe27034 1", 5),
            ("setenv ipaddr 192.168.88.2", 5),
            ("setenv serverip 192.168.88.1", 5),
            ("setenv netmask 255.255.255.0", 5),
            ("setenv ethact ethernet@40040000", 5),
            ("tftpboot 0x900000000a000000 board.dtb", 20),
            ("fdt addr 0x900000000a000000", 5),
            ("fdt resize 4096", 5),
            ("fdt set /chosen bootargs root=/dev/sda2", 5),
            *swap_fdt_commands,
            (
                "fdt set /aliases ethernet0 /2k1000-soc/ethernet@40040000",
                5,
            ),
            (
                "fdt set /2k1000-soc/ahci@400e0000 "
                "ports-implemented <0x1>",
                5,
            ),
            # Record the factory NODE PLL and FREQSCALE state before every
            # handoff.  U-Boot reports only its configured MHz value; these
            # registers distinguish a PLL divider from runtime scaling.
            ("md.q 0x1fe00480 2", 5),
            ("md.q 0x1fe004d0 1", 5),
            (f"tftpboot 0x9000000098000000 {args.image}", 30),
        ]
        for command, timeout in commands:
            runner.command(command, timeout)

        go = "go 0x9000000098000040 0x900000000a000000"
        runner.emit_line(f"[serial-run] >>> {go}")
        runner.write(go.encode("ascii") + b"\r")
        runner.monitor(args.monitor_seconds)
    finally:
        runner.close()
        if log_file is not None:
            log_file.close()


if __name__ == "__main__":
    main()
