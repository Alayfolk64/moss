#!/usr/bin/env python3
"""Convert a live Moss profiler memory dump into its serial snapshot format."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path


CPU_COUNT = 8
STACK_DEPTH = 20
CPU_TABLE_SIZE = 4096
WAIT_TABLE_SIZE = 8192
PENDING_SIZE = 512

AGGREGATE_SIZE = 224
PENDING_SIZE_BYTES = 224
PROFILE_STATE_SIZE = 2_867_232
PROFILE_SHARD_SIZE = 2_867_240

STATE_OFFSET = 8
PENDING_OFFSET = STATE_OFFSET
CPU_OFFSET = PENDING_SIZE * PENDING_SIZE_BYTES
CPU_OFFSET += STATE_OFFSET
WAIT_OFFSET = CPU_OFFSET + CPU_TABLE_SIZE * AGGREGATE_SIZE
LOCK_OFFSET = 0

STACK_OFFSET = 8
STACK_FRAMES_OFFSET = 0
STACK_TASK_OFFSET = 160
STACK_TASK_BYTES_OFFSET = 0
STACK_TASK_LEN_OFFSET = 16
STACK_LEN_OFFSET = 177

TOTAL_OFFSET = 192
COUNT_OFFSET = 200
MAX_OFFSET = 208
PHASE_OFFSET = 216
EVENT_OFFSET = 217


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profiles", type=Path, required=True)
    parser.add_argument("--statics", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--timed-end-seconds",
        type=float,
        help="guest monotonic timestamp at the last completed timed-build output",
    )
    return parser.parse_args()


def u64(data: bytes, offset: int) -> int:
    return struct.unpack_from("<Q", data, offset)[0]


def stack_fields(entry: bytes) -> tuple[bytes, tuple[int, ...]]:
    stack = STACK_OFFSET
    task = stack + STACK_TASK_OFFSET
    task_len = entry[task + STACK_TASK_LEN_OFFSET]
    stack_len = entry[stack + STACK_LEN_OFFSET]
    if task_len > 16 or stack_len > STACK_DEPTH:
        raise ValueError(f"invalid profile entry: task_len={task_len}, stack_len={stack_len}")
    task_name = entry[
        task + STACK_TASK_BYTES_OFFSET : task + STACK_TASK_BYTES_OFFSET + task_len
    ]
    frames = struct.unpack_from(
        f"<{stack_len}Q", entry, stack + STACK_FRAMES_OFFSET
    )
    return task_name, frames


def stack_text(frames: tuple[int, ...]) -> str:
    return ";".join("user" if frame == 2**64 - 1 else f"{frame:#x}" for frame in frames)


def emit_table(
    lines: list[str], shard: bytes, offset: int, size: int, expect_wait: bool
) -> None:
    for index in range(size):
        start = offset + index * AGGREGATE_SIZE
        entry = shard[start : start + AGGREGATE_SIZE]
        if u64(entry, 0) == 0:
            continue
        phase = entry[PHASE_OFFSET]
        event = entry[EVENT_OFFSET]
        if phase not in (1, 2, 3):
            raise ValueError(f"invalid phase {phase} in aggregate table")
        if expect_wait and event not in range(1, 11):
            raise ValueError(f"invalid wait event {event}")
        if not expect_wait and event != 0:
            raise ValueError(f"invalid CPU event {event}")
        task_name, frames = stack_fields(entry)
        if expect_wait:
            lines.append(
                f"MOSS_WAIT phase={phase} event={event} count={u64(entry, COUNT_OFFSET)} "
                f"total_ns={u64(entry, TOTAL_OFFSET)} max_ns={u64(entry, MAX_OFFSET)} "
                f"task={task_name.hex()} stack={stack_text(frames)}"
            )
        else:
            lines.append(
                f"MOSS_CPU phase={phase} samples={u64(entry, TOTAL_OFFSET)} "
                f"task={task_name.hex()} stack={stack_text(frames)}"
            )


def main() -> None:
    args = parse_args()
    profiles = args.profiles.read_bytes()
    statics = args.statics.read_bytes()
    expected_size = PROFILE_SHARD_SIZE * CPU_COUNT
    if len(profiles) != expected_size:
        raise SystemExit(
            f"profile dump has {len(profiles)} bytes, expected {expected_size}"
        )
    if len(statics) < 56:
        raise SystemExit("profile statics dump is truncated")

    phase_started_ns = u64(statics, 8)
    skipped_cpu = u64(statics, 16)
    phase_totals = list(struct.unpack_from("<4Q", statics, 24))
    if args.timed_end_seconds is not None:
        timed_end_ns = round(args.timed_end_seconds * 1_000_000_000)
        phase_totals[3] = max(0, timed_end_ns - phase_started_ns)

    dropped_cpu = 0
    dropped_wait = 0
    dropped_pending = 0
    body: list[str] = []
    for cpu in range(CPU_COUNT):
        shard_start = cpu * PROFILE_SHARD_SIZE
        shard = profiles[shard_start : shard_start + PROFILE_SHARD_SIZE]
        if shard[LOCK_OFFSET] != 0:
            raise SystemExit(f"profile shard {cpu} was dumped while locked")
        dropped_cpu += u64(shard, STATE_OFFSET + PROFILE_STATE_SIZE - 24)
        dropped_wait += u64(shard, STATE_OFFSET + PROFILE_STATE_SIZE - 16)
        dropped_pending += u64(shard, STATE_OFFSET + PROFILE_STATE_SIZE - 8)
        emit_table(body, shard, CPU_OFFSET, CPU_TABLE_SIZE, expect_wait=False)
        emit_table(body, shard, WAIT_OFFSET, WAIT_TABLE_SIZE, expect_wait=True)

    header = (
        "MOSS_PROFILE_V1 sample_hz=10 ext4_sample_rate=16 "
        "page_cache_sample_rate=32 offcpu_sample_rate=8 "
        f"setup_ns={phase_totals[1]} prebuild_ns={phase_totals[2]} "
        f"timed_ns={phase_totals[3]} dropped_cpu={dropped_cpu} "
        f"dropped_wait={dropped_wait} dropped_pending={dropped_pending} "
        f"skipped_cpu={skipped_cpu}"
    )
    output = [
        "#### MOSS GUEST PROFILE START ####",
        header,
        "MOSS_PHASE 1 setup",
        "MOSS_PHASE 2 prebuild",
        "MOSS_PHASE 3 timed",
        *body,
        "#### MOSS GUEST PROFILE END ####",
        "",
    ]
    args.output.write_text("\n".join(output))


if __name__ == "__main__":
    main()
