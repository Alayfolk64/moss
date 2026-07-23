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
from pathlib import Path


PROFILE_SHARD_SIZE = 2_867_240
PROFILE_CPU_COUNT = 8
PROFILE_STATICS_SIZE = 56
READY_MARKER = b"#### MOSS GUEST PROFILE READY ####"
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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--kernel", type=Path, default=Path("kernel-rv"))
    parser.add_argument(
        "--elf", type=Path, default=Path("kernel-rv.profile.elf")
    )
    parser.add_argument("--disk", type=Path, default=Path("sdcard-rv.img"))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--gdb-port", type=int, default=1234)
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


def profiler_symbols(elf: Path) -> tuple[int, int, int]:
    result = subprocess.run(
        ["rust-nm", "-S", str(elf)],
        text=True,
        capture_output=True,
        check=True,
    )
    profiles: tuple[int, int] | None = None
    enabled: int | None = None
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) < 4:
            continue
        address, size, _kind, name = fields[0], fields[1], fields[2], fields[3]
        if "starry_kernel8profiler8PROFILES" in name:
            profiles = (int(address, 16), int(size, 16))
        elif "starry_kernel8profiler7ENABLED" in name:
            enabled = int(address, 16)
    if profiles is None or enabled is None:
        raise SystemExit("profiler symbols are missing from the kernel ELF")
    expected = PROFILE_SHARD_SIZE * PROFILE_CPU_COUNT
    if profiles[1] != expected:
        raise SystemExit(
            f"unexpected PROFILES size {profiles[1]}, expected {expected}"
        )
    return profiles[0], profiles[1], enabled


def process_ticks(pid: int) -> int:
    raw = Path(f"/proc/{pid}/stat").read_text()
    fields = raw[raw.rfind(")") + 2 :].split()
    return int(fields[11]) + int(fields[12])


def profile_locks_are_clear(path: Path) -> bool:
    data = path.read_bytes()
    if len(data) != PROFILE_SHARD_SIZE * PROFILE_CPU_COUNT:
        return False
    return all(data[cpu * PROFILE_SHARD_SIZE] == 0 for cpu in range(PROFILE_CPU_COUNT))


def dump_profile(
    elf: Path,
    out: Path,
    port: int,
    profiles_address: int,
    profiles_size: int,
    statics_address: int,
) -> None:
    profiles = (out / "profiles.bin").resolve()
    statics = (out / "profile-statics.bin").resolve()
    for attempt in range(1, 6):
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
            (
                f"dump binary memory {profiles} "
                f"{profiles_address:#x} {profiles_address + profiles_size:#x}"
            ),
            "-ex",
            (
                f"dump binary memory {statics} "
                f"{statics_address:#x} {statics_address + PROFILE_STATICS_SIZE:#x}"
            ),
            "-ex",
            "detach",
        ]
        result = subprocess.run(command, text=True, capture_output=True)
        if result.returncode == 0 and profile_locks_are_clear(profiles):
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
    kernel = require_file(args.kernel)
    elf = require_file(args.elf)
    disk = require_file(args.disk)
    args.out.mkdir(parents=True, exist_ok=True)
    out = args.out.resolve()
    console_path = out / "console.log"
    metadata_path = out / "run.json"

    profiles_address, profiles_size, statics_address = profiler_symbols(elf)
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
        f"file={disk},if=none,format=raw,id=x0",
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
                if READY_MARKER in tail:
                    status = "complete"
                    reason = "guest completed BuildStorm and stopped the profiler"
                    print(f"ready marker after {now - started:.1f}s", flush=True)
                    dump_profile(
                        elf,
                        out,
                        args.gdb_port,
                        profiles_address,
                        profiles_size,
                        statics_address,
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
                        profiles_address,
                        profiles_size,
                        statics_address,
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
                        profiles_address,
                        profiles_size,
                        statics_address,
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
                        profiles_address,
                        profiles_size,
                        statics_address,
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
                        profiles_address,
                        profiles_size,
                        statics_address,
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
        "profiles_address": profiles_address,
        "profiles_size": profiles_size,
        "statics_address": statics_address,
        "cpu_samples": cpu_samples,
    }
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"status={status}; metadata={metadata_path}", flush=True)
    raise SystemExit(0 if status == "complete" else 2)


if __name__ == "__main__":
    main()
