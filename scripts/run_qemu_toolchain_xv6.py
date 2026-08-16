#!/usr/bin/env python3
"""Run native toolchains and the pinned LLVM xv6 build on QEMU MOSS."""

from __future__ import annotations

import argparse
import signal
from pathlib import Path

from ls2k1000_apk_mariadb import require_shell_command
from ls2k1000_toolchain_xv6 import run_toolchain_xv6_test
from run_qemu_apk_mariadb import QemuRunner, qemu_command


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--arch", choices=("rv", "la"), required=True)
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--hard-deadline", type=float, default=3600)
    parser.add_argument("--install-only", action="store_true")
    parser.add_argument("--memory", default="2G")
    parser.add_argument("--cpus", type=int, default=2)
    return parser.parse_args()


def ensure_toolchain_packages(runner: QemuRunner) -> None:
    installed_rc, _ = runner.shell_command(
        "apk info -e g++ >/dev/null && apk info -e clang19 >/dev/null && "
        "apk info -e llvm19 >/dev/null && "
        "(apk info -e lld >/dev/null || apk info -e lld20 >/dev/null) && "
        "apk info -e perl >/dev/null && apk info -e git >/dev/null && "
        "apk info -e rust >/dev/null && apk info -e cargo >/dev/null",
        timeout=60,
    )
    if installed_rc == 0:
        runner.emit_line("[qemu-toolchain] PASS toolchain packages already installed")
        return
    require_shell_command(
        runner,
        "staged toolchain package signatures",
        "test \"$(find /var/cache/moss-toolchain -maxdepth 1 "
        "-name '*.apk' | wc -l)\" -ge 40 && "
        "apk verify /var/cache/moss-toolchain/*.apk",
        timeout=600,
    )
    require_shell_command(
        runner,
        "offline Alpine toolchain installation",
        "apk --no-network add /var/cache/moss-toolchain/*.apk && "
        "rm -rf /var/cache/moss-toolchain && sync",
        timeout=1500,
    )


def main() -> int:
    args = parse_args()
    if args.hard_deadline <= 0:
        raise SystemExit("--hard-deadline must be positive")
    if args.cpus <= 0:
        raise SystemExit("--cpus must be positive")
    args.log.parent.mkdir(parents=True, exist_ok=True)
    command = qemu_command(args)

    with args.log.open("ab", buffering=0) as log_file:
        runner = QemuRunner(command, log_file, args.hard_deadline)
        previous_sigterm = signal.getsignal(signal.SIGTERM)

        def deadline_signal(_signum: int, _frame: object) -> None:
            raise TimeoutError("QEMU toolchain runner received SIGTERM")

        signal.signal(signal.SIGTERM, deadline_signal)
        shutdown_started = False
        try:
            runner.emit_line(f"[qemu-toolchain] command: {' '.join(command)}")
            runner.wait_for_moss_shell()
            if args.install_only:
                ensure_toolchain_packages(runner)
                runner.emit_line("[qemu-toolchain] PASS APK installation phase")
            else:
                run_toolchain_xv6_test(runner)
            shutdown_started = True
            status = runner.shutdown()
            if status != 0:
                raise RuntimeError(f"QEMU exited with status {status}")
            phase = "installation" if args.install_only else "acceptance"
            runner.emit_line(
                f"[qemu-toolchain] PASS {phase} QEMU exited after MOSS "
                "filesystem shutdown"
            )
            return 0
        except BaseException:
            if not shutdown_started:
                runner.emit_line(
                    "[qemu-toolchain] acceptance failed; attempting sync and "
                    "normal MOSS filesystem shutdown"
                )
                try:
                    runner.shell_command("sync", timeout=300)
                    cleanup_status = runner.shutdown()
                    runner.emit_line(
                        "[qemu-toolchain] diagnostic shutdown status="
                        f"{cleanup_status}"
                    )
                except BaseException as cleanup_error:
                    runner.emit_line(
                        "[qemu-toolchain] diagnostic shutdown failed: "
                        f"{cleanup_error!r}"
                    )
            raise
        finally:
            signal.signal(signal.SIGTERM, previous_sigterm)
            runner.terminate()
            runner.close()


if __name__ == "__main__":
    raise SystemExit(main())
