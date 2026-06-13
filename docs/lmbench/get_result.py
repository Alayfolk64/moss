#!/usr/bin/env python3
from __future__ import annotations

import argparse
import dataclasses
import re
from datetime import datetime
from pathlib import Path


@dataclasses.dataclass(frozen=True)
class Item:
    name: str
    baseline: float

    @property
    def lower_is_better(self) -> bool:
        # Keep this identical to generate_score() in the official judge.
        return "microseconds" in self.name or "seconds" in self.name


SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_OUT_DIR = SCRIPT_DIR / "results"
LIBCS = ("glibc", "musl")

ITEMS = [
    Item("lmbench Simple syscall:(microseconds)", 9.25013),
    Item("lmbench Simple read:(microseconds)", 16.8192),
    Item("lmbench Simple write:(microseconds)", 16.87553),
    Item("lmbench Simple stat:(microseconds)", 425.2565),
    Item("lmbench Simple fstat:(microseconds)", 29.54674),
    Item("lmbench Simple open/close:(microseconds)", 501.2352),
    Item("lmbench Select on 100 fd's:(microseconds)", 56.05561),
    Item("lmbench Signal handler installation:(microseconds)", 18.84948),
    Item("lmbench Signal handler overhead:(microseconds)", 25.89064),
    Item("lmbench Pipe latency:(microseconds)", 141.9106),
    Item("lmbench Process fork+exit:(microseconds)", 4409.188),
    Item("lmbench Process fork+execve:(microseconds)", 2004.3333),
    Item("lmbench Process fork+/bin/sh -c:(microseconds)", 920010.2),
    Item("lmbench Pagefaults on /var/tmp/XXX:(microseconds)", 978.2374643),
    Item("lmbench lat_mmap:(microseconds)", 4109),
    Item("lmbench fs latency 0k create", 24573),
    Item("lmbench fs latency 0k remove", 30703),
    Item("lmbench fs latency 1k create", 15130),
    Item("lmbench fs latency 1k remove", 255558),
    Item("lmbench fs latency 4k create", 13265),
    Item("lmbench fs latency 4k remove", 126595),
    Item("lmbench fs latency 10k create", 11723),
    Item("lmbench fs latency 10k remove", 23688),
    Item("lmbench Pipe bandwidth:(MB/sec)", 127.244),
    Item("lmbench bw_file_rd io_only", 71703.7),
    Item("lmbench bw_file_rd open2close", 2637.494),
    Item("lmbench bw_mmap_rd mmap_only", 1037.403),
    Item("lmbench bw_mmap_rd open2close", 187.0208),
    Item("lmbench context switch 2:(microseconds)", 41.24),
    Item("lmbench context switch 4:(microseconds)", 41.58071),
    Item("lmbench context switch 8:(microseconds)", 41.04231),
    Item("lmbench context switch 16:(microseconds)", 40.77077),
    Item("lmbench context switch 24:(microseconds)", 43.86385),
    Item("lmbench context switch 32:(microseconds)", 47.74923),
    Item("lmbench context switch 64:(microseconds)", 8703.026),
    Item("lmbench context switch 96:(microseconds)", 2952.14),
]

ITEM_BY_NAME = {item.name: item for item in ITEMS}
UNLABELED_BANDWIDTH_ITEMS = (
    "lmbench bw_file_rd io_only",
    "lmbench bw_file_rd open2close",
    "lmbench bw_mmap_rd mmap_only",
    "lmbench bw_mmap_rd open2close",
)
CONTEXT_COUNTS = {2, 4, 8, 16, 24, 32, 64, 96}

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
START_RE = re.compile(r"#### OS COMP TEST GROUP START lmbench-(glibc|musl) ####")
END_RE = re.compile(r"#### OS COMP TEST GROUP END lmbench-(glibc|musl) ####")
UNIT_RE = re.compile(
    r"^(?P<name>.+?:)\s*(?P<value>[+-]?(?:\d+(?:\.\d*)?|\.\d+))\s+"
    r"(?P<unit>microseconds|KB/sec|MB/sec)\s*$"
)
PAIR_RE = re.compile(
    r"^\s*(?P<first>[+-]?(?:\d+(?:\.\d*)?|\.\d+))\s+"
    r"(?P<second>[+-]?(?:\d+(?:\.\d*)?|\.\d+))\s*$"
)
FS_RE = re.compile(
    r"^(?P<size>0k|1k|4k|10k)\s+\S+\s+(?P<create>-?\d+)\s+(?P<remove>-?\d+)\s*$"
)


def infer_libc(path: Path) -> str | None:
    name = path.name.lower()
    for libc in LIBCS:
        if libc in name:
            return libc
    return None


def parse_log(path: Path) -> dict[str, dict[str, float]]:
    text = ANSI_RE.sub("", path.read_text(encoding="utf-8", errors="replace"))
    fallback_libc = infer_libc(path)
    current_libc: str | None = None
    active = False
    state = ""
    lat_mmap_seen = False
    bandwidth_index = 0
    results = {libc: {} for libc in LIBCS}

    for raw in text.splitlines():
        line = raw.strip()

        if match := START_RE.search(line):
            current_libc = match.group(1)
            active = True
            state = ""
            lat_mmap_seen = False
            bandwidth_index = 0
            continue

        if match := END_RE.search(line):
            if current_libc == match.group(1):
                current_libc = None
                active = False
                state = ""
            continue

        if "latency measurements" in line:
            if current_libc is None:
                current_libc = fallback_libc
            active = current_libc in LIBCS
            state = "latency"
            lat_mmap_seen = False
            bandwidth_index = 0
            continue

        if not active or current_libc is None:
            continue

        if "file system latency" in line:
            state = "fs"
            continue
        if "Bandwidth measurements" in line:
            state = "bandwidth"
            bandwidth_index = 0
            continue
        if "context switch overhead" in line:
            state = "context"
            continue

        if match := UNIT_RE.match(line):
            key = f"lmbench {match.group('name')}({match.group('unit')})"
            results[current_libc][key] = float(match.group("value"))
            continue

        if state == "fs" and (match := FS_RE.match(line)):
            size = match.group("size")
            results[current_libc][f"lmbench fs latency {size} create"] = float(match.group("create"))
            results[current_libc][f"lmbench fs latency {size} remove"] = float(match.group("remove"))
            continue

        if match := PAIR_RE.match(line):
            first = float(match.group("first"))
            second = float(match.group("second"))
            if state == "latency" and not lat_mmap_seen:
                results[current_libc]["lmbench lat_mmap:(microseconds)"] = second
                lat_mmap_seen = True
            elif state == "bandwidth" and bandwidth_index < len(UNLABELED_BANDWIDTH_ITEMS):
                results[current_libc][UNLABELED_BANDWIDTH_ITEMS[bandwidth_index]] = second
                bandwidth_index += 1
            elif state == "context" and first.is_integer() and int(first) in CONTEXT_COUNTS:
                results[current_libc][f"lmbench context switch {int(first)}:(microseconds)"] = second

    return {libc: values for libc, values in results.items() if values}


def calc_score(value: float | None, item: Item) -> float | None:
    if value is None or value <= 0:
        return None
    ratio = item.baseline / value if item.lower_is_better else value / item.baseline
    return 2 - 1 / ratio if ratio >= 1 else 1.0


def parse_input(value: str) -> tuple[str, Path]:
    if "=" not in value:
        raise argparse.ArgumentTypeError("--input 必须使用 NAME=PATH 格式")
    label, raw_path = value.split("=", 1)
    if not label.strip() or not raw_path.strip():
        raise argparse.ArgumentTypeError("--input 的名称和路径都不能为空")
    return label.strip(), Path(raw_path)


def collect_inputs(args: argparse.Namespace) -> list[tuple[str, Path]]:
    inputs = list(args.input)
    inputs.extend((path.stem, path) for path in args.logs)
    if inputs:
        return inputs

    defaults = [
        ("RV", (SCRIPT_DIR / "../../output_rv.txt").resolve()),
        ("LA", (SCRIPT_DIR / "../../output_la.txt").resolve()),
    ]
    existing = [(label, path) for label, path in defaults if path.exists()]
    if not existing:
        raise SystemExit("未提供日志，且仓库根目录不存在 output_rv.txt/output_la.txt")
    return existing


def fvalue(value: float | None) -> str:
    return "-" if value is None else f"{value:.6f}"


def build_report(inputs: list[tuple[str, Path]], datasets: dict[str, dict[str, dict[str, float]]]) -> str:
    now = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    lines = [
        "# lmbench 计分结果",
        "",
        f"- 生成时间: `{now}`",
        f"- 计分项: `{len(ITEMS)}`，单个 libc 理论分数范围为 `0` 到 `< {len(ITEMS) * 2}`。",
        "- 单项达到 baseline 得 `1.0` 分；优于 baseline 后按曲线增加，极限趋近 `2.0` 分。",
        "- 官方 judge 通过名称是否含 `microseconds`/`seconds` 判断方向；因此 8 个 `fs latency` 项实际按“越大越好”计分。",
        "- `File /var/tmp/XXX write bandwidth` 和 `Protection fault` 不在 baseline 中，不计分。",
        "",
        "## 输入",
        "",
    ]
    for label, path in inputs:
        lines.append(f"- {label}: `{path}`")

    lines.extend(
        [
            "",
            "## 汇总",
            "",
            "| 输入 | libc | 已解析 | 缺失 | 总分 |",
            "|---|---|---:|---:|---:|",
        ]
    )

    for label, _ in inputs:
        parsed = datasets[label]
        for libc in LIBCS:
            values = parsed.get(libc)
            if not values:
                continue
            present = sum(item.name in values and values[item.name] > 0 for item in ITEMS)
            total = sum(calc_score(values.get(item.name), item) or 0 for item in ITEMS)
            lines.append(f"| {label} | {libc} | {present} | {len(ITEMS) - present} | {total:.6f} |")

    for label, _ in inputs:
        parsed = datasets[label]
        for libc in LIBCS:
            values = parsed.get(libc)
            if not values:
                continue
            lines.extend(
                [
                    "",
                    f"## {label} / {libc}",
                    "",
                    "| 项目 | 方向（judge） | baseline | result | score |",
                    "|---|---|---:|---:|---:|",
                ]
            )
            for item in ITEMS:
                value = values.get(item.name)
                direction = "越低越好" if item.lower_is_better else "越高越好"
                lines.append(
                    f"| {item.name} | {direction} | {item.baseline:.6f} | "
                    f"{fvalue(value)} | {fvalue(calc_score(value, item))} |"
                )

            ignored = sorted(set(values) - set(ITEM_BY_NAME))
            if ignored:
                lines.extend(["", "未计分但成功解析的项目:"])
                lines.extend(f"- `{name}` = `{values[name]}`" for name in ignored)

    return "\n".join(lines) + "\n"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="解析 lmbench 日志并复现官方 judge 计分规则")
    parser.add_argument("logs", nargs="*", type=Path, help="日志路径；标签默认使用文件名")
    parser.add_argument(
        "--input",
        action="append",
        default=[],
        type=parse_input,
        metavar="NAME=PATH",
        help="带自定义标签的日志，可重复指定",
    )
    parser.add_argument("--out", type=Path, default=None, help="Markdown 报告输出路径")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    inputs = collect_inputs(args)
    missing_paths = [str(path) for _, path in inputs if not path.exists()]
    if missing_paths:
        raise SystemExit("日志不存在: " + ", ".join(missing_paths))

    datasets = {label: parse_log(path) for label, path in inputs}
    out = args.out or (DEFAULT_OUT_DIR / f"{datetime.now().strftime('%m%d-%H%M%S')}.md")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(build_report(inputs, datasets), encoding="utf-8")
    print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
