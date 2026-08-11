"""绘制 iozone 相对 Ubuntu=100% 的优化前后对比图。

这是独立版本，不覆盖 plot_iozone_performance.py 及其产物。
数据取自 docs/report_primary/ch5_文件系统.typ 第 5.8 节。
"""

from __future__ import annotations

import argparse
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
from matplotlib import font_manager
from matplotlib.ticker import PercentFormatter


SCRIPT_DIR = Path(__file__).resolve().parent
LABELS = ["顺序写", "顺序读", "顺序重写", "随机读", "随机写", "反向读", "pread", "pwrite"]

BEFORE = np.array([3524, 13136, 7471, 11082, 7141, 10743, 14184, 3724], dtype=float)
GLIBC_AFTER = np.array([33221, 81470, 81639, 57668, 59978, 58824, 108348, 28263], dtype=float)
GLIBC_UBUNTU = np.array([51922, 104754, 72760, 57175, 59817, 67361, 125270, 39335], dtype=float)
MUSL_AFTER = np.array([26407, 62733, 62254, 43813, 51344, 54217, 88315, 28192], dtype=float)
MUSL_UBUNTU = np.array([44818, 98518, 69316, 91905, 57648, 78861, 141988, 49303], dtype=float)

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
    libc: str,
    after: np.ndarray,
    ubuntu: np.ndarray,
) -> tuple[Path, Path]:
    before_pct = BEFORE / ubuntu * 100.0
    after_pct = after / ubuntu * 100.0
    x = np.arange(len(LABELS))
    width = 0.30

    fig, ax = plt.subplots(figsize=(15, 6.8))
    fig.patch.set_facecolor("white")
    ax.bar(x - width / 2, before_pct, width, color=BEFORE_COLOR, label="优化前", zorder=3)
    ax.bar(x + width / 2, after_pct, width, color=AFTER_COLOR, label="优化后", zorder=3)

    upper = max(110.0, float(after_pct.max()) * 1.12)
    ax.set_ylim(0, upper)
    ax.axhline(100, color=UBUNTU_COLOR, linewidth=3, zorder=4)
    ax.text(
        len(LABELS) - 0.42,
        102.2,
        "Ubuntu 基准 = 100%",
        ha="right",
        va="bottom",
        color=UBUNTU_COLOR,
        fontsize=11,
        fontweight="bold",
    )

    ax.set_xticks(x, LABELS)
    ax.set_ylabel("相对 Ubuntu 吞吐（%）", fontsize=12, color=TEXT_COLOR)
    ax.set_title(
        f"iozone 优化前后对比（{libc}，Ubuntu=100%）",
        fontsize=19,
        fontweight="bold",
        color=TEXT_COLOR,
        pad=18,
    )
    ax.yaxis.set_major_formatter(PercentFormatter(xmax=100, decimals=0))
    ax.set_axisbelow(True)
    ax.grid(axis="y", color=GRID_COLOR, linewidth=0.8)
    ax.spines[["top", "right"]].set_visible(False)
    ax.spines[["left", "bottom"]].set_color("#8792A2")
    ax.tick_params(colors=TEXT_COLOR, labelsize=10)
    ax.legend(loc="upper center", bbox_to_anchor=(0.5, 1.01), ncol=2, frameon=False, fontsize=11)
    fig.subplots_adjust(left=0.075, right=0.985, top=0.86, bottom=0.11)

    stem = f"iozone_{libc}_ubuntu_reference"
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
        *draw_one(output_dir, args.dpi, "glibc", GLIBC_AFTER, GLIBC_UBUNTU),
        *draw_one(output_dir, args.dpi, "musl", MUSL_AFTER, MUSL_UBUNTU),
    ]
    for path in outputs:
        print(path)


if __name__ == "__main__":
    main()
