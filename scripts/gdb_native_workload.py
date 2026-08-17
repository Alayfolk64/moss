#!/usr/bin/env python3
"""Shared guest-side native GDB demo and acceptance checks."""

from __future__ import annotations

import re
from typing import Protocol


class ShellRunner(Protocol):
    def shell_command(self, line: str, timeout: float = 20): ...

    def emit_line(self, message: str) -> None: ...


REQUIRED_MARKERS = (
    b"GDB_NATIVE_BREAKPOINT_SET",
    b"GDB_NATIVE_BREAKPOINT_HIT",
    b"GDB_NATIVE_BACKTRACE_DONE",
    b"GDB_NATIVE_PROC_MAPPINGS_DONE",
    b"GDB_NATIVE_PROC_FILES_DONE",
    b"GDB_NATIVE_PROC_AUXV_DONE",
    b"GDB_NATIVE_PROC_STATUS_BEGIN",
    b"GDB_NATIVE_PROC_STATUS_DONE",
    b"GDB_NATIVE_REGS_DONE",
    b"GDB_NATIVE_MEMORY_DONE",
    b"GDB_NATIVE_STEPI_DONE",
    b"GDB_NATIVE_SOURCE_STEP_BEGIN",
    b"GDB_NATIVE_SOURCE_STEP_DONE",
    b"gdb-native-demo value=42",
    b"exited normally",
    b"GDB_NATIVE_SMOKE_DONE",
)

REQUIRED_BACKTRACE = (
    b"native_marker",
    b"demo_worker",
    b"demo_entry",
    b"main",
)

FORBIDDEN_OUTPUT = (
    b"GDB_NATIVE_STEPI_FAILED",
    b"Trace/breakpoint trap",
    b"core dumped",
    b"Cannot access memory",
    b"Couldn't get registers",
    b"ptrace: Function not implemented",
    b"During startup program exited",
    b"The program is not being run",
)


def _command(
    runner: ShellRunner,
    label: str,
    line: str,
    timeout: float,
) -> bytes:
    result = runner.shell_command(line, timeout=timeout)
    if isinstance(result, tuple):
        rc, output = result
    else:
        rc, output = 0, result
    if rc != 0:
        raise RuntimeError(f"{label} returned {rc}")
    runner.emit_line(f"[gdb-native] PASS {label}")
    return output


def ensure_gdb_packages(runner: ShellRunner) -> None:
    _command(
        runner,
        "offline GDB package installation",
        "apk --no-network add --allow-untrusted "
        "/var/cache/moss-gdb-native/*.apk",
        600,
    )
    output = _command(
        runner,
        "GDB/GCC package inventory",
        "gdb --version | sed -n '1,2p'; gcc --version | sed -n '1p'; "
        "apk info -e gdb gcc musl-dev binutils",
        60,
    )
    if b"GNU gdb" not in output or b"gcc" not in output:
        raise RuntimeError("installed GDB/GCC inventory is incomplete")


def run_gdb_native_test(runner: ShellRunner) -> None:
    _command(
        runner,
        "demo staging",
        "rm -rf /var/tmp/moss-gdb-native && "
        "mkdir -p /var/tmp/moss-gdb-native && "
        "cp /var/cache/moss-gdb-native/gdb-native-demo.c "
        "/var/cache/moss-gdb-native/gdb-native-smoke.gdb "
        "/var/tmp/moss-gdb-native/",
        30,
    )
    build_output = _command(
        runner,
        "demo compilation",
        "cd /var/tmp/moss-gdb-native && "
        "gcc -O0 -g3 -fno-omit-frame-pointer -fno-pie -no-pie "
        "-Wall -Wextra -Werror "
        "gdb-native-demo.c -o gdb-native-demo && "
        "file gdb-native-demo && readelf -h gdb-native-demo | "
        "sed -n '1,24p'",
        180,
    )
    if b"with debug_info" not in build_output and b"not stripped" not in build_output:
        raise RuntimeError("demo binary does not retain debug information")
    if re.search(rb"Type:\s+EXEC", build_output) is None:
        raise RuntimeError("demo binary is not the required non-PIE ET_EXEC image")

    output = _command(
        runner,
        "native GDB batch demo",
        "cd /var/tmp/moss-gdb-native && "
        "timeout -s KILL 240 gdb -q -nx -batch "
        "-x gdb-native-smoke.gdb ./gdb-native-demo",
        270,
    )
    missing = [marker.decode() for marker in REQUIRED_MARKERS if marker not in output]
    if missing:
        raise RuntimeError(f"native GDB markers missing: {', '.join(missing)}")
    missing_frames = [
        frame.decode() for frame in REQUIRED_BACKTRACE if frame not in output
    ]
    if missing_frames:
        raise RuntimeError(
            f"native GDB backtrace frames missing: {', '.join(missing_frames)}"
        )
    forbidden = [marker.decode() for marker in FORBIDDEN_OUTPUT if marker in output]
    if forbidden:
        raise RuntimeError(f"native GDB failure text observed: {', '.join(forbidden)}")
    step = re.search(
        rb"GDB_NATIVE_STEPI_DONE before=(0x[0-9a-fA-F]+) after=(0x[0-9a-fA-F]+)",
        output,
    )
    if step is None or step.group(1) == step.group(2):
        raise RuntimeError("native GDB stepi did not produce two distinct PCs")
    runner.emit_line(
        "[gdb-native] ALL NATIVE GDB BREAKPOINT/BACKTRACE/REGISTERS/"
        "MEMORY/STEPI/CONTINUE TESTS PASSED"
    )
