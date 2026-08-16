#!/usr/bin/env python3
"""Boot an LLVM-built xv6 RISC-V image and exercise its ordinary shell."""

from __future__ import annotations

import argparse
import os
import select
import signal
import subprocess
import time
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--fs-image", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--hard-deadline", type=float, default=120)
    return parser.parse_args()


class Xv6Runner:
    def __init__(self, command: list[str], log_path: Path, hard_deadline: float):
        log_path.parent.mkdir(parents=True, exist_ok=True)
        self.log_file = log_path.open("ab", buffering=0)
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        if self.process.stdin is None or self.process.stdout is None:
            raise RuntimeError("failed to create QEMU pipes")
        self.deadline = time.monotonic() + hard_deadline
        self.buffer = bytearray()
        self.emit(f"[xv6-qemu] command: {' '.join(command)}")

    def emit(self, message: str) -> None:
        line = (message + "\n").encode()
        self.log_file.write(line)
        print(message, flush=True)

    def _read(self, timeout: float) -> None:
        remaining = min(timeout, self.deadline - time.monotonic())
        if remaining <= 0:
            raise TimeoutError("xv6 QEMU hard deadline expired")
        readable, _, _ = select.select(
            [self.process.stdout.fileno()], [], [], remaining
        )
        if not readable:
            raise TimeoutError("timed out waiting for xv6 console output")
        data = os.read(self.process.stdout.fileno(), 4096)
        if not data:
            raise RuntimeError(
                f"xv6 QEMU exited early with status {self.process.poll()}"
            )
        self.log_file.write(data)
        os.write(1, data)
        self.buffer.extend(data.replace(b"\r", b""))

    def expect(self, expected: bytes, timeout: float = 30) -> bytes:
        local_deadline = min(self.deadline, time.monotonic() + timeout)
        while True:
            index = self.buffer.find(expected)
            if index >= 0:
                end = index + len(expected)
                consumed = bytes(self.buffer[:end])
                del self.buffer[:end]
                return consumed
            remaining = local_deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"did not observe xv6 output {expected!r}")
            self._read(remaining)

    def command(self, command: str, expected: bytes | None = None) -> None:
        self.buffer.clear()
        self.process.stdin.write((command + "\n").encode())
        self.process.stdin.flush()
        if expected is not None:
            self.expect(expected)
        self.expect(b"$ ")
        self.emit(f"[xv6-qemu] PASS command: {command}")

    def close(self) -> None:
        if self.process.poll() is None:
            try:
                self.process.stdin.write(b"\x01x")
                self.process.stdin.flush()
                self.process.wait(timeout=10)
            except (BrokenPipeError, subprocess.TimeoutExpired):
                os.killpg(self.process.pid, signal.SIGTERM)
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(self.process.pid, signal.SIGKILL)
                    self.process.wait(timeout=5)
        self.log_file.close()


def main() -> int:
    args = parse_args()
    if args.hard_deadline <= 0:
        raise SystemExit("--hard-deadline must be positive")
    for path in (args.kernel, args.fs_image):
        if not path.is_file():
            raise SystemExit(f"missing xv6 artifact: {path}")
    command = [
        "qemu-system-riscv64",
        "-machine",
        "virt",
        "-bios",
        "none",
        "-kernel",
        str(args.kernel),
        "-m",
        "128M",
        "-smp",
        "2",
        "-nographic",
        "-global",
        "virtio-mmio.force-legacy=false",
        "-drive",
        f"file={args.fs_image},if=none,format=raw,id=x0",
        "-device",
        "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0",
    ]
    runner = Xv6Runner(command, args.log, args.hard_deadline)
    try:
        runner.expect(b"init: starting sh")
        runner.expect(b"$ ")
        runner.emit("[xv6-qemu] PASS booted to xv6 shell")
        runner.command("mkdir accept")
        runner.command("echo XV6_RISCV_QEMU_OK > accept/boot-marker")
        runner.command("cat accept/boot-marker", b"XV6_RISCV_QEMU_OK")
        runner.command("echo alpha beta gamma > accept/data")
        runner.command("cat accept/data", b"alpha beta gamma")
        runner.command("grep beta accept/data", b"alpha beta gamma")
        runner.command("wc accept/data", b"1 3 17 accept/data")
        runner.command("ls accept", b"data")
        runner.emit("[xv6-qemu] ALL XV6 RISC-V BOOT TESTS PASSED")
        return 0
    finally:
        runner.close()


if __name__ == "__main__":
    raise SystemExit(main())
