#!/usr/bin/env python3
"""Measure MOSS serial-console echo latency on an already booted board.

The test is intentionally board-neutral: point it at VF2 ``/dev/ttyACM0`` or
LS2K1000LA ``/dev/ttyUSB0`` after an interactive MOSS shell is ready. It does
not modify U-Boot state or reboot the board.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys
import time
from typing import BinaryIO

try:
    import serial
except ImportError as error:
    raise SystemExit("pyserial is required: python3 -m pip install pyserial") from error


class ConsoleProbe:
    def __init__(
        self, device: str, baud: int, log_file: BinaryIO | None = None
    ) -> None:
        self.port = serial.Serial(device, baud, timeout=0.002, write_timeout=2)
        self.log_file = log_file

    def close(self) -> None:
        self.port.close()

    def write(self, data: bytes) -> None:
        self.port.write(data)
        self.port.flush()

    def read(self, size: int = 4096) -> bytes:
        data = self.port.read(size)
        if data and self.log_file is not None:
            self.log_file.write(data)
            self.log_file.flush()
        if b"\x1b[6n" in data:
            # BusyBox ash asks a real terminal for cursor position before
            # redrawing an edited line. Automated probes must answer the DSR.
            self.write(b"\x1b[1;1R")
        return data

    def settle_shell(self) -> None:
        self.write(b"\x03\r")
        self.read_for(0.5)
        self.port.reset_input_buffer()

    def read_for(self, seconds: float) -> bytes:
        deadline = time.monotonic() + seconds
        output = bytearray()
        while time.monotonic() < deadline:
            output.extend(self.read())
        return bytes(output)

    def wait_for(self, marker: bytes, timeout: float) -> bytes:
        deadline = time.monotonic() + timeout
        tail = bytearray()
        while time.monotonic() < deadline:
            tail.extend(self.read())
            if marker in tail:
                return bytes(tail)
            if len(tail) > 131072:
                del tail[:-65536]
        raise TimeoutError(f"serial marker not observed: {marker!r}")

    def echo_samples(self, count: int, timeout: float) -> list[float]:
        samples: list[float] = []
        for _ in range(count):
            # The background load emits only "X" and line endings, so '~' is
            # unambiguous in both idle and loaded measurements.
            self.port.reset_input_buffer()
            started = time.monotonic_ns()
            self.write(b"~")
            self.wait_for(b"~", timeout)
            elapsed_ms = (time.monotonic_ns() - started) / 1_000_000
            samples.append(elapsed_ms)
            # Keep the shell's canonical input line empty between samples.
            self.write(b"\x7f")
            self.read_for(0.003)
        return samples

    def verify_paste(self, length: int, timeout: float) -> None:
        alphabet = b"0123456789abcdefghijklmnopqrstuvwxyz"
        payload = bytes(alphabet[index % len(alphabet)] for index in range(length))
        expected = hashlib.sha256(payload).hexdigest().encode()
        command = b"printf '%s' '" + payload + b"' | busybox sha256sum\r"
        self.port.reset_input_buffer()
        self.write(command)
        self.wait_for(expected, timeout)
        self.settle_shell()


def percentile(samples: list[float], percent: float) -> float:
    ordered = sorted(samples)
    index = max(0, min(len(ordered) - 1, int(len(ordered) * percent + 0.999) - 1))
    return ordered[index]


def summarize(name: str, samples: list[float]) -> dict[str, float | int | str]:
    return {
        "name": name,
        "samples": len(samples),
        "min_ms": round(min(samples), 3),
        "p50_ms": round(percentile(samples, 0.50), 3),
        "p95_ms": round(percentile(samples, 0.95), 3),
        "max_ms": round(max(samples), 3),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--serial", required=True, help="host serial device")
    parser.add_argument("--baud", type=int, default=115200)
    parser.add_argument("--samples", type=int, default=100)
    parser.add_argument("--sample-timeout", type=float, default=1.0)
    parser.add_argument("--idle-p95-ms", type=float, default=20.0)
    parser.add_argument("--idle-max-ms", type=float, default=50.0)
    parser.add_argument("--load-p95-ms", type=float, default=30.0)
    parser.add_argument("--load-max-ms", type=float, default=100.0)
    parser.add_argument("--skip-load", action="store_true")
    parser.add_argument("--skip-paste", action="store_true")
    parser.add_argument("--paste-lengths", type=int, nargs="+", default=[22, 262, 518])
    parser.add_argument("--log", type=Path)
    parser.add_argument("--json", type=Path, help="write machine-readable summary")
    return parser.parse_args()


def check_threshold(
    summary: dict[str, float | int | str], p95_limit: float, max_limit: float
) -> list[str]:
    failures: list[str] = []
    if float(summary["p95_ms"]) > p95_limit:
        failures.append(
            f"{summary['name']} p95 {summary['p95_ms']} ms exceeds {p95_limit} ms"
        )
    if float(summary["max_ms"]) > max_limit:
        failures.append(
            f"{summary['name']} max {summary['max_ms']} ms exceeds {max_limit} ms"
        )
    return failures


def main() -> None:
    args = parse_args()
    if args.samples <= 0:
        raise SystemExit("--samples must be positive")
    serial_path = Path(args.serial)
    if not serial_path.exists():
        raise SystemExit(f"missing serial device: {serial_path}")

    log_file = args.log.open("ab", buffering=0) if args.log else None
    probe: ConsoleProbe | None = None
    summaries: list[dict[str, float | int | str]] = []
    failures: list[str] = []
    paste_passed: list[int] = []
    try:
        probe = ConsoleProbe(args.serial, args.baud, log_file)
        probe.settle_shell()

        idle = summarize(
            "idle", probe.echo_samples(args.samples, args.sample_timeout)
        )
        summaries.append(idle)
        failures.extend(check_threshold(idle, args.idle_p95_ms, args.idle_max_ms))
        print(json.dumps(idle, ensure_ascii=False))

        if not args.skip_load:
            probe.settle_shell()
            probe.write(b"busybox yes X\r")
            probe.read_for(0.2)
            loaded = summarize(
                "continuous-output",
                probe.echo_samples(args.samples, args.sample_timeout),
            )
            summaries.append(loaded)
            failures.extend(
                check_threshold(loaded, args.load_p95_ms, args.load_max_ms)
            )
            print(json.dumps(loaded, ensure_ascii=False))
            probe.settle_shell()

        if not args.skip_paste:
            for length in args.paste_lengths:
                if length <= 0:
                    failures.append(f"invalid paste length: {length}")
                    continue
                probe.verify_paste(length, max(5.0, args.sample_timeout))
                paste_passed.append(length)
                print(f"paste_sha256 length={length}: PASS")
    except (OSError, TimeoutError, serial.SerialException) as error:
        failures.append(str(error))
    finally:
        if probe is not None:
            try:
                probe.settle_shell()
            except (OSError, serial.SerialException):
                pass
            probe.close()
        if log_file is not None:
            log_file.close()

    result = {
        "serial": args.serial,
        "baud": args.baud,
        "latency": summaries,
        "paste_passed": paste_passed,
        "failures": failures,
    }
    if args.json:
        args.json.write_text(
            json.dumps(result, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
    if failures:
        for failure in failures:
            print(f"FAIL: {failure}", file=sys.stderr)
        raise SystemExit(1)
    print("console latency and paste checks: PASS")


if __name__ == "__main__":
    main()
