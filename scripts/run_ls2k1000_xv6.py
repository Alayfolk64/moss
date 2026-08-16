#!/usr/bin/env python3
"""Boot the LS2K1000LA xv6 port with a hardware-watchdog safety net."""

from __future__ import annotations

import argparse
import time

from run_ls2k1000_serial_tftp import SerialRunner


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("image", help="TFTP filename of the flat xv6 image")
    parser.add_argument(
        "--device",
        default="/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0",
    )
    parser.add_argument("--log", required=True)
    return parser.parse_args()


def expect(runner: SerialRunner, marker: bytes, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    tail = b""
    while time.monotonic() < deadline:
        tail = (tail + runner.read_and_emit(4096))[-131072:]
        if marker in tail:
            return
        if b"[root@LS-GD" in tail and b"#" in tail:
            raise RuntimeError(f"factory Linux returned before marker {marker!r}")
    raise TimeoutError(f"timed out waiting for xv6 marker {marker!r}")


def xv6_command(
    runner: SerialRunner, command: str, expected: bytes | None = None
) -> None:
    runner.emit_line(f"[xv6-ls2k1000la] >>> {command}")
    runner.write(command.encode("ascii") + b"\r")
    if expected is not None:
        expect(runner, expected, 10)
    expect(runner, b"$ ", 10)
    runner.emit_line(f"[xv6-ls2k1000la] PASS command: {command}")


def wait_for_automatic_reset(runner: SerialRunner) -> None:
    runner.emit_line(
        "[xv6-ls2k1000la] waiting for the pre-handoff hardware watchdog reset"
    )
    runner.wait_for_linux_shell(150)
    runner.emit_line(
        "[xv6-ls2k1000la] PASS hardware watchdog returned the board to factory Linux"
    )


def main() -> int:
    args = parse_args()
    with open(args.log, "ab", buffering=0) as log_file:
        runner = SerialRunner(args.device, log_file)
        handed_off = False
        watchdog_armed = False
        factory_returned = False
        try:
            runner.wait_for_linux_shell(120)
            runner.emit_line("[xv6-ls2k1000la] >>> Linux: sync; /sbin/reboot -f")
            runner.write(b"sync; /sbin/reboot -f\r")
            runner.stop_uboot_autoboot()
            commands = (
                # LS2X watchdog: 125 MHz APB count for a 30-second reset.
                "mw.l 0x1fe27030 0",
                "mw.l 0x1fe27038 0xdf847580",
                "mw.l 0x1fe27030 2",
                "mw.l 0x1fe27034 1",
                "setenv ipaddr 192.168.88.2",
                "setenv serverip 192.168.88.1",
                "setenv netmask 255.255.255.0",
                "setenv ethact ethernet@40040000",
                f"tftpboot 0x9000000098000000 {args.image}",
            )
            for index, command in enumerate(commands):
                runner.command(command, 30 if command.startswith("tftpboot") else 5)
                if index == 3:
                    watchdog_armed = True

            runner.emit_line(
                "[xv6-ls2k1000la] >>> go 0x9000000098000000"
            )
            runner.write(b"go 0x9000000098000000\r")
            handed_off = True
            expect(runner, b"init: starting sh", 20)
            expect(runner, b"$ ", 5)
            runner.emit_line(
                "[xv6-ls2k1000la] PASS booted LLVM xv6 to the board shell"
            )
            xv6_command(runner, "mkdir accept")
            xv6_command(
                runner,
                "echo XV6_LOONGARCH_LS2K1000LA_OK > accept/boot-marker",
            )
            xv6_command(
                runner,
                "cat accept/boot-marker",
                b"XV6_LOONGARCH_LS2K1000LA_OK",
            )
            xv6_command(runner, "echo alpha beta gamma > accept/data")
            xv6_command(runner, "grep beta accept/data", b"alpha beta gamma")
            xv6_command(runner, "wc accept/data", b"1 3 17 accept/data")
            xv6_command(runner, "ls accept", b"boot-marker")
            wait_for_automatic_reset(runner)
            factory_returned = True
            runner.emit_line(
                "[xv6-ls2k1000la] COMPLETE LLVM build/boot/shell/reset acceptance passed"
            )
            return 0
        except Exception as error:
            runner.emit_line(f"[xv6-ls2k1000la] FAIL {type(error).__name__}: {error}")
            if (handed_off or watchdog_armed) and not factory_returned:
                try:
                    wait_for_automatic_reset(runner)
                    factory_returned = True
                    runner.emit_line(
                        "[xv6-ls2k1000la] ABANDONED boot attempt; board recovered automatically"
                    )
                except Exception as reset_error:
                    runner.emit_line(
                        "[xv6-ls2k1000la] ABANDONED board: automatic reset was not observed; "
                        f"do not attempt a manual reset ({reset_error})"
                    )
            return 2
        finally:
            runner.close()


if __name__ == "__main__":
    raise SystemExit(main())
