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
    baseline: float

    @property
    def key(self) -> tuple[str, str]:
        return (self.libc, self.test)

    @property
    def name(self) -> str:
        return f"iperf {self.test} (Mbits/sec)"


SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_RV = (SCRIPT_DIR / "../../../docs/output_rv.txt").resolve()
DEFAULT_LA = (SCRIPT_DIR / "../../../docs/output_la.txt").resolve()
DEFAULT_OUT_DIR = SCRIPT_DIR / "results"
LIBCS = ("glibc", "musl")

BASE_ITEMS = [
    ("BASIC_TCP", 796.0),
    ("BASIC_UDP", 393.0),
    ("PARALLEL_TCP", 1310.72),
    ("PARALLEL_UDP", 442.0),
    ("REVERSE_TCP", 792.0),
    ("REVERSE_UDP", 381.0),
]
ITEMS = [Item(libc, test, baseline) for libc in LIBCS for test, baseline in BASE_ITEMS]

ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
START_RE = re.compile(r"#### OS COMP TEST GROUP START iperf-(glibc|musl) ####")
END_RE = re.compile(r"#### OS COMP TEST GROUP END iperf-(glibc|musl) ####")
BEGIN_RE = re.compile(r"====== iperf (BASIC_UDP|BASIC_TCP|PARALLEL_UDP|PARALLEL_TCP|REVERSE_UDP|REVERSE_TCP) begin ======")
RECEIVER_RE = re.compile(r"^\[\s*(?:\d+|SUM)\]\s+.+?\s+([0-9.]+)\s+([MG])bits/sec\b.*\breceiver\s*$")


def infer_libc(path: Path) -> str | None:
    name = path.name.lower()
    if "glibc" in name:
        return "glibc"
    if "musl" in name:
        return "musl"
    return None


def parse_logs(paths: Iterable[Path]) -> dict[tuple[str, str], float]:
    results: dict[tuple[str, str], float] = {}
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

            if m := RECEIVER_RE.match(line):
                value = float(m.group(1))
                if m.group(2) == "G":
                    value *= 1024
                results[(libc, current_test)] = value

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
    rv: dict[tuple[str, str], float],
    la: dict[tuple[str, str], float],
) -> str:
    now = datetime.now().strftime("%m%d-%H%M%S")
    lines = [
        "# iperf 结果",
        "",
        f"- 时间: `{now}`",
        f"- RV: {fmt_paths(rv_paths)}",
        f"- LA: {fmt_paths(la_paths)}",
        "- result 取每个测试段最后一条 `receiver` 行的 `Bitrate`，`Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。",
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
