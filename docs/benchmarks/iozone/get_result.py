#!/usr/bin/env python3
from __future__ import annotations

import argparse
import dataclasses
import re
from datetime import datetime
from pathlib import Path


@dataclasses.dataclass(frozen=True)
class Item:
    libc: str
    group: str
    op: str
    baseline: float

    @property
    def key(self) -> tuple[str, str, str]:
        return (self.libc, self.group, self.op)

    @property
    def name(self) -> str:
        return f"iozone {self.group} 4 {self.op} (kB/sec)"


@dataclasses.dataclass
class Result:
    children: float | None = None
    parent: float | None = None
    max: float | None = None


SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_RV = (SCRIPT_DIR / "../../../docs/output_rv.txt").resolve()
DEFAULT_LA = (SCRIPT_DIR / "../../../docs/output_la.txt").resolve()
DEFAULT_OUT_DIR = SCRIPT_DIR / "results"
LIBCS = ("glibc", "musl")

BASE_ITEMS = [
    ("fwrite/fread", "freaders", 7123.18),
    ("fwrite/fread", "fwriters", 3204.25),
    ("pwrite/pread", "pread readers", 14183.61),
    ("pwrite/pread", "pwrite writers", 3724.00),
    ("pwritev/preadv", "initial writers", 3609.25),
    ("pwritev/preadv", "rewriters", 8305.90),
    ("random-read", "initial writers", 3401.72),
    ("random-read", "random readers", 11082.03),
    ("random-read", "random writers", 7140.81),
    ("random-read", "rewriters", 7196.32),
    ("read-backwards", "initial writers", 3331.39),
    ("read-backwards", "reverse readers", 10743.42),
    ("read-backwards", "rewriters", 4906.05),
    ("stride-read", "initial writers", 3468.09),
    ("stride-read", "rewriters", 7004.64),
    ("stride-read", "stride readers", 11848.11),
    ("write/read", "initial writers", 3524.04),
    ("write/read", "re-readers", 13064.75),
    ("write/read", "readers", 13135.64),
    ("write/read", "rewriters", 7471.07),
]
ITEMS = [Item(libc, group, op, baseline) for libc in LIBCS for group, op, baseline in BASE_ITEMS]

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
START_RE = re.compile(r"#### OS COMP TEST GROUP START iozone-(glibc|musl) ####")
END_RE = re.compile(r"#### OS COMP TEST GROUP END iozone-(glibc|musl) ####")
GROUP_RE = re.compile(r"^iozone\s+through(?:t)?put\s+(.+?)\s+measurements", re.IGNORECASE)
CHILDREN_RE = re.compile(r"Children see throughput for\s+\d+\s+(.+?)\s*=\s*([0-9.]+)\s+kB/sec", re.IGNORECASE)
PARENT_RE = re.compile(r"Parent sees throughput for\s+\d+\s+(.+?)\s*=\s*([0-9.]+)\s+kB/sec", re.IGNORECASE)
MAX_RE = re.compile(r"Max throughput per process\s*=\s*([0-9.]+)\s+kB/sec", re.IGNORECASE)


def norm(s: str) -> str:
    return " ".join(s.strip().lower().split())


def parse_log(path: Path) -> dict[tuple[str, str, str], Result]:
    if not path.exists():
        return {}
    text = ANSI_RE.sub("", path.read_text(encoding="utf-8", errors="replace"))
    libc: str | None = None
    group: str | None = None
    current_key: tuple[str, str, str] | None = None
    current: Result | None = None
    results: dict[tuple[str, str, str], Result] = {}

    for raw in text.splitlines():
        line = raw.strip()

        if m := START_RE.search(line):
            libc = m.group(1)
            group = None
            current_key = None
            current = None
            continue

        if m := END_RE.search(line):
            if libc == m.group(1):
                libc = None
                group = None
                current_key = None
                current = None
            continue

        if libc is None:
            continue

        if m := GROUP_RE.match(line):
            group = norm(m.group(1))
            current_key = None
            current = None
            continue

        if group is None:
            continue

        if m := CHILDREN_RE.search(line):
            op = norm(m.group(1))
            current_key = (libc, group, op)
            current = Result(children=float(m.group(2)))
            continue

        if current is not None and (m := PARENT_RE.search(line)):
            current.parent = float(m.group(2))
            continue

        if current is not None and current_key is not None and (m := MAX_RE.search(line)):
            current.max = float(m.group(1))
            results[current_key] = current
            current_key = None
            current = None

    return results


def calc_score(value: float | None, baseline: float) -> float | None:
    if value is None:
        return None
    if value <= baseline:
        return 1.0
    return 2 - baseline / value


def f2(value: float | None) -> str:
    return "-" if value is None else f"{value:.2f}"


def fs(value: float | None) -> str:
    return "-" if value is None else f"{value:.6f}"


def build_report(rv_path: Path, la_path: Path, rv: dict[tuple[str, str, str], Result], la: dict[tuple[str, str, str], Result]) -> str:
    now = datetime.now().strftime("%m%d-%H%M%S")
    lines = [
        "# iozone 结果",
        "",
        f"- 时间: `{now}`",
        f"- RV: `{rv_path}`",
        f"- LA: `{la_path}`",
        "- result 取日志里的 `Max throughput per process`。",
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
            rv_score = calc_score(rv.get(item.key).max if rv.get(item.key) else None, item.baseline)
            la_score = calc_score(la.get(item.key).max if la.get(item.key) else None, item.baseline)
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
        lines.extend(["", f"## {libc}", "", "| 项目 | baseline | RV result | RV score | LA result | LA score |", "|---|---:|---:|---:|---:|---:|"])
        for item in [i for i in ITEMS if i.libc == libc]:
            rv_value = rv.get(item.key).max if rv.get(item.key) else None
            la_value = la.get(item.key).max if la.get(item.key) else None
            lines.append(
                f"| {item.name} | {f2(item.baseline)} | {f2(rv_value)} | "
                f"{fs(calc_score(rv_value, item.baseline))} | {f2(la_value)} | {fs(calc_score(la_value, item.baseline))} |"
            )

    return "\n".join(lines) + "\n"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rv", type=Path, default=DEFAULT_RV)
    parser.add_argument("--la", type=Path, default=DEFAULT_LA)
    parser.add_argument("--out", type=Path, default=None)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    out = args.out or (DEFAULT_OUT_DIR / f"{datetime.now().strftime('%m%d-%H%M%S')}.md")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(build_report(args.rv, args.la, parse_log(args.rv), parse_log(args.la)), encoding="utf-8")
    print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
