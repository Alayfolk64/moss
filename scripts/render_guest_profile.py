#!/usr/bin/env python3
"""Render Moss guest CPU and wait profiles from a serial log."""

from __future__ import annotations

import argparse
import html
import json
import re
import subprocess
from collections import defaultdict
from pathlib import Path


PHASE_NAMES = {1: "setup", 2: "prebuild", 3: "timed"}
EVENT_NAMES = {
    1: "mutex-wait",
    2: "ext4-inclusive",
    3: "page-cache-inclusive",
    4: "block-read-sync",
    5: "block-write-sync",
    6: "off-cpu-all",
    7: "ext4-global-lock-wait",
    8: "folio-load-wait",
    9: "invalidate-read-wait",
    10: "invalidate-write-wait",
    11: "mprotect-total",
    12: "mprotect-aspace-lock-wait",
    13: "mprotect-seal-lock-wait",
    14: "mprotect-seal-scan",
    15: "mprotect-local-update",
    16: "mprotect-record-lock-wait",
    17: "mprotect-record-update",
}
MPROTECT_PAGE_BUCKET_LABELS = (
    "0",
    "1",
    "2-4",
    "5-16",
    "17-64",
    "65-256",
    "257-1024",
    "1025+",
)
MPROTECT_PROT_BUCKET_LABELS = (
    "none",
    "read",
    "write",
    "read-write",
    "exec",
    "read-exec",
    "write-exec",
    "read-write-exec",
)
INSTRUMENTATION_FRAMES = (
    "starry_kernel::profiler::",
    "axtask::future::profile_block_",
    "axsync::profile::profile_wait_",
    "axplat::percpu::this_cpu_id",
)
BACKGROUND_TASKS = {"gc", "rebalance-task", "profile-watchdog"}
BACKGROUND_STACK_MARKERS = (
    "axfs::highlevel::file::start_readahead_workers::",
    "axfs::fs::ext4::block_io::SharedBlockDevice::ensure_completion_task::",
)

CPU_RE = re.compile(r"MOSS_CPU phase=(\d+) samples=(\d+) task=([0-9a-f]*) stack=(.*)")
WAIT_RE = re.compile(
    r"MOSS_WAIT phase=(\d+) event=(\d+) count=(\d+) "
    r"total_ns=(\d+) max_ns=(\d+) task=([0-9a-f]*) stack=(.*)"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--elf", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--addr2line", default="addr2line")
    parser.add_argument("--flamegraph-dir", type=Path, default=Path("/tmp/moss-flamegraph"))
    return parser.parse_args()


def parse_stack(raw: str) -> tuple[str, ...]:
    return tuple(frame for frame in raw.strip().split(";") if frame)


def parse_task_name(raw: str) -> str:
    return bytes.fromhex(raw).decode(errors="replace") if raw else "unknown"


def read_profile(log: Path) -> tuple[list[dict], list[dict], dict, dict[int, dict]]:
    cpu: list[dict] = []
    waits: list[dict] = []
    header: dict[str, int] = {}
    mprotect: dict[int, dict] = {}
    inside = False
    for raw_line in log.read_text(errors="replace").splitlines():
        line = raw_line.replace("\r", "")
        if "#### MOSS GUEST PROFILE START ####" in line:
            inside = True
            continue
        if "#### MOSS GUEST PROFILE END ####" in line:
            break
        if not inside:
            continue
        if line.startswith("MOSS_PROFILE_V1"):
            for key, value in re.findall(r"(\w+)=(\d+)", line):
                header[key] = int(value)
            continue
        if line.startswith("MOSS_MPROTECT "):
            fields = dict(field.split("=", 1) for field in line.split()[1:])
            phase = int(fields.pop("phase"))
            page_buckets = [int(value) for value in fields.pop("page_buckets").split(",")]
            prot_buckets = [int(value) for value in fields.pop("prot_buckets").split(",")]
            mprotect[phase] = {
                **{key: int(value) for key, value in fields.items()},
                "page_buckets": dict(zip(MPROTECT_PAGE_BUCKET_LABELS, page_buckets)),
                "prot_buckets": dict(zip(MPROTECT_PROT_BUCKET_LABELS, prot_buckets)),
            }
            continue
        match = CPU_RE.search(line)
        if match:
            cpu.append(
                {
                    "phase": int(match.group(1)),
                    "samples": int(match.group(2)),
                    "task": parse_task_name(match.group(3)),
                    "stack": parse_stack(match.group(4)),
                }
            )
            continue
        match = WAIT_RE.search(line)
        if match:
            waits.append(
                {
                    "phase": int(match.group(1)),
                    "event": int(match.group(2)),
                    "count": int(match.group(3)),
                    "total_ns": int(match.group(4)),
                    "max_ns": int(match.group(5)),
                    "task": parse_task_name(match.group(6)),
                    "stack": parse_stack(match.group(7)),
                }
            )
    if not cpu and not waits:
        raise SystemExit("no complete MOSS guest profile found in the log")
    return cpu, waits, header, mprotect


def symbolize(addresses: set[str], elf: Path, addr2line: str) -> dict[str, str]:
    numeric = sorted(address for address in addresses if address != "user")
    if not numeric:
        return {"user": "[user-space]"}
    process = subprocess.run(
        [addr2line, "-C", "-f", "-e", str(elf)],
        input="\n".join(numeric) + "\n",
        text=True,
        capture_output=True,
        check=True,
    )
    lines = process.stdout.splitlines()
    if len(lines) != len(numeric) * 2:
        raise SystemExit("unexpected addr2line output")
    symbols = {"user": "[user-space]"}
    for index, address in enumerate(numeric):
        function = lines[index * 2].strip()
        location = lines[index * 2 + 1].strip()
        if function in {"", "??"}:
            function = address
        if location not in {"", "??:0", "??:?"}:
            location = location.replace(str(Path.cwd()) + "/", "")
            function = f"{function} ({location})"
        symbols[address] = function.replace(";", ":").replace("\n", " ")
    return symbols


def folded_stack(
    stack: tuple[str, ...], symbols: dict[str, str], phase: int, task: str, root: str
) -> str:
    frames = []
    for address in reversed(stack):
        frame = symbols.get(address, address)
        if not any(marker in frame for marker in INSTRUMENTATION_FRAMES):
            frames.append(frame)
    task = task.replace(";", ":").replace("\n", " ")
    return ";".join(
        [root, f"phase:{PHASE_NAMES.get(phase, phase)}", f"task:{task}", *frames]
    )


def write_folded(path: Path, values: dict[str, int]) -> None:
    with path.open("w") as output:
        for stack, value in sorted(values.items()):
            if value > 0:
                output.write(f"{stack} {value}\n")


def render_svg(
    flamegraph: Path, folded: Path, svg: Path, title: str, count_name: str, color: str
) -> None:
    if folded.stat().st_size == 0:
        svg.write_text(
            '<svg xmlns="http://www.w3.org/2000/svg" width="1800" height="120" '
            'viewBox="0 0 1800 120">\n'
            '<rect width="1800" height="120" fill="#fafafa"/>\n'
            f'<text x="900" y="48" text-anchor="middle" font-size="22">'
            f'{html.escape(title)}</text>\n'
            '<text x="900" y="82" text-anchor="middle" font-size="16" '
            'fill="#555">No samples recorded</text>\n'
            '</svg>\n'
        )
        return
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
    cpu, waits, header, mprotect = read_profile(args.log)
    addresses = {
        address
        for row in [*cpu, *waits]
        for address in row["stack"]
    }
    symbols = symbolize(addresses, args.elf, args.addr2line)

    cpu_values: dict[tuple[int | None, str], dict[str, int]] = defaultdict(
        lambda: defaultdict(int)
    )
    wait_values: dict[tuple[int | None, int | None], dict[str, int]] = defaultdict(
        lambda: defaultdict(int)
    )
    workload_offcpu_values: dict[int | None, dict[str, int]] = defaultdict(
        lambda: defaultdict(int)
    )
    for row in cpu:
        stack = folded_stack(
            row["stack"], symbols, row["phase"], row["task"], "guest-cpu"
        )
        cpu_values[(None, "cpu")][stack] += row["samples"]
        cpu_values[(row["phase"], "cpu")][stack] += row["samples"]
    for row in waits:
        stack = folded_stack(
            row["stack"],
            symbols,
            row["phase"],
            row["task"],
            f"guest-wait:{EVENT_NAMES.get(row['event'], row['event'])}",
        )
        microseconds = max(1, row["total_ns"] // 1000)
        wait_values[(None, None)][stack] += microseconds
        wait_values[(row["phase"], None)][stack] += microseconds
        wait_values[(None, row["event"])][stack] += microseconds
        wait_values[(row["phase"], row["event"])][stack] += microseconds
        background_stack = any(marker in stack for marker in BACKGROUND_STACK_MARKERS)
        if (
            row["event"] == 6
            and row["task"] not in BACKGROUND_TASKS
            and not background_stack
        ):
            workload_offcpu_values[None][stack] += microseconds
            workload_offcpu_values[row["phase"]][stack] += microseconds

    flamegraph = args.flamegraph_dir / "flamegraph.pl"
    if not flamegraph.exists():
        raise SystemExit(f"missing FlameGraph renderer: {flamegraph}")

    summary: dict[str, object] = {
        "header": header,
        "cpu": {},
        "wait": {},
        "mprotect": {
            PHASE_NAMES[phase]: values for phase, values in mprotect.items()
        },
    }
    for phase in [None, 1, 2, 3]:
        suffix = "all" if phase is None else PHASE_NAMES[phase]
        values = cpu_values[(phase, "cpu")]
        folded = args.out / f"guest-cpu-{suffix}.folded"
        svg = args.out / f"guest-cpu-{suffix}.svg"
        write_folded(folded, values)
        render_svg(flamegraph, folded, svg, f"Moss guest CPU - {suffix}", "samples", "hot")
        summary["cpu"][suffix] = {"total_samples": sum(values.values()), "top": top_rows(values)}

        mutex_values = wait_values[(phase, 1)]
        folded = args.out / f"guest-offcpu-lock-{suffix}.folded"
        svg = args.out / f"guest-offcpu-lock-{suffix}.svg"
        write_folded(folded, mutex_values)
        render_svg(
            flamegraph,
            folded,
            svg,
            f"Moss guest off-CPU mutex wait - {suffix}",
            "microseconds",
            "chain",
        )
        summary["wait"][f"mutex-{suffix}"] = {
            "total_us": sum(mutex_values.values()),
            "top": top_rows(mutex_values),
        }

        ext4_lock_values = wait_values[(phase, 7)]
        folded = args.out / f"guest-ext4-lock-wait-{suffix}.folded"
        svg = args.out / f"guest-ext4-lock-wait-{suffix}.svg"
        write_folded(folded, ext4_lock_values)
        render_svg(
            flamegraph,
            folded,
            svg,
            f"Moss guest ext4 global-lock wait - {suffix}",
            "microseconds",
            "chain",
        )
        summary["wait"][f"ext4-lock-{suffix}"] = {
            "total_us": sum(ext4_lock_values.values()),
            "top": top_rows(ext4_lock_values),
        }

        for event, label, title in (
            (8, "folio-load", "folio load wait"),
            (9, "invalidate-read", "invalidation read-lock wait"),
            (10, "invalidate-write", "invalidation write-lock wait"),
        ):
            event_values = wait_values[(phase, event)]
            folded = args.out / f"guest-{label}-wait-{suffix}.folded"
            svg = args.out / f"guest-{label}-wait-{suffix}.svg"
            write_folded(folded, event_values)
            render_svg(
                flamegraph,
                folded,
                svg,
                f"Moss guest {title} - {suffix}",
                "microseconds",
                "chain",
            )
            summary["wait"][f"{label}-{suffix}"] = {
                "total_us": sum(event_values.values()),
                "top": top_rows(event_values),
            }

        for event, label, title in (
            (11, "mprotect-total", "mprotect total latency"),
            (12, "mprotect-aspace-lock-wait", "mprotect address-space lock wait"),
            (13, "mprotect-seal-lock-wait", "mprotect seal-check record-lock wait"),
            (14, "mprotect-seal-scan", "mprotect seal-check record scan"),
            (15, "mprotect-local-update", "mprotect per-mm VMA and page-table update"),
            (16, "mprotect-record-lock-wait", "mprotect update record-lock wait"),
            (17, "mprotect-record-update", "mprotect mapping-record rebuild"),
        ):
            event_values = wait_values[(phase, event)]
            folded = args.out / f"guest-{label}-{suffix}.folded"
            svg = args.out / f"guest-{label}-{suffix}.svg"
            write_folded(folded, event_values)
            render_svg(
                flamegraph,
                folded,
                svg,
                f"Moss guest {title} - {suffix}",
                "microseconds",
                "chain",
            )
            summary["wait"][f"{label}-{suffix}"] = {
                "total_us": sum(event_values.values()),
                "top": top_rows(event_values),
            }

        offcpu_values = wait_values[(phase, 6)]
        folded = args.out / f"guest-offcpu-{suffix}.folded"
        svg = args.out / f"guest-offcpu-{suffix}.svg"
        write_folded(folded, offcpu_values)
        render_svg(
            flamegraph,
            folded,
            svg,
            f"Moss guest total off-CPU time - {suffix}",
            "microseconds",
            "chain",
        )
        summary["wait"][f"offcpu-{suffix}"] = {
            "total_us": sum(offcpu_values.values()),
            "top": top_rows(offcpu_values),
        }

        workload_values = workload_offcpu_values[phase]
        folded = args.out / f"guest-offcpu-workload-{suffix}.folded"
        svg = args.out / f"guest-offcpu-workload-{suffix}.svg"
        write_folded(folded, workload_values)
        render_svg(
            flamegraph,
            folded,
            svg,
            f"Moss guest workload off-CPU time - {suffix}",
            "microseconds",
            "chain",
        )
        summary["wait"][f"offcpu-workload-{suffix}"] = {
            "excluded_tasks": sorted(BACKGROUND_TASKS),
            "excluded_stack_markers": list(BACKGROUND_STACK_MARKERS),
            "total_us": sum(workload_values.values()),
            "top": top_rows(workload_values),
        }

        latency_values: dict[str, int] = defaultdict(int)
        for event in (2, 3, 4, 5):
            for stack, value in wait_values[(phase, event)].items():
                latency_values[stack] += value
        folded = args.out / f"guest-fs-io-latency-{suffix}.folded"
        svg = args.out / f"guest-fs-io-latency-{suffix}.svg"
        write_folded(folded, latency_values)
        render_svg(
            flamegraph,
            folded,
            svg,
            f"Moss guest ext4/page-cache/synchronous-I/O latency - {suffix}",
            "microseconds",
            "io",
        )
        summary["wait"][f"fs-io-{suffix}"] = {
            "total_us_inclusive": sum(latency_values.values()),
            "top": top_rows(latency_values),
        }

    event_totals = {}
    for event, name in EVENT_NAMES.items():
        event_totals[name] = {
            PHASE_NAMES[phase]: sum(wait_values[(phase, event)].values())
            for phase in (1, 2, 3)
        }
    summary["event_totals_us"] = event_totals

    cpu_by_task: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    wait_by_task: dict[str, dict[str, dict[str, int]]] = defaultdict(
        lambda: defaultdict(lambda: defaultdict(int))
    )
    for row in cpu:
        cpu_by_task[PHASE_NAMES[row["phase"]]][row["task"]] += row["samples"]
    for row in waits:
        event_name = EVENT_NAMES.get(row["event"], str(row["event"]))
        wait_by_task[event_name][PHASE_NAMES[row["phase"]]][row["task"]] += (
            row["total_ns"] // 1000
        )
    summary["cpu_samples_by_task"] = cpu_by_task
    summary["wait_us_by_event_phase_task"] = wait_by_task

    event_stats: dict[str, dict[str, dict[str, int]]] = defaultdict(dict)
    for event, name in EVENT_NAMES.items():
        for phase, phase_name in PHASE_NAMES.items():
            rows = [
                row for row in waits if row["event"] == event and row["phase"] == phase
            ]
            event_stats[name][phase_name] = {
                "estimated_total_us": sum(row["total_ns"] for row in rows) // 1000,
                "sampled_count": sum(row["count"] for row in rows),
                "max_sample_us": max((row["max_ns"] for row in rows), default=0) // 1000,
            }
    summary["event_stats"] = event_stats
    (args.out / "summary.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
