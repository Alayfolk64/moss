#!/usr/bin/env python3
from __future__ import annotations

import argparse
import dataclasses
import re
from datetime import datetime
from pathlib import Path


BASELINE_LOG = """
====== cyclictest NO_STRESS_P1 begin ======
WARN: stat /dev/cpu_dma_latency failed: No such file or directory
T: 0 (  271) P:99 I:1000 C:    896 Min:    107 Act:  214 Avg:  563 Max:    1862
====== cyclictest NO_STRESS_P1 end: success ======
====== cyclictest NO_STRESS_P8 begin ======
WARN: stat /dev/cpu_dma_latency failed: No such file or directory
T: 0 (  273) P:99 I:1000 C:    995 Min:     97 Act:  186 Avg:  303 Max:    1917
T: 1 (  274) P:99 I:1500 C:    666 Min:    135 Act:  252 Avg:  325 Max:    2021
T: 2 (  275) P:99 I:2000 C:    498 Min:     90 Act:  165 Avg:  416 Max:    1274
T: 3 (  276) P:99 I:2500 C:    399 Min:    150 Act:  184 Avg:  350 Max:    1164
T: 4 (  277) P:99 I:3000 C:    333 Min:    108 Act:  618 Avg:  266 Max:    1539
T: 5 (  278) P:99 I:3500 C:    285 Min:    125 Act:  644 Avg:  373 Max:    1882
T: 6 (  279) P:99 I:4000 C:    249 Min:    102 Act:  247 Avg:  479 Max:    1362
T: 7 (  280) P:99 I:4500 C:    221 Min:    138 Act:  255 Avg:  405 Max:    1306
====== cyclictest NO_STRESS_P8 end: success ======
====== start hackbench ======
Running in process mode with 10 groups using 40 file descriptors each (== 400 tasks)
Each sender will pass 100000000 messages of 100 bytes
====== cyclictest STRESS_P1 begin ======
WARN: stat /dev/cpu_dma_latency failed: No such file or directory
T: 0 (  684) P:99 I:1000 C:    986 Min:     87 Act:  368 Avg:  389 Max:    1310
====== cyclictest STRESS_P1 end: success ======
====== cyclictest STRESS_P8 begin ======
WARN: stat /dev/cpu_dma_latency failed: No such file or directory
T: 0 (  687) P:99 I:1000 C:    971 Min:     51 Act:  150 Avg:  454 Max:    1950
T: 1 (  688) P:99 I:1500 C:    661 Min:    136 Act:  192 Avg:  473 Max:    1539
T: 2 (  689) P:99 I:2000 C:    495 Min:    152 Act:  242 Avg:  571 Max:    1515
T: 3 (  690) P:99 I:2500 C:    379 Min:    140 Act:  364 Avg:  434 Max:    1656
T: 4 (  691) P:99 I:3000 C:    317 Min:     97 Act:  320 Avg:  523 Max:    1375
T: 5 (  692) P:99 I:3500 C:    249 Min:    156 Act:  299 Avg:  484 Max:    1239
T: 6 (  693) P:99 I:4000 C:    225 Min:    153 Act:  266 Avg:  512 Max:    1468
T: 7 (  694) P:99 I:4500 C:     89 Min: 100000 Act:  213 Avg:  473 Max:    0
====== cyclictest STRESS_P8 end: success ======
Signal 2 caught, longjmp'ing out!
longjmp'ed out, reaping children
sending SIGTERM to all child processes
signaling 400 worker threads to terminate
Time: 9.813
====== kill hackbench: success ======
"""


@dataclasses.dataclass(frozen=True)
class Item:
    name: str
    baseline: float


SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_RV = (SCRIPT_DIR / "../../docs/output_rv.txt").resolve()
DEFAULT_LA = (SCRIPT_DIR / "../../docs/output_la.txt").resolve()
DEFAULT_OUT_DIR = SCRIPT_DIR / "results"
UBUNTU_LOGS = (
    (
        "ubuntu_musl",
        Path("/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/musl_cyclictest.log"),
    ),
    (
        "ubuntu_glibc",
        Path("/Users/wuxun/os/riscv-ubuntu26/share/ubuntu-perf-results/20260608-021349/glibc_cyclictest.log"),
    ),
)
RUN_ALL_LOGS = (("rv", DEFAULT_RV), ("la", DEFAULT_LA))
LIBCS = ("musl", "glibc")

ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
START_RE = re.compile(r"#### OS COMP TEST GROUP START cyclictest-(musl|glibc) ####")
END_RE = re.compile(r"#### OS COMP TEST GROUP END cyclictest-(musl|glibc) ####")
BEGIN_RE = re.compile(r"====== cyclictest (NO_STRESS_P1|NO_STRESS_P8|STRESS_P1|STRESS_P8) begin ======")
SUCCESS_RE = re.compile(r"====== cyclictest (NO_STRESS_P1|NO_STRESS_P8|STRESS_P1|STRESS_P8) end: success ======")
T_RE = re.compile(r"T: .+ Min:\s+(\d+).+Max:\s+(\d+)")
KILL_SUCCESS = "kill hackbench: success"


def parse_cyclictest_text(text: str, require_kill_success: bool = True) -> dict[str, float]:
    results: dict[str, float] = {}
    key = ""
    values: list[int] = []
    kill_success = False

    for raw in ANSI_RE.sub("", text).splitlines():
        line = raw.strip()
        if m := BEGIN_RE.search(line):
            key = f"cyclictest {m.group(1)} (microseconds)"
        elif SUCCESS_RE.search(line):
            if values and key:
                results[key] = sum(values) / len(values)
                values = []
        elif KILL_SUCCESS in line:
            kill_success = True

        if m := T_RE.search(line):
            values.append(max(int(m.group(1)), int(m.group(2))))

    if require_kill_success and not kill_success:
        return {}
    return results


ITEMS = [Item(name, baseline) for name, baseline in parse_cyclictest_text(BASELINE_LOG).items()]
ITEM_BY_NAME = {item.name: item for item in ITEMS}


def infer_libc(path: Path) -> str | None:
    name = path.name.lower()
    if "musl" in name:
        return "musl"
    if "glibc" in name:
        return "glibc"
    return None


def parse_log(path: Path) -> dict[str, dict[str, float]]:
    if not path.exists():
        return {}

    text = path.read_text(encoding="utf-8", errors="replace")
    datasets: dict[str, dict[str, float]] = {}
    current_libc: str | None = None
    section: list[str] = []
    seen_libcs: set[str] = set()

    saw_marker = False
    for raw in text.splitlines():
        if m := START_RE.search(raw):
            saw_marker = True
            current_libc = m.group(1)
            seen_libcs.add(current_libc)
            section = [raw]
            continue
        if current_libc is not None:
            section.append(raw)
            if m := END_RE.search(raw):
                if m.group(1) == current_libc:
                    parsed = parse_cyclictest_text("\n".join(section))
                    if parsed:
                        datasets[current_libc] = parsed
                    current_libc = None
                    section = []
            continue

    if saw_marker:
        for libc in seen_libcs:
            datasets.setdefault(libc, {})
        return datasets

    libc = infer_libc(path)
    if libc is None:
        return {}
    parsed = parse_cyclictest_text(text)
    return {libc: parsed}


def calc_score(value: float | None, item: Item) -> float | None:
    if value is None:
        return None
    if value <= 0:
        return 0.0
    ratio = item.baseline / value
    if ratio >= 1:
        return 2 - (1 / ratio)
    return 1.0


def parse_input(raw: str) -> tuple[str, Path]:
    if "=" not in raw:
        raise argparse.ArgumentTypeError("输入格式应为 NAME=PATH")
    name, path = raw.split("=", 1)
    if not name:
        raise argparse.ArgumentTypeError("NAME 不能为空")
    return name, Path(path)


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
        "# cyclictest 计分结果",
        "",
        f"- 生成时间: `{now}`",
        "- baseline 和公式来自官方 `judge_cyclictest-musl.py` / `judge_cyclictest-glibc.py`。",
        "- 官方解析只记录 `end: success` 的测试段，且必须有 `kill hackbench: success`；否则整组为 0 分。",
        "- 每段 result 为该段所有 `T:` 行的 `max(Max, Min)` 平均值，单位 `microseconds`，越低越好。",
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
            if values is None:
                continue
            present = sum(item.name in values and values[item.name] > 0 for item in ITEMS)
            total = sum(calc_score(values.get(item.name), item) or 0 for item in ITEMS)
            lines.append(f"| {label} | {libc} | {present} | {len(ITEMS) - present} | {total:.6f} |")

    for label, _ in inputs:
        parsed = datasets[label]
        for libc in LIBCS:
            values = parsed.get(libc)
            if values is None:
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
        description="解析 cyclictest 日志并复现官方 judge 计分规则",
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
    out = args.out or (DEFAULT_OUT_DIR / f"{datetime.now().strftime('%m%d-%H%M%S')}.md")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(build_report(inputs, datasets), encoding="utf-8")
    print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
