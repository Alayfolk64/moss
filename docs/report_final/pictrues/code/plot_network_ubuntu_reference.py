"""绘制网络性能相对 Ubuntu=100% 的优化前后对比图。

这是独立版本，不覆盖 plot_network_performance.py 及其产物。
数据取自 docs/report_primary/ch7_网络.typ 的三张性能表。
"""

from __future__ import annotations

import argparse
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
from matplotlib import font_manager
from matplotlib.ticker import PercentFormatter


SCRIPT_DIR = Path(__file__).resolve().parent

IPERF_LABELS = ["TCP 基本", "TCP 并发", "TCP 反向", "UDP 基本", "UDP 并发", "UDP 反向"]
IPERF_GLIBC_BEFORE = np.array([265, 239, 283, 36.4, 19.6, 38.9], dtype=float)
IPERF_GLIBC_AFTER = np.array([1608, 1935, 1618, 1167, 1260, 1280], dtype=float)
IPERF_GLIBC_UBUNTU = np.array([3041, 3574, 3277, 2908, 3840, 2744], dtype=float)
IPERF_MUSL_BEFORE = np.array([245, 207, 278, 34.7, 23.9, 37.4], dtype=float)
IPERF_MUSL_AFTER = np.array([1669, 1997, 1638, 1178, 1249, 1270], dtype=float)
IPERF_MUSL_UBUNTU = np.array([3267, 2755, 3379, 2324, 3809, 2427], dtype=float)

NETPERF_LABELS = ["TCP_STREAM", "UDP_STREAM", "TCP_RR", "UDP_RR", "TCP_CRR"]
NETPERF_BEFORE = np.array([11.01, 159.85, 1591.55, 1644.41, 146.30], dtype=float)
NETPERF_AFTER = np.array([299.35, 192.11, 2681.80, 2140.90, 274.08], dtype=float)
NETPERF_UBUNTU = np.array([277.37, 127.63, 2544.46, 2807.66, 753.23], dtype=float)

BEFORE_COLOR = "#FF705D"
AFTER_COLOR = "#2864E8"
UBUNTU_COLOR = "#08A878"
GRID_COLOR = "#D8DEE8"
TEXT_COLOR = "#283548"


def configure_font() -> None:
    candidates = [
        Path("C:/Windows/Fonts/msyh.ttc"),
        Path("C:/Windows/Fonts/simhei.ttf"),
        Path("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
    ]
    for path in candidates:
        if path.exists():
            font_manager.fontManager.addfont(str(path))
            plt.rcParams["font.family"] = font_manager.FontProperties(fname=str(path)).get_name()
            break
    plt.rcParams["axes.unicode_minus"] = False


def draw_one(
    output_dir: Path,
    dpi: int,
    stem: str,
    title: str,
    labels: list[str],
    before: np.ndarray,
    after: np.ndarray,
    ubuntu: np.ndarray,
) -> tuple[Path, Path]:
    before_pct = before / ubuntu * 100.0
    after_pct = after / ubuntu * 100.0
    x = np.arange(len(labels))
    width = 0.30

    fig, ax = plt.subplots(figsize=(14, 6.8))
    fig.patch.set_facecolor("white")
    ax.bar(x - width / 2, before_pct, width, color=BEFORE_COLOR, label="优化前", zorder=3)
    ax.bar(x + width / 2, after_pct, width, color=AFTER_COLOR, label="优化后", zorder=3)

    upper = max(110.0, float(after_pct.max()) * 1.12)
    ax.set_ylim(0, upper)
    ax.axhline(100, color=UBUNTU_COLOR, linewidth=3, zorder=4)
    ax.text(
        len(labels) - 0.35,
        102.2,
        "Ubuntu 基准 = 100%",
        ha="right",
        va="bottom",
        color=UBUNTU_COLOR,
        fontsize=11,
        fontweight="bold",
    )
    ax.set_xticks(x, labels)
    ax.set_ylabel("相对 Ubuntu 性能（%）", fontsize=12, color=TEXT_COLOR)
    ax.set_title(title, fontsize=19, fontweight="bold", color=TEXT_COLOR, pad=18)
    ax.yaxis.set_major_formatter(PercentFormatter(xmax=100, decimals=0))
    ax.set_axisbelow(True)
    ax.grid(axis="y", color=GRID_COLOR, linewidth=0.8)
    ax.spines[["top", "right"]].set_visible(False)
    ax.spines[["left", "bottom"]].set_color("#8792A2")
    ax.tick_params(colors=TEXT_COLOR, labelsize=10)
    ax.legend(loc="upper center", bbox_to_anchor=(0.5, 1.01), ncol=2, frameon=False, fontsize=11)
    fig.subplots_adjust(left=0.075, right=0.985, top=0.86, bottom=0.11)

    png = output_dir / f"{stem}.png"
    svg = output_dir / f"{stem}.svg"
    ax.title.set_visible(False)
    fig.savefig(png, dpi=dpi, bbox_inches="tight", facecolor="white")
    ax.title.set_visible(True)
    fig.savefig(svg, bbox_inches="tight", facecolor="white")
    plt.close(fig)
    return png, svg


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=SCRIPT_DIR)
    parser.add_argument("--dpi", type=int, default=300)
    args = parser.parse_args()
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    configure_font()
    outputs = [
        *draw_one(
            output_dir,
            args.dpi,
            "iperf3_glibc_ubuntu_reference",
            "iperf3 优化前后对比（glibc，Ubuntu=100%）",
            IPERF_LABELS,
            IPERF_GLIBC_BEFORE,
            IPERF_GLIBC_AFTER,
            IPERF_GLIBC_UBUNTU,
        ),
        *draw_one(
            output_dir,
            args.dpi,
            "iperf3_musl_ubuntu_reference",
            "iperf3 优化前后对比（musl，Ubuntu=100%）",
            IPERF_LABELS,
            IPERF_MUSL_BEFORE,
            IPERF_MUSL_AFTER,
            IPERF_MUSL_UBUNTU,
        ),
        *draw_one(
            output_dir,
            args.dpi,
            "netperf_glibc_ubuntu_reference",
            "netperf 优化前后对比（glibc，Ubuntu=100%）",
            NETPERF_LABELS,
            NETPERF_BEFORE,
            NETPERF_AFTER,
            NETPERF_UBUNTU,
        ),
    ]
    for path in outputs:
        print(path)


if __name__ == "__main__":
    main()
