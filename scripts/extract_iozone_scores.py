#!/usr/bin/env python3
"""Extract iozone-musl throughput scores from StarryOS benchmark logs."""

from __future__ import annotations

import argparse
import dataclasses
import re
from pathlib import Path


@dataclasses.dataclass(frozen=True)
class PdfRow:
    group: str
    operation: str
    baseline: float
    pdf_rv_result: float
    pdf_rv_score: float
    pdf_la_result: float
    pdf_la_score: float

    @property
    def key(self) -> tuple[str, str]:
        return (self.group, self.operation)

    @property
    def label(self) -> str:
        return f"iozone {self.group} 4 {self.operation} (kB/sec)"


@dataclasses.dataclass
class LogRow:
    group: str
    operation: str
    children: float | None = None
    parent: float | None = None
    max_throughput: float | None = None
    command: str | None = None
    line: int | None = None

    @property
    def key(self) -> tuple[str, str]:
        return (self.group, self.operation)


# Extracted from /Users/wuxun/评测结果/6.8/开放课程.pdf, section iozone-musl.
# The PDF table's result column matches each log row's "Max throughput per process".
PDF_ROWS = [
    PdfRow("fwrite/fread", "freaders", 7123.18, 4249.63, 1.0, 4980.06, 1.0),
    PdfRow("fwrite/fread", "fwriters", 3204.25, 1670.99, 1.0, 1624.69, 1.0),
    PdfRow("pwrite/pread", "pread readers", 14183.61, 5701.54, 1.0, 12041.21, 1.0),
    PdfRow("pwrite/pread", "pwrite writers", 3724.00, 1622.73, 1.0, 2370.23, 1.0),
    PdfRow("pwritev/preadv", "initial writers", 3609.25, 1642.04, 1.0, 2373.76, 1.0),
    PdfRow("pwritev/preadv", "rewriters", 8305.90, 7165.79, 1.0, 9056.25, 1.0828543823326432),
    PdfRow("random-read", "initial writers", 3401.72, 1597.25, 1.0, 1689.47, 1.0),
    PdfRow("random-read", "random readers", 11082.03, 2606.44, 1.0, 5540.23, 1.0),
    PdfRow("random-read", "random writers", 7140.81, 4249.78, 1.0, 4690.71, 1.0),
    PdfRow("random-read", "rewriters", 7196.32, 6677.12, 1.0, 8423.92, 1.1457278796569772),
    PdfRow("read-backwards", "initial writers", 3331.39, 1603.31, 1.0, 1696.69, 1.0),
    PdfRow("read-backwards", "reverse readers", 10743.42, 3136.80, 1.0, 4849.46, 1.0),
    PdfRow("read-backwards", "rewriters", 4906.05, 8425.48, 1.4177127000479497, 6935.31, 1.2925983121158247),
    PdfRow("stride-read", "initial writers", 3468.09, 1629.52, 1.0, 2288.30, 1.0),
    PdfRow("stride-read", "rewriters", 7004.64, 7093.90, 1.0125826414243222, 7758.43, 1.0971575434720684),
    PdfRow("stride-read", "stride readers", 11848.11, 3117.69, 1.0, 6098.85, 1.0),
    PdfRow("write/read", "initial writers", 3524.04, 1652.22, 1.0, 1638.70, 1.0),
    PdfRow("write/read", "re-readers", 13064.75, 4428.16, 1.0, 8730.79, 1.0),
    PdfRow("write/read", "readers", 13135.64, 3525.47, 1.0, 7237.07, 1.0),
    PdfRow("write/read", "rewriters", 7471.07, 4618.73, 1.0, 6873.59, 1.0),
]

ANSI_ESCAPE_RE = re.compile(r"\x1b\[[0-9;]*m")
GROUP_START_RE = re.compile(r"#### OS COMP TEST GROUP START iozone-musl ####")
GROUP_END_RE = re.compile(r"#### OS COMP TEST GROUP END iozone-musl ####")
MEASURE_RE = re.compile(r"^iozone\s+through(?:t)?put\s+(.+?)\s+measurements", re.IGNORECASE)
COMMAND_RE = re.compile(r"Command line used:\s+(.+)")
CHILDREN_RE = re.compile(
    r"Children see throughput for\s+(\d+)\s+(.+?)\s*=\s*([0-9]+(?:\.[0-9]+)?)\s+kB/sec",
    re.IGNORECASE,
)
PARENT_RE = re.compile(
    r"Parent sees throughput for\s+(\d+)\s+(.+?)\s*=\s*([0-9]+(?:\.[0-9]+)?)\s+kB/sec",
    re.IGNORECASE,
)
MAX_RE = re.compile(
    r"Max throughput per process\s*=\s*([0-9]+(?:\.[0-9]+)?)\s+kB/sec",
    re.IGNORECASE,
)


def normalize_name(value: str) -> str:
    return " ".join(value.strip().lower().split())


def parse_log(path: Path) -> dict[tuple[str, str], LogRow]:
    text = ANSI_ESCAPE_RE.sub("", path.read_text(encoding="utf-8", errors="replace"))
    in_iozone = False
    group: str | None = None
    command: str | None = None
    current: LogRow | None = None
    rows: dict[tuple[str, str], LogRow] = {}

    for line_no, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.strip()
        if GROUP_START_RE.search(line):
            in_iozone = True
            group = None
            command = None
            current = None
            continue
        if GROUP_END_RE.search(line):
            break
        if not in_iozone:
            continue

        measure_match = MEASURE_RE.match(line)
        if measure_match:
            group = normalize_name(measure_match.group(1))
            command = None
            current = None
            continue

        command_match = COMMAND_RE.search(line)
        if command_match and group:
            command = command_match.group(1).strip()
            continue

        children_match = CHILDREN_RE.search(line)
        if children_match and group:
            operation = normalize_name(children_match.group(2))
            current = LogRow(
                group=group,
                operation=operation,
                children=float(children_match.group(3)),
                command=command,
                line=line_no,
            )
            continue

        parent_match = PARENT_RE.search(line)
        if parent_match and current:
            current.parent = float(parent_match.group(3))
            continue

        max_match = MAX_RE.search(line)
        if max_match and current:
            current.max_throughput = float(max_match.group(1))
            previous = rows.get(current.key)
            if (
                previous is None
                or previous.max_throughput is None
                or current.max_throughput > previous.max_throughput
            ):
                rows[current.key] = current
            current = None

    return rows


def score(result: float, baseline: float) -> float:
    if result <= baseline:
        return 1.0
    return 2.0 - baseline / result


def fmt_float(value: float | None, digits: int = 2) -> str:
    if value is None:
        return "-"
    return f"{value:.{digits}f}"


def fmt_score(value: float | None) -> str:
    if value is None:
        return "-"
    return f"{value:.6f}"


def get_required(rows: dict[tuple[str, str], LogRow], pdf_row: PdfRow, arch: str) -> LogRow:
    try:
        return rows[pdf_row.key]
    except KeyError as exc:
        key = f"{pdf_row.group} / {pdf_row.operation}"
        raise SystemExit(f"missing {arch} iozone row: {key}") from exc


def build_markdown(rv_path: Path, la_path: Path, rv_rows: dict[tuple[str, str], LogRow], la_rows: dict[tuple[str, str], LogRow]) -> str:
    lines: list[str] = []
    pdf_rv_total = sum(row.pdf_rv_score for row in PDF_ROWS)
    pdf_la_total = sum(row.pdf_la_score for row in PDF_ROWS)
    local_rv_total = 0.0
    local_la_total = 0.0

    lines.append("# IOZONE musl 本地得分对照")
    lines.append("")
    lines.append(f"- RV 日志: `{rv_path}`")
    lines.append(f"- LA 日志: `{la_path}`")
    lines.append("- PDF 的 `result` 列对应日志里的 `Max throughput per process`，不是 `Children see throughput` 或 `Parent sees throughput`。")
    lines.append("- 只统计 `iozone-musl` 中 `./iozone -t 4 ... -r 1k -s 1m` 的 20 个吞吐项；`./iozone -a -r 1k -s 4m` 自动测试块不参与评分。")
    lines.append("- 得分公式: `result <= baseline` 时为 `1.0`，否则为 `2 - baseline / result`。")
    lines.append("")
    lines.append("| 项目 | baseline kB/s | PDF RV result | 本地 RV Max | 本地 RV score | PDF LA result | 本地 LA Max | 本地 LA score |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|---:|")

    for pdf_row in PDF_ROWS:
        rv = get_required(rv_rows, pdf_row, "RV")
        la = get_required(la_rows, pdf_row, "LA")
        rv_score = score(rv.max_throughput or 0.0, pdf_row.baseline)
        la_score = score(la.max_throughput or 0.0, pdf_row.baseline)
        local_rv_total += rv_score
        local_la_total += la_score
        lines.append(
            "| "
            + " | ".join(
                [
                    pdf_row.label,
                    fmt_float(pdf_row.baseline),
                    fmt_float(pdf_row.pdf_rv_result),
                    fmt_float(rv.max_throughput),
                    fmt_score(rv_score),
                    fmt_float(pdf_row.pdf_la_result),
                    fmt_float(la.max_throughput),
                    fmt_score(la_score),
                ]
            )
            + " |"
        )

    lines.append("")
    lines.append("## 总分")
    lines.append("")
    lines.append("| 对象 | 总分 |")
    lines.append("|---|---:|")
    lines.append(f"| PDF RV | {fmt_score(pdf_rv_total)} |")
    lines.append(f"| 本地 RV | {fmt_score(local_rv_total)} |")
    lines.append(f"| PDF LA | {fmt_score(pdf_la_total)} |")
    lines.append(f"| 本地 LA | {fmt_score(local_la_total)} |")
    lines.append("")
    lines.append("## 本地日志原始字段")
    lines.append("")
    lines.append("| 项目 | RV Children | RV Parent | RV Max | LA Children | LA Parent | LA Max |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|")

    for pdf_row in PDF_ROWS:
        rv = get_required(rv_rows, pdf_row, "RV")
        la = get_required(la_rows, pdf_row, "LA")
        lines.append(
            "| "
            + " | ".join(
                [
                    pdf_row.label,
                    fmt_float(rv.children),
                    fmt_float(rv.parent),
                    fmt_float(rv.max_throughput),
                    fmt_float(la.children),
                    fmt_float(la.parent),
                    fmt_float(la.max_throughput),
                ]
            )
            + " |"
        )

    return "\n".join(lines) + "\n"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate an iozone-musl score report from output_rv.txt/output_la.txt.",
    )
    parser.add_argument("--rv", default="output_rv.txt", type=Path, help="RISC-V benchmark log")
    parser.add_argument("--la", default="output_la.txt", type=Path, help="LoongArch benchmark log")
    parser.add_argument("--out", default="iozone_scores.md", type=Path, help="Markdown report path")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    rv_rows = parse_log(args.rv)
    la_rows = parse_log(args.la)

    report = build_markdown(args.rv, args.la, rv_rows, la_rows)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(report, encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
