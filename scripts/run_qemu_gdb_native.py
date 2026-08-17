#!/usr/bin/env python3
"""Run the native GDB demo inside one QEMU MOSS guest."""

from __future__ import annotations

import argparse
import os
import re
import selectors
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import BinaryIO

from gdb_native_workload import ensure_gdb_packages, run_gdb_native_test


class QemuRunner:
    def __init__(
        self, command: list[str], log_file: BinaryIO, hard_deadline: float
    ) -> None:
        self.log_file = log_file
        self.deadline = time.monotonic() + hard_deadline
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            bufsize=0,
            start_new_session=True,
        )
        if self.process.stdin is None or self.process.stdout is None:
            raise RuntimeError("QEMU pipes were not created")
        self.input = self.process.stdin
        self.output = self.process.stdout
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.output, selectors.EVENT_READ)

    def close(self) -> None:
        self.selector.close()
        self.input.close()
        self.output.close()

    def emit(self, data: bytes) -> None:
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
        self.log_file.write(data)
        self.log_file.flush()

    def emit_line(self, message: str) -> None:
        self.emit(f"\n{message}\n".encode())

    def remaining(self, requested: float) -> float:
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("QEMU GDB hard deadline expired")
        return min(requested, remaining)

    def write(self, data: bytes) -> None:
        if self.process.poll() is not None:
            raise RuntimeError(f"QEMU exited early: {self.process.returncode}")
        self.input.write(data)
        self.input.flush()

    def read_and_emit(self, size: int = 4096, timeout: float = 0.05) -> bytes:
        if self.process.poll() is not None:
            timeout = 0
        events = self.selector.select(self.remaining(timeout) if timeout else 0)
        if not events:
            return b""
        data = os.read(self.output.fileno(), size)
        if data:
            self.emit(data)
        return data

    def wait_for_moss_shell(self, timeout: float = 240) -> None:
        deadline = time.monotonic() + self.remaining(timeout)
        tail = b""
        while time.monotonic() < deadline:
            chunk = self.read_and_emit()
            if b"\x1b[6n" in chunk:
                self.write(b"\x1b[1;1R")
            tail = (tail + chunk)[-16384:]
            if b"starry:~#" in tail:
                rc, output = self.shell_command("echo QEMU_GDB_SHELL_READY", 10)
                if rc == 0 and b"QEMU_GDB_SHELL_READY" in output:
                    self.emit_line("[qemu-gdb] PASS MOSS shell ready")
                    return
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"QEMU exited before MOSS shell: {self.process.returncode}"
                )
        raise TimeoutError("MOSS shell did not become ready")

    def shell_command(
        self, line: str, timeout: float = 20
    ) -> tuple[int, bytes]:
        self.emit_line(f"[qemu-gdb] >>> MOSS shell: {line}")
        self.write(b"\x03\r")
        prompt_deadline = time.monotonic() + self.remaining(0.5)
        while time.monotonic() < prompt_deadline:
            chunk = self.read_and_emit()
            if b"\x1b[6n" in chunk:
                self.write(b"\x1b[1;1R")
        command = (
            line
            + "; __moss_gdb_rc=$?; printf '\\n__MOSS_GDB_RC_%s__\\n' "
            + '"$__moss_gdb_rc"\r'
        )
        self.write(command.encode("ascii"))
        deadline = time.monotonic() + self.remaining(timeout)
        tail = b""
        marker = re.compile(rb"__MOSS_GDB_RC_(\d+)__")
        while time.monotonic() < deadline:
            chunk = self.read_and_emit()
            if b"\x1b[6n" in chunk:
                self.write(b"\x1b[1;1R")
            tail = (tail + chunk)[-2_097_152:]
            match = marker.search(tail)
            if match is not None:
                return int(match.group(1)), tail
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"QEMU exited during command: {self.process.returncode}: {line}"
                )
        raise TimeoutError(f"MOSS shell command timed out: {line}")

    def shutdown(self, timeout: float = 300) -> int:
        self.emit_line("[qemu-gdb] syncing, leaving shell, and powering off")
        self.write(b"\x03\r")
        time.sleep(0.2)
        self.write(b"sync; exit\r")
        deadline = time.monotonic() + self.remaining(timeout)
        while time.monotonic() < deadline:
            self.read_and_emit(timeout=0.1)
            status = self.process.poll()
            if status is not None:
                while self.read_and_emit(timeout=0):
                    pass
                return status
        raise TimeoutError("QEMU did not exit after MOSS shell ended")

    def terminate(self) -> None:
        if self.process.poll() is not None:
            return
        self.emit_line("[qemu-gdb] terminating QEMU process group")
        os.killpg(self.process.pid, signal.SIGTERM)
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(self.process.pid, signal.SIGKILL)
            self.process.wait(timeout=10)


def qemu_command(args: argparse.Namespace) -> list[str]:
    common = [
        "-machine",
        "virt",
        "-kernel",
        str(args.kernel),
        "-m",
        args.memory,
        "-smp",
        str(args.cpus),
        "-nographic",
        "-drive",
        f"file={args.image},if=none,format=raw,id=x0",
        "-no-reboot",
        "-rtc",
        "base=utc",
    ]
    if args.arch == "rv":
        return [
            "qemu-system-riscv64",
            *common[:2],
            "-bios",
            "default",
            *common[2:],
            "-device",
            "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0",
        ]
    return [
        "qemu-system-loongarch64",
        *common,
        "-accel",
        "tcg,thread=multi,tb-size=512",
        "-device",
        "virtio-blk-pci,drive=x0",
    ]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--arch", choices=("rv", "la"), required=True)
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--hard-deadline", type=float, default=900)
    parser.add_argument("--install-only", action="store_true")
    parser.add_argument("--memory", default="2G")
    parser.add_argument("--cpus", type=int, default=2)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    args.log.parent.mkdir(parents=True, exist_ok=True)
    command = qemu_command(args)
    with args.log.open("ab", buffering=0) as log_file:
        runner = QemuRunner(command, log_file, args.hard_deadline)
        try:
            runner.emit_line(f"[qemu-gdb] command: {' '.join(command)}")
            runner.wait_for_moss_shell()
            if args.install_only:
                ensure_gdb_packages(runner)
            else:
                run_gdb_native_test(runner)
            status = runner.shutdown()
            if status != 0:
                raise RuntimeError(f"QEMU exited with status {status}")
            phase = "installation" if args.install_only else "acceptance"
            runner.emit_line(f"[qemu-gdb] PASS {phase} boot shut down cleanly")
            return 0
        finally:
            runner.terminate()
            runner.close()


if __name__ == "__main__":
    raise SystemExit(main())
