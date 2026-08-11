"""按 ch7_网络.typ 的三张性能表分别绘制优化前后对比图。

默认输出到脚本所在的 code 目录：
  - iperf3_glibc_before_after.png / .svg
  - iperf3_musl_before_after.png / .svg
  - netperf_glibc_before_after.png / .svg

运行：
    python docs/report_primary/pictrues/code/plot_network_performance.py
"""

from __future__ import annotations

import argparse
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
from matplotlib import font_manager
from matplotlib.ticker import FuncFormatter


SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_OUTPUT_DIR = SCRIPT_DIR

BEFORE = "#AEB7C4"
AFTER = "#246BCE"
ACCENT = "#D95F02"
GRID = "#D8DEE8"
TEXT = "#172033"


# 数据严格取自 docs/report_primary/ch7_网络.typ。
IPERF_LABELS = ["TCP 基本", "TCP 并发", "TCP 反向", "UDP 基本", "UDP 并发", "UDP 反向"]

IPERF_GLIBC_BEFORE = np.array([265, 239, 283, 36.4, 19.6, 38.9], dtype=float)
IPERF_GLIBC_AFTER = np.array([1608, 1935, 1618, 1167, 1260, 1280], dtype=float)

IPERF_MUSL_BEFORE = np.array([245, 207, 278, 34.7, 23.9, 37.4], dtype=float)
IPERF_MUSL_AFTER = np.array([1669, 1997, 1638, 1178, 1249, 1270], dtype=float)

NETPERF_STREAM_BEFORE = np.array([11.01], dtype=float)
NETPERF_STREAM_AFTER = np.array([299.35], dtype=float)
NETPERF_RATE_LABELS = ["UDP_STREAM", "TCP_RR", "UDP_RR", "TCP_CRR"]
NETPERF_RATE_BEFORE = np.array([159.85, 1591.55, 1644.41, 146.30], dtype=float)
NETPERF_RATE_AFTER = np.array([192.11, 2681.80, 2140.90, 274.08], dtype=float)


def configure_font() -> None:
    candidates = [
        Path("C:/Windows/Fonts/msyh.ttc"),
        Path("C:/Windows/Fonts/simhei.ttf"),
        Path("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
        Path("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc"),
    ]
    for path in candidates:
        if path.exists():
            font_manager.fontManager.addfont(str(path))
            plt.rcParams["font.family"] = font_manager.FontProperties(fname=str(path)).get_name()
            break
    plt.rcParams["axes.unicode_minus"] = False


def gain(before: float, after: float) -> float:
    return (after - before) / before * 100.0


def compact(value: float, _position: int | None = None) -> str:
    return f"{value / 1000:g}k" if value >= 1000 else f"{value:g}"


def style(ax: plt.Axes) -> None:
    ax.set_axisbelow(True)
    ax.grid(axis="y", color=GRID, linewidth=0.8)
    ax.spines[["top", "right"]].set_visible(False)
    ax.spines[["left", "bottom"]].set_color("#8792A2")
    ax.tick_params(colors=TEXT, labelsize=10)


def add_grouped_bars(
    ax: plt.Axes,
    labels: list[str],
    before: np.ndarray,
    after: np.ndarray,
    *,
    ylabel: str,
    log_scale: bool = False,
) -> None:
    x = np.arange(len(labels))
    width = 0.36
    old_bars = ax.bar(x - width / 2, before, width, color=BEFORE, label="优化前", zorder=3)
    new_bars = ax.bar(x + width / 2, after, width, color=AFTER, label="优化后", zorder=3)
    ax.set_xticks(x, labels)
    ax.set_ylabel(ylabel, fontsize=11, color=TEXT)
    style(ax)

    if log_scale:
        ax.set_yscale("log")
        ax.set_ylim(max(10, before.min() * 0.55), after.max() * 2.4)
        ax.yaxis.set_major_formatter(FuncFormatter(compact))
    else:
        ax.set_ylim(0, max(before.max(), after.max()) * 1.30)

    for bars, values in ((old_bars, before), (new_bars, after)):
        for bar, value in zip(bars, values):
            y = value * 1.10 if log_scale else value + ax.get_ylim()[1] * 0.012
            ax.text(
                bar.get_x() + bar.get_width() / 2,
                y,
                f"{value:g}",
                ha="center",
                va="bottom",
                fontsize=8,
                color=TEXT,
            )

    for bar, old, new in zip(new_bars, before, after):
        y = new * 1.48 if log_scale else new + ax.get_ylim()[1] * 0.075
        ax.text(
            bar.get_x() + bar.get_width() / 2,
            y,
            f"+{gain(old, new):.1f}%",
            ha="center",
            va="bottom",
            fontsize=9,
            fontweight="bold",
            color=ACCENT,
        )


def save(fig: plt.Figure, output_dir: Path, stem: str, dpi: int) -> tuple[Path, Path]:
    png = output_dir / f"{stem}.png"
    svg = output_dir / f"{stem}.svg"
    titles = [ax.title for ax in fig.axes]
    if fig._suptitle is not None:
        titles.append(fig._suptitle)
    for title in titles:
        title.set_visible(False)
    fig.savefig(png, dpi=dpi, bbox_inches="tight", facecolor="white")
    for title in titles:
        title.set_visible(True)
    fig.savefig(svg, bbox_inches="tight", facecolor="white")
    plt.close(fig)
    return png, svg


def draw_iperf(
    output_dir: Path,
    dpi: int,
    libc: str,
    before: np.ndarray,
    after: np.ndarray,
) -> tuple[Path, Path]:
    fig, ax = plt.subplots(figsize=(13, 7.2))
    fig.patch.set_facecolor("white")
    add_grouped_bars(
        ax,
        IPERF_LABELS,
        before,
        after,
        ylabel="吞吐量（Mbit/s，对数坐标）",
        log_scale=True,
    )
    ax.set_title(
        f"iperf3 性能优化前后对比（{libc}，本地 Loopback）",
        fontsize=18,
        fontweight="bold",
        color=TEXT,
        pad=18,
    )
    ax.legend(loc="upper left", frameon=False, ncol=2, fontsize=11)
    fig.subplots_adjust(left=0.09, right=0.98, top=0.88, bottom=0.10)
    return save(fig, output_dir, f"iperf3_{libc}_before_after", dpi)


def draw_netperf(output_dir: Path, dpi: int) -> tuple[Path, Path]:
    # 同一张原始表包含 Mbit/s 和“传输率/秒”两种单位，因此一张图片内分两个坐标区。
    fig, axes = plt.subplots(1, 2, figsize=(14.5, 7.2), gridspec_kw={"width_ratios": [0.8, 2.2]})
    fig.patch.set_facecolor("white")

    add_grouped_bars(
        axes[0],
        ["TCP_STREAM"],
        NETPERF_STREAM_BEFORE,
        NETPERF_STREAM_AFTER,
        ylabel="吞吐量（Mbit/s）",
    )
    axes[0].set_title("流式吞吐", fontsize=14, fontweight="bold", color=TEXT, pad=12)

    add_grouped_bars(
        axes[1],
        NETPERF_RATE_LABELS,
        NETPERF_RATE_BEFORE,
        NETPERF_RATE_AFTER,
        ylabel="传输率（次/秒）",
    )
    axes[1].set_title("事务与 UDP 流式传输", fontsize=14, fontweight="bold", color=TEXT, pad=12)

    handles, labels = axes[0].get_legend_handles_labels()
    fig.legend(handles, labels, loc="upper center", bbox_to_anchor=(0.5, 0.91), ncol=2, frameon=False, fontsize=11)
    fig.suptitle(
        "netperf 性能优化前后对比（glibc，本地 Loopback）",
        fontsize=19,
        fontweight="bold",
        color=TEXT,
        y=0.985,
    )
    fig.subplots_adjust(left=0.075, right=0.98, top=0.82, bottom=0.09, wspace=0.30)
    return save(fig, output_dir, "netperf_glibc_before_after", dpi)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT_DIR)
    parser.add_argument("--dpi", type=int, default=300)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    configure_font()

    outputs = [
        *draw_iperf(output_dir, args.dpi, "glibc", IPERF_GLIBC_BEFORE, IPERF_GLIBC_AFTER),
        *draw_iperf(output_dir, args.dpi, "musl", IPERF_MUSL_BEFORE, IPERF_MUSL_AFTER),
        *draw_netperf(output_dir, args.dpi),
    ]
    for path in outputs:
        print(path)


if __name__ == "__main__":
    main()
