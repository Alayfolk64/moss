#!/usr/bin/env python3
from __future__ import annotations

import argparse
import dataclasses
import re
from datetime import datetime
from pathlib import Path


@dataclasses.dataclass(frozen=True)
class Item:
    label: str
    baseline: float

    @property
    def name(self) -> str:
        return f"libc-bench {self.label} (seconds)"


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[1]
DEFAULT_OUT_DIR = SCRIPT_DIR / "results"
RUN_ALL_LOGS = (
    ("output_rv", REPO_ROOT / "docs/output_rv.txt"),
    ("output_la", REPO_ROOT / "docs/output_la.txt"),
)
UBUNTU_LOGS = (
    (
        "ubuntu_musl",
        Path("/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/musl_libcbench.log"),
    ),
    (
        "ubuntu_glibc",
        Path("/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/glibc_libcbench.log"),
    ),
)
LIBCS = ("glibc", "musl")

ITEMS = [
    Item("b_malloc_sparse (0)", 0.384919462),
    Item("b_malloc_bubble (0)", 0.360153490),
    Item("b_malloc_tiny1 (0)", 0.014186324),
    Item("b_malloc_tiny2 (0)", 0.010810043),
    Item("b_malloc_big1 (0)", 0.118578071),
    Item("b_malloc_big2 (0)", 0.107088632),
    Item("b_malloc_thread_stress (0)", 0.096325490),
    Item("b_malloc_thread_local (0)", 0.096989785),
    Item('b_string_strstr ("abcdefghijklmnopqrstuvwxyz")', 0.011402239),
    Item('b_string_strstr ("azbycxdwevfugthsirjqkplomn")', 0.020868089),
    Item('b_string_strstr ("aaaaaaaaaaaaaacccccccccccc")', 0.011944437),
    Item('b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaac")', 0.011749838),
    Item('b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac")', 0.013963025),
    Item("b_string_memset (0)", 0.025313365),
    Item("b_string_strchr (0)", 0.012811332),
    Item("b_string_strlen (0)", 0.013459429),
    Item("b_pthread_createjoin_serial1 (0)", 1.284791557),
    Item("b_pthread_createjoin_serial2 (0)", 1.006433722),
    Item("b_pthread_create_serial1 (0)", 0.892420718),
    Item("b_pthread_uselesslock (0)", 0.080551368),
    Item("b_utf8_bigbuf (0)", 0.036977802),
    Item("b_utf8_onebyone (0)", 0.109599811),
    Item("b_stdio_putcgetc (0)", 0.767822967),
    Item("b_stdio_putcgetc_unlocked (0)", 0.765147171),
    Item('b_regex_compile ("(a|b|c)*d*b")', 0.088259223),
    Item('b_regex_search ("(a|b|c)*d*b")', 0.083251251),
    Item('b_regex_search ("a{25}b")', 0.286712251),
]

ITEM_BY_NAME = {item.name: item for item in ITEMS}
ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
START_RE = re.compile(r"#### OS COMP TEST GROUP START libcbench(?:-(glibc|musl))? ####")
END_RE = re.compile(r"#### OS COMP TEST GROUP END libcbench(?:-(glibc|musl))? ####")
TIME_RE = re.compile(r"\btime:\s*([+-]?(?:\d+(?:\.\d*)?|\.\d+))")


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
    label: str | None = None
    results = {libc: {} for libc in LIBCS}

    for raw in text.splitlines():
        line = raw.strip()

        if match := START_RE.search(line):
            current_libc = match.group(1) or fallback_libc
            active = current_libc in LIBCS
            label = None
            continue

        if match := END_RE.search(line):
            end_libc = match.group(1)
            if end_libc is None or end_libc == current_libc:
                current_libc = None
                active = False
                label = None
            continue

        if not active or current_libc is None:
            continue

        if line.startswith("b_"):
            label = line
            continue

        if label and (match := TIME_RE.search(line)):
            key = f"libc-bench {label} (seconds)"
            results[current_libc][key] = float(match.group(1))
            label = None

    return {libc: values for libc, values in results.items() if values}


def calc_score(value: float | None, item: Item) -> float | None:
    if value is None or value <= 0:
        return None
    ratio = item.baseline / value
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

    existing = [(label, path) for label, path in RUN_ALL_LOGS if path.exists()]
    existing.extend((label, path) for label, path in UBUNTU_LOGS if path.exists())
    if not existing:
        expected = "、".join(str(path) for _, path in (*RUN_ALL_LOGS, *UBUNTU_LOGS))
        raise SystemExit(f"未提供日志，且未找到默认日志：{expected}")
    return existing


def fvalue(value: float | None) -> str:
    return "-" if value is None else f"{value:.6f}"


def build_report(inputs: list[tuple[str, Path]], datasets: dict[str, dict[str, dict[str, float]]]) -> str:
    now = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    lines = [
        "# libcbench 计分结果",
        "",
        f"- 生成时间: `{now}`",
        "- baseline 来自官方 `judge_libcbench-musl.py` / `judge_libcbench-glibc.py`。",
        f"- 计分项: `{len(ITEMS)}`，单个 libc 理论分数范围为 `0` 到 `< {len(ITEMS) * 2}`。",
        "- 官方 key 形如 `libc-bench <case> (seconds)`，全部按越低越好计分。",
        "- 缺失或非正数结果得 `0`；单项达到 baseline 得 `1.0`，优于 baseline 后逐渐接近 `2.0`。",
        "",
        "## 计分规则",
        "",
        "```text",
        "result <= baseline: score = 2 - result / baseline",
        "result >  baseline: score = 1",
        "missing/result <= 0: score = 0",
        "```",
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
                    "| 项目 | baseline | result | score |",
                    "|---|---:|---:|---:|",
                ]
            )
            for item in ITEMS:
                value = values.get(item.name)
                lines.append(
                    f"| {item.name} | {item.baseline:.6f} | "
                    f"{fvalue(value)} | {fvalue(calc_score(value, item))} |"
                )

            ignored = sorted(set(values) - set(ITEM_BY_NAME))
            if ignored:
                lines.extend(["", "未计分但成功解析的项目:"])
                lines.extend(f"- `{name}` = `{values[name]}`" for name in ignored)

    return "\n".join(lines) + "\n"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="解析 libcbench 日志并复现官方 judge 计分规则",
        epilog="不传日志时，默认读取 docs/output_rv.txt、docs/output_la.txt 和本机 Ubuntu 对照日志。",
    )
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
    if not any(parsed for dataset in datasets.values() for parsed in dataset.values()):
        paths = "、".join(str(path) for _, path in inputs)
        raise SystemExit(
            "日志中没有找到有效的 libcbench 结果。"
            f"已检查：{paths}。请确认 src/init.sh 中 libcbench=1，"
            "并在 make run_rv 或 make run_la 完成后重新运行本脚本。"
        )

    out = args.out or (DEFAULT_OUT_DIR / f"{datetime.now().strftime('%m%d-%H%M%S')}.md")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(build_report(inputs, datasets), encoding="utf-8")
    print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
