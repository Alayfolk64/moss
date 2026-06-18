#!/usr/bin/env python3
from __future__ import annotations

import argparse
import dataclasses
import re
from datetime import datetime
from pathlib import Path
from typing import Iterable


@dataclasses.dataclass(frozen=True)
class Item:
    libc: str
    test: str
    metric: str
    baseline: float

    @property
    def key(self) -> tuple[str, str, str]:
        return (self.libc, self.test, self.metric)

    @property
    def name(self) -> str:
        return f"netperf {self.test} ({self.metric})"


SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_RV = (SCRIPT_DIR / "../../docs/output_rv.txt").resolve()
DEFAULT_LA = (SCRIPT_DIR / "../../docs/output_la.txt").resolve()
DEFAULT_OUT_DIR = SCRIPT_DIR / "results"
LIBCS = ("glibc", "musl")

BASE_ITEMS = [
    ("UDP_STREAM", "Trans. Rate/sec", 27.50),
    ("TCP_STREAM", "Throughput 10^6bits/sec", 79.75),
    ("UDP_RR", "Trans. Rate/sec", 2070.62),
    ("TCP_RR", "Trans. Rate/sec", 2017.98),
    ("TCP_CRR", "Trans. Rate/sec", 390.53),
]
ITEMS = [Item(libc, test, metric, baseline) for libc in LIBCS for test, metric, baseline in BASE_ITEMS]

ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
START_RE = re.compile(r"#### OS COMP TEST GROUP START netperf-(glibc|musl) ####")
END_RE = re.compile(r"#### OS COMP TEST GROUP END netperf-(glibc|musl) ####")
BEGIN_RE = re.compile(r"====== netperf (UDP_STREAM|TCP_STREAM|UDP_RR|TCP_RR|TCP_CRR) begin ======")


def infer_libc(path: Path) -> str | None:
    name = path.name.lower()
    if "glibc" in name:
        return "glibc"
    if "musl" in name:
        return "musl"
    return None


def is_int(text: str) -> bool:
    try:
        int(text)
        return True
    except ValueError:
        return False


def parse_logs(paths: Iterable[Path]) -> dict[tuple[str, str, str], float]:
    results: dict[tuple[str, str, str], float] = {}
    for path in paths:
        if not path.exists():
            continue

        text = ANSI_RE.sub("", path.read_text(encoding="utf-8", errors="replace"))
        libc = infer_libc(path)
        current_test: str | None = None

        for raw in text.splitlines():
            line = raw.strip()

            if m := START_RE.search(line):
                libc = m.group(1)
                current_test = None
                continue

            if m := END_RE.search(line):
                if libc == m.group(1):
                    current_test = None
                continue

            if m := BEGIN_RE.search(line):
                current_test = m.group(1)
                continue

            if libc is None or current_test is None:
                continue

            cols = line.split()
            if (len(cols) == 5 or len(cols) == 6) and is_int(cols[0]):
                metric = "Throughput 10^6bits/sec" if len(cols) == 5 else "Trans. Rate/sec"
                results[(libc, current_test, metric)] = float(cols[-1])
                current_test = None

    return results


def calc_score(value: float | None, baseline: float) -> float | None:
    if value is None:
        return None
    if value <= 0:
        return 0.0
    ratio = value / baseline
    if ratio >= 1:
        return 2 - 1 / ratio
    return 1.0


def f2(value: float | None) -> str:
    return "-" if value is None else f"{value:.2f}"


def fs(value: float | None) -> str:
    return "-" if value is None else f"{value:.6f}"


def fmt_paths(paths: list[Path]) -> str:
    return ", ".join(f"`{p}`" for p in paths)


def build_report(
    rv_paths: list[Path],
    la_paths: list[Path],
    rv: dict[tuple[str, str, str], float],
    la: dict[tuple[str, str, str], float],
) -> str:
    now = datetime.now().strftime("%m%d-%H%M%S")
    lines = [
        "# netperf 结果",
        "",
        f"- 时间: `{now}`",
        f"- RV: {fmt_paths(rv_paths)}",
        f"- LA: {fmt_paths(la_paths)}",
        "- result 按评测脚本逻辑提取：测试段内第一条 5/6 列数字行的最后一列。",
        "",
        "## 总分",
        "",
        "| libc | RV | LA |",
        "|---|---:|---:|",
    ]

    for libc in LIBCS:
        rv_total = 0.0
        la_total = 0.0
        rv_missing = 0
        la_missing = 0
        for item in [i for i in ITEMS if i.libc == libc]:
            rv_score = calc_score(rv.get(item.key), item.baseline)
            la_score = calc_score(la.get(item.key), item.baseline)
            if rv_score is None:
                rv_missing += 1
            else:
                rv_total += rv_score
            if la_score is None:
                la_missing += 1
            else:
                la_total += la_score
        rv_text = f"- (缺 {rv_missing} 项)" if rv_missing else fs(rv_total)
        la_text = f"- (缺 {la_missing} 项)" if la_missing else fs(la_total)
        lines.append(f"| {libc} | {rv_text} | {la_text} |")

    for libc in LIBCS:
        lines.extend([
            "",
            f"## {libc}",
            "",
            "| 项目 | baseline | RV result | RV score | LA result | LA score |",
            "|---|---:|---:|---:|---:|---:|",
        ])
        for item in [i for i in ITEMS if i.libc == libc]:
            rv_value = rv.get(item.key)
            la_value = la.get(item.key)
            lines.append(
                f"| {item.name} | {f2(item.baseline)} | {f2(rv_value)} | "
                f"{fs(calc_score(rv_value, item.baseline))} | {f2(la_value)} | {fs(calc_score(la_value, item.baseline))} |"
            )

    return "\n".join(lines) + "\n"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rv", type=Path, nargs="+", default=[DEFAULT_RV])
    parser.add_argument("--la", type=Path, nargs="+", default=[DEFAULT_LA])
    parser.add_argument("--out", type=Path, default=None)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    out = args.out or (DEFAULT_OUT_DIR / f"{datetime.now().strftime('%m%d-%H%M%S')}.md")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(
        build_report(args.rv, args.la, parse_logs(args.rv), parse_logs(args.la)),
        encoding="utf-8",
    )
    print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
