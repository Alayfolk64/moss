#!/usr/bin/env python3
"""Boot a Linux legacy uImage on LS2K1000 through TFTP without installing it.

The test boots the competition root read-only with ``init=/bin/sh`` and arms
the board watchdog in U-Boot.  The watchdog deliberately is not fed, so the
factory Linux image is restored automatically by the next normal boot even if
the candidate kernel hangs.
"""

from __future__ import annotations

import argparse
import sys
import time

from run_ls2k1000_serial_tftp import SerialRunner


LOAD_ADDRESS = "0x9000000098000000"
WATCHDOG_COUNT_30S = "0xdf847580"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("image", help="TFTP filename of the Linux uImage")
    parser.add_argument("--device", default="/dev/ttyUSB0")
    parser.add_argument("--linux-timeout", type=float, default=600)
    parser.add_argument("--monitor-seconds", type=float, default=75)
    return parser.parse_args()


def monitor_candidate(runner: SerialRunner, seconds: float) -> bool:
    deadline = time.monotonic() + seconds
    tail = b""
    tests_sent = False

    while time.monotonic() < deadline:
        tail = (tail + runner.read_and_emit())[-16384:]
        init_shell_ready = (
            b"Run /bin/sh as init process" in tail and b"#" in tail
        )
        if not tests_sent and init_shell_ready:
            tests_sent = True
            runner.write(
                b"export PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:"
                b"/usr/sbin:/usr/bin:/sbin:/bin\r"
                b"echo __LINUX612_TEST_BEGIN__\r"
                b"uname -a\r"
                b"mount -t proc proc /proc 2>/dev/null || true\r"
                b"/usr/bin/timeout 1 /bin/true; echo timeout_rc=$?\r"
                b"rustc --version; echo rustc_rc=$?\r"
                b"cargo --version; echo cargo_rc=$?\r"
                b"echo __LINUX612_TEST_END__\r"
            )

    return tests_sent and b"__LINUX612_TEST_END__" in tail


def main() -> None:
    args = parse_args()
    runner = SerialRunner(args.device)
    try:
        runner.wait_for_linux_shell(args.linux_timeout)
        runner.linux_command("umount /dev/sda2 2>/dev/null || true; sync")
        print(
            "\n[linux-uimage] rebooting into a read-only, watchdog-bounded test",
            flush=True,
        )
        runner.write(b"/sbin/reboot -f\r")
        runner.stop_uboot_autoboot()

        bootargs = (
            "earlycon console=ttyS0,115200 root=/dev/sda2 "
            "rootfstype=ext4 ro rootwait init=/bin/sh panic=10"
        )
        commands = [
            ("setenv ipaddr 192.168.88.2", 5),
            ("setenv serverip 192.168.88.1", 5),
            ("setenv netmask 255.255.255.0", 5),
            ("setenv ethact ethernet@40040000", 5),
            (f"setenv bootargs {bootargs}", 5),
            (f"tftpboot {LOAD_ADDRESS} {args.image}", 30),
            ("mw.l 0x1fe27030 0", 5),
            (f"mw.l 0x1fe27038 {WATCHDOG_COUNT_30S}", 5),
            ("mw.l 0x1fe27030 2", 5),
            ("mw.l 0x1fe27034 1", 5),
        ]
        for command, timeout in commands:
            runner.command(command, timeout)

        bootm = f"bootm {LOAD_ADDRESS}"
        print(f"\n[linux-uimage] >>> {bootm}", flush=True)
        runner.write(bootm.encode("ascii") + b"\r")
        passed = monitor_candidate(runner, args.monitor_seconds)
        print(f"\n[linux-uimage] userspace-smoke={'PASS' if passed else 'FAIL'}")
        if not passed:
            raise SystemExit(1)
    finally:
        runner.close()


if __name__ == "__main__":
    main()
