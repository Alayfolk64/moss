#!/usr/bin/env python3
"""Render phase-complete Linux guest BPF profiles as FlameGraph SVGs."""

from __future__ import annotations

import argparse
import html
import json
import re
import subprocess
from collections import defaultdict
from pathlib import Path


PHASES = ("setup", "prebuild", "timed")
ENTRY_RE = re.compile(r"^@(\w+)\[(.*?)\]:\s*(\d+)\s*$", re.MULTILINE | re.DOTALL)
DURATION_MAPS = {
    "offcpu",
    "mutex",
    "rwsem",
    "pagecache",
    "ext4",
    "block_read",
    "block_write",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--flamegraph-dir", type=Path, default=Path("/tmp/moss-flamegraph"))
    return parser.parse_args()


def normalize_frame(frame: str) -> str:
    frame = frame.strip().replace(";", ":")
    frame = re.sub(r"\+[0-9]+$", "", frame)
    return frame or "[unknown]"


def parse_key(name: str, key: str) -> tuple[str, int | None, tuple[str, ...]]:
    field_count = 3 if name == "offcpu" else 2
    fields = key.split(",", field_count - 1)
    if len(fields) != field_count:
        raise ValueError(f"unexpected @{name} key: {key[:160]!r}")
    comm = fields[0].strip() or "unknown"
    state = int(fields[1].strip()) if name == "offcpu" else None
    stack_text = fields[-1]
    frames = []
    for raw in stack_text.splitlines():
        frame = normalize_frame(raw)
        if frame == "[unknown]" and not raw.strip():
            continue
        if not frames or frames[-1] != frame:
            frames.append(frame)
    return comm, state, tuple(frames)


def parse_raw(path: Path, phase: str) -> list[dict]:
    text = path.read_text(errors="replace")
    if "Attaching 20 probes..." not in text:
        raise ValueError(f"profile never attached: {path}")
    rows = []
    for match in ENTRY_RE.finditer(text):
        name, key, value_text = match.groups()
        if name not in DURATION_MAPS | {"cpu"}:
            continue
        comm, state, frames = parse_key(name, key)
        rows.append(
            {
                "phase": phase,
                "event": name,
                "comm": comm,
                "state": state,
                "frames": frames,
                "value": int(value_text),
            }
        )
    if not any(row["event"] == "cpu" for row in rows):
        raise ValueError(f"profile contains no CPU samples: {path}")
    return rows


def parse_times(path: Path) -> dict[str, dict[str, float]]:
    result = {}
    for line in path.read_text().splitlines():
        fields = line.split()
        if len(fields) != 3 or fields[0] not in PHASES:
            continue
        start, end = map(float, fields[1:])
        result[fields[0]] = {"start_s": start, "end_s": end, "elapsed_s": end - start}
    missing = set(PHASES) - result.keys()
    if missing:
        raise ValueError(f"missing phase times: {sorted(missing)}")
    return result


def is_kernel_background(row: dict) -> bool:
    comm = row["comm"]
    frames = row["frames"]
    if comm in {"bpftrace", "kthreadd"}:
        return True
    if any(frame == "kthread" for frame in frames):
        return True
    return comm.startswith(
        (
            "kworker/",
            "migration/",
            "rcu_",
            "ksoftirqd/",
            "watchdog/",
            "cpuhp/",
            "kcompactd",
            "kswapd",
            "jbd2/",
        )
    )


def is_ext4_lock_path(row: dict) -> bool:
    """Return true when the contended lock itself is in an ext4/jbd2 path.

    A task can page fault while copy_to_user() is running under ext4.  Looking
    anywhere in that stack would incorrectly label the mmap_lock as an ext4
    lock, so only inspect frames close to the mutex/rwsem slowpath probe.
    """
    lock_path = []
    for frame in row["frames"]:
        if frame == "__irqentry_text_start":
            break
        lock_path.append(frame)
    return any("ext4_" in frame or "jbd2_" in frame for frame in lock_path)


def folded(row: dict, root: str) -> str:
    frames = list(reversed(row["frames"]))
    if not frames:
        frames = ["[user-space]"]
    return ";".join(
        [root, f"phase:{row['phase']}", f"task:{row['comm'].replace(';', ':')}", *frames]
    )


def aggregate(rows: list[dict], root: str, duration: bool) -> dict[str, int]:
    values: dict[str, int] = defaultdict(int)
    for row in rows:
        value = max(1, row["value"] // 1000) if duration else row["value"]
        values[folded(row, root)] += value
    return values


def write_folded(path: Path, values: dict[str, int]) -> None:
    with path.open("w") as output:
        for stack, value in sorted(values.items()):
            if value > 0:
                output.write(f"{stack} {value}\n")


def render_svg(
    flamegraph: Path, folded_path: Path, svg: Path, title: str, count_name: str, color: str
) -> None:
    if folded_path.stat().st_size == 0:
        svg.write_text(
            '<svg xmlns="http://www.w3.org/2000/svg" width="1800" height="120" '
            'viewBox="0 0 1800 120">\n'
            '<rect width="1800" height="120" fill="#fafafa"/>\n'
            f'<text x="900" y="48" text-anchor="middle" font-size="22">{html.escape(title)}</text>\n'
            '<text x="900" y="82" text-anchor="middle" font-size="16" fill="#555">'
            'No samples recorded</text>\n</svg>\n'
        )
        return
    with folded_path.open() as source, svg.open("w") as output:
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
                count_name,
                "--colors",
                color,
            ],
            stdin=source,
            stdout=output,
            check=True,
        )


def top_rows(values: dict[str, int], limit: int = 30) -> list[dict]:
    total = sum(values.values())
    return [
        {"stack": stack, "value": value, "percent": value * 100.0 / total if total else 0.0}
        for stack, value in sorted(values.items(), key=lambda item: item[1], reverse=True)[:limit]
    ]


def main() -> None:
    args = parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    flamegraph = args.flamegraph_dir / "flamegraph.pl"
    if not flamegraph.exists():
        raise SystemExit(f"missing FlameGraph renderer: {flamegraph}")

    phase_rows = {
        phase: parse_raw(args.raw_dir / f"{phase}.raw", phase) for phase in PHASES
    }
    times = parse_times(args.raw_dir / "profile-times.txt")
    summary: dict[str, object] = {"times": times, "cpu": {}, "wait": {}, "event_totals": {}}

    for suffix in ("all", *PHASES):
        rows = [row for phase in PHASES for row in phase_rows[phase]] if suffix == "all" else phase_rows[suffix]

        groups = {
            "cpu": ([row for row in rows if row["event"] == "cpu"], False, "linux-cpu", "samples", "hot"),
            "offcpu": ([row for row in rows if row["event"] == "offcpu"], True, "linux-offcpu", "microseconds", "chain"),
            "offcpu-workload": (
                [row for row in rows if row["event"] == "offcpu" and not is_kernel_background(row)],
                True,
                "linux-offcpu-workload",
                "microseconds",
                "chain",
            ),
            "offcpu-lock": (
                [row for row in rows if row["event"] in {"mutex", "rwsem"}],
                True,
                "linux-lock-wait",
                "microseconds",
                "chain",
            ),
            "ext4-lock-wait": (
                [
                    row
                    for row in rows
                    if row["event"] in {"mutex", "rwsem"}
                    and is_ext4_lock_path(row)
                ],
                True,
                "linux-ext4-lock-wait",
                "microseconds",
                "chain",
            ),
            "fs-io-latency": (
                [
                    row
                    for row in rows
                    if row["event"] in {"pagecache", "ext4", "block_read", "block_write"}
                ],
                True,
                "linux-fs-io-latency",
                "microseconds",
                "io",
            ),
        }

        for name, (selected, duration, root, count_name, color) in groups.items():
            values = aggregate(selected, root, duration)
            folded_path = args.out / f"linux-{name}-{suffix}.folded"
            svg = args.out / f"linux-{name}-{suffix}.svg"
            write_folded(folded_path, values)
            render_svg(
                flamegraph,
                folded_path,
                svg,
                f"Linux guest {name.replace('-', ' ')} - {suffix}",
                count_name,
                color,
            )
            section = "cpu" if name == "cpu" else "wait"
            summary[section][f"{name}-{suffix}"] = {
                "total": sum(values.values()),
                "unit": count_name,
                "top": top_rows(values),
            }

    for event in ("cpu", *sorted(DURATION_MAPS)):
        summary["event_totals"][event] = {}
        for phase in PHASES:
            values = [row["value"] for row in phase_rows[phase] if row["event"] == event]
            summary["event_totals"][event][phase] = {
                "total": sum(values),
                "unit": "samples" if event == "cpu" else "nanoseconds",
                "entries": len(values),
            }

    by_task: dict[str, dict[str, dict[str, int]]] = defaultdict(
        lambda: defaultdict(lambda: defaultdict(int))
    )
    for phase in PHASES:
        for row in phase_rows[phase]:
            by_task[row["event"]][phase][row["comm"]] += row["value"]
    summary["by_task"] = by_task
    (args.out / "summary.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
