#!/usr/bin/env python3
"""Run one complete profiled RISC-V BuildStorm workload under QEMU.

The guest stops its profiler and prints ``MOSS GUEST PROFILE READY`` after the
workload.  At that point this runner freezes QEMU through its GDB stub and
copies the binary profile tables.  A low-CPU/no-console-progress watchdog does
the same capture if the guest appears deadlocked.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import signal
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path


PROFILE_SHARD_SIZE = 2_867_240
PROFILE_CPU_COUNT = 8
PROFILE_STATICS_SIZE = 56
PHASE_STARTED_SIZE = 8
SKIPPED_CPU_SIZE = 8
PHASE_TOTALS_SIZE = 32
MPROTECT_STATS_SIZE = 960
RISCV_PHYS_VIRT_OFFSET = 0xFFFF_FFC0_0000_0000
DEFAULT_READY_MARKER = "#### MOSS GUEST PROFILE READY ####"
PANIC_MARKERS = (b"panicked at", b"Kernel panic", b"assertion failed")
PROGRESS_RE = re.compile(
    rb"(?:"
    rb"Building \[[^\r\n]*\]\s+\d+/\d+"
    rb"|Compiling [^\r\n]+"
    rb"|Finished `[^`]+` profile"
    rb"|BUILDSTORM_(?:BEGIN|COMPILE|MINIBUILD|TOOLCHAIN)[^\r\n]*"
    rb"|\[axbuild\][^\r\n]+"
    rb"|OS COMP TEST GROUP END buildstorm-glibc"
    rb")"
)
BUILDSTORM_END_MARKER = b"#### OS COMP TEST GROUP END buildstorm-glibc ####"


@dataclass(frozen=True)
class ProfilerSymbols:
    profiles_address: int
    profiles_size: int
    phase_started_address: int
    skipped_cpu_address: int
    phase_totals_address: int
    phase_totals_size: int
    mprotect_stats_address: int
    mprotect_stats_size: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--kernel", type=Path, default=Path("kernel-rv"))
    parser.add_argument(
        "--elf", type=Path, default=Path("kernel-rv.profile.elf")
    )
    parser.add_argument("--disk", type=Path, default=Path("sdcard-rv.img"))
    parser.add_argument(
        "--disk-format",
        choices=("raw", "qcow2"),
        default="raw",
    )
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--gdb-port", type=int, default=1234)
    parser.add_argument(
        "--ready-marker",
        default=DEFAULT_READY_MARKER,
        help="console marker after which the stopped profile is captured",
    )
    parser.add_argument(
        "--stall-seconds",
        type=int,
        default=900,
        help="console-silent interval before low CPU can be classified as a stall",
    )
    parser.add_argument(
        "--low-cpu-seconds",
        type=int,
        default=180,
        help="continuous low-CPU interval required after console silence",
    )
    parser.add_argument(
        "--low-cpu-percent",
        type=float,
        default=20.0,
        help="QEMU process CPU threshold; 100%% means one saturated host CPU",
    )
    parser.add_argument(
        "--completion-grace-seconds",
        type=int,
        default=300,
        help="maximum delay from BuildStorm END to the stopped-profile marker",
    )
    parser.add_argument("--timeout-seconds", type=int, default=6 * 60 * 60)
    parser.add_argument("--poll-seconds", type=int, default=15)
    return parser.parse_args()


def iso_now() -> str:
    return dt.datetime.now(dt.timezone.utc).astimezone().isoformat()


def require_file(path: Path) -> Path:
    path = path.resolve()
    if not path.is_file():
        raise SystemExit(f"required file does not exist: {path}")
    return path


def profiler_symbols(elf: Path) -> ProfilerSymbols:
    result = subprocess.run(
        ["rust-nm", "-S", str(elf)],
        text=True,
        capture_output=True,
        check=True,
    )
    profiles: tuple[int, int] | None = None
    phase_started: tuple[int, int] | None = None
    skipped_cpu: tuple[int, int] | None = None
    phase_totals: tuple[int, int] | None = None
    mprotect_stats: tuple[int, int] | None = None
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) < 4:
            continue
        address, size, _kind, name = fields[0], fields[1], fields[2], fields[3]
        if "starry_kernel8profiler8PROFILES" in name:
            profiles = (int(address, 16), int(size, 16))
        elif "starry_kernel8profiler16PHASE_STARTED_NS" in name:
            phase_started = (int(address, 16), int(size, 16))
        elif "starry_kernel8profiler11SKIPPED_CPU" in name:
            skipped_cpu = (int(address, 16), int(size, 16))
        elif "starry_kernel8profiler14PHASE_TOTAL_NS" in name:
            phase_totals = (int(address, 16), int(size, 16))
        elif "MPROTECT_STATS" in name:
            mprotect_stats = (int(address, 16), int(size, 16))
    if (
        profiles is None
        or phase_started is None
        or skipped_cpu is None
        or phase_totals is None
        or mprotect_stats is None
    ):
        raise SystemExit("profiler symbols are missing from the kernel ELF")
    expected = PROFILE_SHARD_SIZE * PROFILE_CPU_COUNT
    if profiles[1] != expected:
        raise SystemExit(
            f"unexpected PROFILES size {profiles[1]}, expected {expected}"
        )
    if mprotect_stats[1] != MPROTECT_STATS_SIZE:
        raise SystemExit(
            f"unexpected MPROTECT_STATS size {mprotect_stats[1]}, "
            f"expected {MPROTECT_STATS_SIZE}"
        )
    for name, symbol, expected_size in (
        ("PHASE_STARTED_NS", phase_started, PHASE_STARTED_SIZE),
        ("SKIPPED_CPU", skipped_cpu, SKIPPED_CPU_SIZE),
        ("PHASE_TOTAL_NS", phase_totals, PHASE_TOTALS_SIZE),
    ):
        if symbol[1] != expected_size:
            raise SystemExit(
                f"unexpected {name} size {symbol[1]}, expected {expected_size}"
            )
    return ProfilerSymbols(
        profiles_address=profiles[0],
        profiles_size=profiles[1],
        phase_started_address=phase_started[0],
        skipped_cpu_address=skipped_cpu[0],
        phase_totals_address=phase_totals[0],
        phase_totals_size=phase_totals[1],
        mprotect_stats_address=mprotect_stats[0],
        mprotect_stats_size=mprotect_stats[1],
    )


def process_ticks(pid: int) -> int:
    raw = Path(f"/proc/{pid}/stat").read_text()
    fields = raw[raw.rfind(")") + 2 :].split()
    return int(fields[11]) + int(fields[12])


def profile_locks_are_clear(path: Path) -> bool:
    if not path.is_file():
        return False
    data = path.read_bytes()
    if len(data) != PROFILE_SHARD_SIZE * PROFILE_CPU_COUNT:
        return False
    return all(data[cpu * PROFILE_SHARD_SIZE] == 0 for cpu in range(PROFILE_CPU_COUNT))


def dump_profile(
    elf: Path,
    out: Path,
    port: int,
    symbols: ProfilerSymbols,
) -> None:
    def physical_address(address: int) -> int:
        if address < RISCV_PHYS_VIRT_OFFSET:
            raise RuntimeError(f"kernel symbol is not in the direct map: {address:#x}")
        return address - RISCV_PHYS_VIRT_OFFSET

    profiles_address = physical_address(symbols.profiles_address)
    phase_started_address = physical_address(symbols.phase_started_address)
    skipped_cpu_address = physical_address(symbols.skipped_cpu_address)
    phase_totals_address = physical_address(symbols.phase_totals_address)
    mprotect_stats_address = physical_address(symbols.mprotect_stats_address)
    profiles = (out / "profiles.bin").resolve()
    statics = (out / "profile-statics.bin").resolve()
    phase_started = (out / "phase-started.bin").resolve()
    skipped_cpu = (out / "skipped-cpu.bin").resolve()
    phase_totals = (out / "phase-totals.bin").resolve()
    mprotect_stats = (out / "mprotect-stats.bin").resolve()
    for attempt in range(1, 6):
        for capture in (
            profiles,
            statics,
            phase_started,
            skipped_cpu,
            phase_totals,
            mprotect_stats,
        ):
            capture.unlink(missing_ok=True)
        command = [
            "gdb-multiarch",
            "-q",
            "-batch",
            str(elf),
            "-ex",
            "set pagination off",
            "-ex",
            f"target remote 127.0.0.1:{port}",
            "-ex",
            "maintenance packet Qqemu.PhyMemMode:1",
            "-ex",
            (
                f"dump binary memory {profiles} "
                f"{profiles_address:#x} "
                f"{profiles_address + symbols.profiles_size:#x}"
            ),
            "-ex",
            (
                f"dump binary memory {phase_started} "
                f"{phase_started_address:#x} "
                f"{phase_started_address + PHASE_STARTED_SIZE:#x}"
            ),
            "-ex",
            (
                f"dump binary memory {skipped_cpu} "
                f"{skipped_cpu_address:#x} "
                f"{skipped_cpu_address + SKIPPED_CPU_SIZE:#x}"
            ),
            "-ex",
            (
                f"dump binary memory {phase_totals} "
                f"{phase_totals_address:#x} "
                f"{phase_totals_address + symbols.phase_totals_size:#x}"
            ),
            "-ex",
            (
                f"dump binary memory {mprotect_stats} "
                f"{mprotect_stats_address:#x} "
                f"{mprotect_stats_address + symbols.mprotect_stats_size:#x}"
            ),
            "-ex",
            "detach",
        ]
        result = subprocess.run(command, text=True, capture_output=True)
        if (
            result.returncode == 0
            and profile_locks_are_clear(profiles)
            and phase_started.is_file()
            and phase_started.stat().st_size == PHASE_STARTED_SIZE
            and skipped_cpu.is_file()
            and skipped_cpu.stat().st_size == SKIPPED_CPU_SIZE
            and phase_totals.is_file()
            and phase_totals.stat().st_size == PHASE_TOTALS_SIZE
            and mprotect_stats.is_file()
            and mprotect_stats.stat().st_size == MPROTECT_STATS_SIZE
        ):
            compatible_statics = bytearray(PROFILE_STATICS_SIZE)
            compatible_statics[8:16] = phase_started.read_bytes()
            compatible_statics[16:24] = skipped_cpu.read_bytes()
            compatible_statics[24:56] = phase_totals.read_bytes()
            statics.write_bytes(compatible_statics)
            print(f"profile dump completed on attempt {attempt}", flush=True)
            return
        print(
            f"profile dump attempt {attempt} was inconsistent: "
            f"gdb_rc={result.returncode}",
            flush=True,
        )
        if result.stderr:
            print(result.stderr[-2000:], flush=True)
        time.sleep(2)
    raise RuntimeError("could not capture profile tables with all shard locks clear")


def terminate_process_group(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=10)


def main() -> None:
    args = parse_args()
    ready_marker = args.ready_marker.encode()
    kernel = require_file(args.kernel)
    elf = require_file(args.elf)
    disk = require_file(args.disk)
    args.out.mkdir(parents=True, exist_ok=True)
    out = args.out.resolve()
    console_path = out / "console.log"
    metadata_path = out / "run.json"

    symbols = profiler_symbols(elf)
    qemu = [
        "qemu-system-riscv64",
        "-machine",
        "virt",
        "-kernel",
        str(kernel),
        "-m",
        "8G",
        "-nographic",
        "-smp",
        "8",
        "-bios",
        "default",
        "-snapshot",
        "-drive",
        f"file={disk},if=none,format={args.disk_format},id=x0",
        "-device",
        "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0",
        "-no-reboot",
        "-device",
        "virtio-net-device,netdev=net",
        "-netdev",
        "user,id=net",
        "-rtc",
        "base=utc",
        "-gdb",
        f"tcp:127.0.0.1:{args.gdb_port}",
    ]
    started_at = iso_now()
    started = time.monotonic()
    status = "running"
    reason = ""
    cpu_samples: list[dict[str, float]] = []

    with console_path.open("wb", buffering=0) as console:
        process = subprocess.Popen(
            qemu,
            stdout=console,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        print(f"qemu pid={process.pid}; console={console_path}", flush=True)
        last_size = 0
        last_output_at = started
        last_progress_at = started
        scan_offset = 0
        scan_tail = b""
        completion_seen_at: float | None = None
        low_cpu_since: float | None = None
        last_sample_at = started
        last_ticks = process_ticks(process.pid)
        clock_ticks = os.sysconf(os.sysconf_names["SC_CLK_TCK"])
        try:
            while process.poll() is None:
                time.sleep(args.poll_seconds)
                now = time.monotonic()
                size = console_path.stat().st_size
                if size != last_size:
                    last_size = size
                    last_output_at = now
                if size > scan_offset:
                    with console_path.open("rb") as source:
                        source.seek(scan_offset)
                        new_output = source.read(size - scan_offset)
                    scan_offset = size
                    scan_output = scan_tail + new_output
                    tail_length = len(scan_tail)
                    scan_tail = scan_output[-1024:]
                    if any(
                        match.end() > tail_length
                        for match in PROGRESS_RE.finditer(scan_output)
                    ):
                        last_progress_at = now
                    if (
                        completion_seen_at is None
                        and BUILDSTORM_END_MARKER in scan_output[
                            max(0, tail_length - len(BUILDSTORM_END_MARKER)) :
                        ]
                    ):
                        completion_seen_at = now

                ticks = process_ticks(process.pid)
                interval = max(now - last_sample_at, 0.001)
                cpu_percent = (ticks - last_ticks) * 100.0 / clock_ticks / interval
                last_ticks = ticks
                last_sample_at = now
                cpu_samples.append(
                    {
                        "elapsed_s": round(now - started, 3),
                        "qemu_cpu_percent": round(cpu_percent, 2),
                        "console_bytes": size,
                    }
                )

                tail_size = min(size, 256 * 1024)
                with console_path.open("rb") as source:
                    source.seek(size - tail_size)
                    tail = source.read()
                if ready_marker in tail:
                    status = "complete"
                    reason = (
                        f"guest emitted the stopped-profile marker: {args.ready_marker}"
                    )
                    print(f"ready marker after {now - started:.1f}s", flush=True)
                    dump_profile(
                        elf,
                        out,
                        args.gdb_port,
                        symbols,
                    )
                    break
                if any(marker in tail for marker in PANIC_MARKERS):
                    status = "failed"
                    reason = "guest panic detected"
                    print(reason, flush=True)
                    dump_profile(
                        elf,
                        out,
                        args.gdb_port,
                        symbols,
                    )
                    break

                silent_for = now - last_output_at
                progress_silent_for = now - last_progress_at
                if (
                    progress_silent_for >= args.stall_seconds
                    and cpu_percent < args.low_cpu_percent
                ):
                    if low_cpu_since is None:
                        low_cpu_since = now
                else:
                    low_cpu_since = None
                low_cpu_for = 0.0 if low_cpu_since is None else now - low_cpu_since
                if low_cpu_for >= args.low_cpu_seconds:
                    status = "stalled"
                    reason = (
                        f"no workload progress for {progress_silent_for:.0f}s and QEMU "
                        f"stayed below {args.low_cpu_percent:.1f}% CPU for "
                        f"{low_cpu_for:.0f}s"
                    )
                    print(reason, flush=True)
                    dump_profile(
                        elf,
                        out,
                        args.gdb_port,
                        symbols,
                    )
                    break
                if (
                    completion_seen_at is not None
                    and now - completion_seen_at >= args.completion_grace_seconds
                ):
                    status = "stalled"
                    reason = (
                        "BuildStorm printed its END marker but init did not reach the "
                        f"stopped-profile marker within {args.completion_grace_seconds}s"
                    )
                    print(reason, flush=True)
                    dump_profile(
                        elf,
                        out,
                        args.gdb_port,
                        symbols,
                    )
                    break
                if now - started >= args.timeout_seconds:
                    status = "timeout"
                    reason = f"wall-clock timeout after {now - started:.0f}s"
                    print(reason, flush=True)
                    dump_profile(
                        elf,
                        out,
                        args.gdb_port,
                        symbols,
                    )
                    break
                print(
                    f"elapsed={now - started:.0f}s qemu_cpu={cpu_percent:.1f}% "
                    f"silent={silent_for:.0f}s progress_silent={progress_silent_for:.0f}s "
                    f"bytes={size}",
                    flush=True,
                )
            else:
                status = "failed"
                reason = f"QEMU exited with status {process.returncode}"
        finally:
            terminate_process_group(process)

    finished = time.monotonic()
    metadata = {
        "status": status,
        "reason": reason,
        "started_at": started_at,
        "finished_at": iso_now(),
        "host_elapsed_s": round(finished - started, 3),
        "qemu_command": qemu,
        "kernel": str(kernel),
        "elf": str(elf),
        "disk": str(disk),
        "disk_format": args.disk_format,
        "profiles_address": symbols.profiles_address,
        "profiles_size": symbols.profiles_size,
        "phase_started_address": symbols.phase_started_address,
        "skipped_cpu_address": symbols.skipped_cpu_address,
        "phase_totals_address": symbols.phase_totals_address,
        "phase_totals_size": symbols.phase_totals_size,
        "mprotect_stats_address": symbols.mprotect_stats_address,
        "mprotect_stats_size": symbols.mprotect_stats_size,
        "cpu_samples": cpu_samples,
    }
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"status={status}; metadata={metadata_path}", flush=True)
    raise SystemExit(0 if status == "complete" else 2)


if __name__ == "__main__":
    main()
