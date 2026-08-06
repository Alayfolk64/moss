#!/usr/bin/env python3
"""Sample all QEMU guest CPUs through the GDB stub and emit folded stacks.

This profiler does not instrument the guest.  Each sample briefly stops all
vCPUs, asks GDB for every CPU backtrace, detaches, and lets QEMU continue.
The resulting wall-clock time is therefore not a benchmark result; the stack
distribution is intended for hotspot discovery only.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import time
from collections import Counter
from pathlib import Path


THREAD_RE = re.compile(r"^Thread\s+\d+\s+\(Thread .*?CPU#(\d+)", re.MULTILINE)
FRAME_RE = re.compile(r"^#\d+\s+(.*)$")
ADDRESS_IN_RE = re.compile(r"^0x[0-9a-fA-F]+\s+in\s+")
ADDRESS_RE = re.compile(r"^0x[0-9a-fA-F]+$")
BEGIN_MARKER = "BUILDSTORM_BEGIN mode=multi"
END_MARKER = "#### OS COMP TEST GROUP END buildstorm-glibc ####"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--elf", type=Path, required=True)
    parser.add_argument("--console", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--gdb-port", type=int, default=1234)
    parser.add_argument("--interval", type=float, default=2.0)
    parser.add_argument("--timeout", type=float, default=900.0)
    parser.add_argument("--depth", type=int, default=16)
    parser.add_argument("--flamegraph", type=Path)
    return parser.parse_args()


def console_text(path: Path) -> str:
    try:
        return path.read_text(errors="replace")
    except FileNotFoundError:
        return ""


def normalize_frame(raw: str) -> str:
    frame = ADDRESS_IN_RE.sub("", raw.strip())
    frame = frame.split(" at ", 1)[0]
    frame = frame.split(" from ", 1)[0]
    if frame.endswith(" ()"):
        frame = frame[:-3]
    if " (" in frame:
        frame = frame.split(" (", 1)[0]
    if frame == "??" or ADDRESS_RE.fullmatch(frame):
        return "[guest-user-or-unknown]"
    return frame.replace(";", ":") or "[unknown]"


def parse_backtraces(text: str) -> list[tuple[int, tuple[str, ...]]]:
    matches = list(THREAD_RE.finditer(text))
    stacks: list[tuple[int, tuple[str, ...]]] = []
    for index, match in enumerate(matches):
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        frames = []
        for line in text[match.end() : end].splitlines():
            frame_match = FRAME_RE.match(line)
            if frame_match:
                frame = normalize_frame(frame_match.group(1))
                if not frames or frames[-1] != frame:
                    frames.append(frame)
        if frames:
            stacks.append((int(match.group(1)), tuple(reversed(frames))))
    return stacks


def sample(elf: Path, port: int, depth: int) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            "gdb-multiarch",
            "-q",
            "-batch",
            str(elf),
            "-ex",
            "set pagination off",
            "-ex",
            "set confirm off",
            "-ex",
            f"target remote 127.0.0.1:{port}",
            "-ex",
            f"thread apply all bt {depth}",
            "-ex",
            "detach",
        ],
        text=True,
        capture_output=True,
        timeout=20,
    )


def resume_after_failed_sample(elf: Path, port: int) -> subprocess.CompletedProcess[str]:
    """Detach a fresh GDB client because an aborted GDB can leave QEMU stopped."""
    return subprocess.run(
        [
            "gdb-multiarch",
            "-q",
            "-batch",
            str(elf),
            "-ex",
            "set confirm off",
            "-ex",
            f"target remote 127.0.0.1:{port}",
            "-ex",
            "detach",
        ],
        text=True,
        capture_output=True,
        timeout=20,
    )


def is_idle(frames: tuple[str, ...]) -> bool:
    return any("run_idle" in frame or "rust_main_secondary" in frame for frame in frames)


def write_folded(path: Path, values: Counter[tuple[str, ...]]) -> None:
    with path.open("w") as output:
        for frames, count in sorted(values.items()):
            output.write(f"{';'.join(frames)} {count}\n")


def render(flamegraph: Path, folded: Path, svg: Path, title: str) -> None:
    with folded.open() as source, svg.open("w") as output:
        subprocess.run(
            [
                str(flamegraph),
                "--width",
                "1800",
                "--minwidth",
                "0.25",
                "--title",
                title,
                "--countname",
                "vCPU samples",
                "--colors",
                "hot",
            ],
            stdin=source,
            stdout=output,
            check=True,
        )


def top_leaves(values: Counter[tuple[str, ...]], limit: int = 30) -> list[dict[str, object]]:
    leaves: Counter[str] = Counter()
    for frames, count in values.items():
        leaves[frames[-1]] += count
    total = leaves.total()
    return [
        {
            "leaf": leaf,
            "samples": count,
            "percent": count * 100.0 / total if total else 0.0,
        }
        for leaf, count in leaves.most_common(limit)
    ]


def resolved_kernel(values: Counter[tuple[str, ...]]) -> Counter[tuple[str, ...]]:
    return Counter(
        {
            frames: count
            for frames, count in values.items()
            if frames[-1] not in {"[guest-user-or-unknown]", "trap_vector_base"}
        }
    )


def top_inclusive(values: Counter[tuple[str, ...]], limit: int = 30) -> list[dict[str, object]]:
    functions: Counter[str] = Counter()
    for frames, count in values.items():
        for frame in set(frames[2:]):
            functions[frame] += count
    total = values.total()
    return [
        {
            "function": function,
            "samples": count,
            "percent": count * 100.0 / total if total else 0.0,
        }
        for function, count in functions.most_common(limit)
    ]


def main() -> None:
    args = parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    while BEGIN_MARKER not in console_text(args.console):
        if time.monotonic() - started > args.timeout:
            raise SystemExit("timed out waiting for BuildStorm begin marker")
        time.sleep(0.25)

    all_stacks: Counter[tuple[str, ...]] = Counter()
    active_stacks: Counter[tuple[str, ...]] = Counter()
    raw_path = args.out / "gdb-samples.txt"
    raw_output = raw_path.open("w")
    attempts = 0
    failures = 0
    while END_MARKER not in console_text(args.console):
        if time.monotonic() - started > args.timeout:
            raise SystemExit("timed out while sampling BuildStorm")
        attempts += 1
        try:
            result = sample(args.elf, args.gdb_port, args.depth)
        except subprocess.TimeoutExpired:
            failures += 1
            try:
                resume_after_failed_sample(args.elf, args.gdb_port)
            except (subprocess.SubprocessError, OSError):
                pass
            time.sleep(args.interval)
            continue
        raw_output.write(
            f"===== SAMPLE {attempts} rc={result.returncode} =====\n"
            f"{result.stdout}{result.stderr}"
        )
        raw_output.flush()
        if result.returncode != 0:
            failures += 1
            try:
                rescue = resume_after_failed_sample(args.elf, args.gdb_port)
                raw_output.write(
                    f"===== RESUME {attempts} rc={rescue.returncode} =====\n"
                    f"{rescue.stdout}{rescue.stderr}"
                )
                raw_output.flush()
            except (subprocess.SubprocessError, OSError) as error:
                raw_output.write(f"===== RESUME {attempts} failed={error!r} =====\n")
                raw_output.flush()
        else:
            for _cpu, frames in parse_backtraces(result.stdout):
                state = "state:idle" if is_idle(frames) else "state:active"
                folded = ("riscv-qemu-guest", state, *frames)
                all_stacks[folded] += 1
                if state == "state:active":
                    active_stacks[folded] += 1
        time.sleep(args.interval)

    raw_output.close()
    all_folded = args.out / "cpu-all.folded"
    active_folded = args.out / "cpu-active.folded"
    write_folded(all_folded, all_stacks)
    write_folded(active_folded, active_stacks)
    resolved = resolved_kernel(active_stacks)
    summary = {
        "method": "qemu-gdb-all-vcpu",
        "interval_seconds": args.interval,
        "attempts": attempts,
        "failures": failures,
        "all_vcpu_samples": all_stacks.total(),
        "active_vcpu_samples": active_stacks.total(),
        "idle_vcpu_samples": all_stacks.total() - active_stacks.total(),
        "active_percent": (
            active_stacks.total() * 100.0 / all_stacks.total() if all_stacks else 0.0
        ),
        "resolved_kernel_samples": resolved.total(),
        "top_resolved_kernel_inclusive": top_inclusive(resolved),
        "top_active_leaves": top_leaves(active_stacks),
        "top_all_leaves": top_leaves(all_stacks),
    }
    (args.out / "summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n"
    )
    if args.flamegraph is not None:
        render(
            args.flamegraph,
            all_folded,
            args.out / "cpu-all.svg",
            "Moss RV BuildStorm CPU - all vCPUs",
        )
        render(
            args.flamegraph,
            active_folded,
            args.out / "cpu-active.svg",
            "Moss RV BuildStorm CPU - active vCPUs",
        )
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
