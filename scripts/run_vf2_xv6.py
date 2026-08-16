#!/usr/bin/env python3
"""Boot the LLVM-built JH7110 xv6 port on VF2 with automatic recovery."""

from __future__ import annotations

import argparse
from pathlib import Path
import sys
import time
from typing import BinaryIO

from run_vf2_gmac_test import SerialRunner
from run_vf2_serial_tftp import (
    BOARD_IP,
    HOST_IP,
    arm_uboot_watchdog,
    deploy_tftp,
    discover_interface,
    discover_serial,
    ensure_host_network,
    host_network_managed,
    run,
    set_host_network_managed,
    ssh_command,
    wait_for_linux_return,
    wait_for_ssh,
)


def emit(log_file: BinaryIO, message: str) -> None:
    data = f"[xv6-vf2] {message}\n".encode()
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()
    log_file.write(data)
    log_file.flush()


def expect(
    runner: SerialRunner,
    marker: bytes,
    timeout: float,
    *,
    occurrences: int = 1,
) -> bytes:
    deadline = time.monotonic() + timeout
    tail = b""
    while time.monotonic() < deadline:
        tail = (tail + runner.read())[-262144:]
        if tail.count(marker) >= occurrences:
            return tail
        if b"visionfive2 login:" in tail:
            raise RuntimeError(f"Debian returned before xv6 marker {marker!r}")
    raise TimeoutError(f"timed out waiting for xv6 marker {marker!r}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--fs", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--tftp-dir", type=Path, default=Path("/srv/tftp"))
    parser.add_argument("--serial")
    parser.add_argument("--interface")
    parser.add_argument("--username", default="vf2")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    kernel = args.kernel.resolve()
    fs_image = args.fs.resolve()
    for image in (kernel, fs_image):
        if not image.is_file() or image.stat().st_size == 0:
            raise SystemExit(f"missing xv6 image: {image}")
    if kernel.stat().st_size >= 0x0100_0000:
        raise SystemExit("kernel ELF is too large for the 0x48000000 load slot")
    if fs_image.stat().st_size > 0x0040_0000:
        raise SystemExit("fs.img exceeds the JH7110 port's 4 MiB RAM-disk mapping")

    args.log.parent.mkdir(parents=True, exist_ok=True)
    serial_device = args.serial or discover_serial()
    interface = args.interface or discover_interface()
    original_managed = host_network_managed(interface)
    runner: SerialRunner | None = None
    watchdog_armed = False
    linux_returned = False

    with args.log.open("ab", buffering=0) as log_file:
        try:
            run(["sudo", "-n", "true"])
            if original_managed:
                set_host_network_managed(interface, False)
            ensure_host_network(interface)
            wait_for_ssh(args.username, interface, log_file, 240)
            kernel_name, kernel_sha = deploy_tftp(kernel, args.tftp_dir)
            fs_name, fs_sha = deploy_tftp(fs_image, args.tftp_dir)
            emit(
                log_file,
                f"kernel={kernel} sha256={kernel_sha} tftp={kernel_name}",
            )
            emit(log_file, f"fs={fs_image} sha256={fs_sha} tftp={fs_name}")

            runner = SerialRunner(
                serial_device, 115200, log_file, label="xv6-vf2"
            )
            ssh_command(
                args.username,
                "sudo -n /usr/local/sbin/moss-vf2-relay reboot",
                log_file,
                timeout=30,
                allow_disconnect=True,
            )
            runner.wait_for_uboot(120)
            ensure_host_network(interface)
            arm_uboot_watchdog(runner)
            watchdog_armed = True
            for command in (
                f"setenv ipaddr {BOARD_IP}",
                f"setenv serverip {HOST_IP}",
                "setenv netmask 255.255.255.0",
                f"tftpboot 0x48000000 {kernel_name}",
                f"tftpboot 0x49000000 {fs_name}",
            ):
                if command.startswith("tftpboot"):
                    ensure_host_network(interface)
                runner.uboot_command(
                    command, 60 if command.startswith("tftpboot") else 5
                )

            runner.message(">>> U-Boot: bootelf 0x48000000")
            runner.write(b"\x15bootelf 0x48000000\r")
            expect(runner, b"init: starting sh", 15)
            runner.write(b"\r")
            expect(runner, b"$ ", 5)
            emit(log_file, "PASS LLVM xv6 reached its physical-board shell")

            command = (
                b"mkdir accept; echo alpha beta gamma > accept/data; "
                b"cat accept/data; grep beta accept/data; wc accept/data; "
                b"ls accept; echo XV6_VF2_ACCEPT_OK\r"
            )
            runner.write(command)
            output = expect(
                runner,
                b"XV6_VF2_ACCEPT_OK",
                10,
                occurrences=2,
            )
            if output.count(b"alpha beta gamma") < 3:
                raise RuntimeError("cat/grep output was not observed")
            if b"1 3 17 accept/data" not in output:
                raise RuntimeError("wc output was not observed")
            if b"\r\ndata\r\n" not in output and b"\ndata\n" not in output:
                raise RuntimeError("ls output was not observed")
            emit(
                log_file,
                "PASS mkdir/echo/cat/grep/wc/ls ordinary command workflow",
            )

            runner.write(b"reboot\r")
            wait_for_linux_return(runner, interface, 180)
            linux_returned = True
            wait_for_ssh(args.username, interface, log_file, 240)
            emit(
                log_file,
                "COMPLETE musl LLVM build/physical boot/commands/automatic reset passed",
            )
            return 0
        except BaseException as error:
            emit(log_file, f"FAIL {type(error).__name__}: {error}")
            if watchdog_armed and not linux_returned and runner is not None:
                try:
                    emit(log_file, "waiting only for the hardware watchdog recovery")
                    wait_for_linux_return(runner, interface, 180)
                    linux_returned = True
                    wait_for_ssh(args.username, interface, log_file, 240)
                    emit(
                        log_file,
                        "ABANDONED xv6 attempt; board recovered automatically",
                    )
                except BaseException as recovery_error:
                    emit(
                        log_file,
                        "ABANDONED board without manual reset: "
                        f"automatic recovery not observed ({recovery_error})",
                    )
            return 2
        finally:
            if runner is not None:
                runner.close()
            if original_managed:
                set_host_network_managed(interface, True)


if __name__ == "__main__":
    raise SystemExit(main())
