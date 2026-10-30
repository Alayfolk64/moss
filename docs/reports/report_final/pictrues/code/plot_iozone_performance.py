"""按 ch5_文件系统.typ 的两张 iozone 表分别绘制优化前后对比图。

默认在脚本所在的 code 目录输出：
  - iozone_glibc_before_after.png / .svg
  - iozone_musl_before_after.png / .svg

运行：
    python docs/report_primary/pictrues/code/plot_iozone_performance.py
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

BEFORE_COLOR = "#AEB7C4"
AFTER_COLOR = "#246BCE"
ACCENT_COLOR = "#D95F02"
GRID_COLOR = "#D8DEE8"
TEXT_COLOR = "#172033"


# 数据严格取自 docs/report_primary/ch5_文件系统.typ 第 5.8 节。
LABELS = [
    "顺序写",
    "顺序读",
    "顺序重写",
    "随机读",
    "随机写",
    "反向读",
    "pread\n并发读",
    "pwrite\n并发写",
]

GLIBC_BEFORE = np.array([3524, 13136, 7471, 11082, 7141, 10743, 14184, 3724], dtype=float)
GLIBC_AFTER = np.array([33221, 81470, 81639, 57668, 59978, 58824, 108348, 28263], dtype=float)

MUSL_BEFORE = np.array([3524, 13136, 7471, 11082, 7141, 10743, 14184, 3724], dtype=float)
MUSL_AFTER = np.array([26407, 62733, 62254, 43813, 51344, 54217, 88315, 28192], dtype=float)


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


def thousands(value: float, _position: int | None = None) -> str:
    if value == 0:
        return "0"
    return f"{value / 1000:.0f}k"


def draw_one(
    output_dir: Path,
    dpi: int,
    libc: str,
    before: np.ndarray,
    after: np.ndarray,
) -> tuple[Path, Path]:
    x = np.arange(len(LABELS))
    width = 0.36

    fig, ax = plt.subplots(figsize=(15, 8))
    fig.patch.set_facecolor("white")

    old_bars = ax.bar(
        x - width / 2,
        before,
        width,
        color=BEFORE_COLOR,
        edgecolor="white",
        linewidth=0.6,
        label="优化前",
        zorder=3,
    )
    new_bars = ax.bar(
        x + width / 2,
        after,
        width,
        color=AFTER_COLOR,
        edgecolor="white",
        linewidth=0.6,
        label="优化后",
        zorder=3,
    )

    y_max = float(after.max()) * 1.28
    ax.set_ylim(0, y_max)
    ax.set_xticks(x, LABELS)
    ax.set_ylabel("吞吐量（kB/s）", fontsize=12, color=TEXT_COLOR)
    ax.set_title(
        f"iozone 性能优化前后对比（{libc}，本地文件系统）",
        fontsize=19,
        fontweight="bold",
        color=TEXT_COLOR,
        pad=18,
    )
    ax.legend(loc="upper left", frameon=False, ncol=2, fontsize=11)
    ax.yaxis.set_major_formatter(FuncFormatter(thousands))
    ax.set_axisbelow(True)
    ax.grid(axis="y", color=GRID_COLOR, linewidth=0.8)
    ax.spines[["top", "right"]].set_visible(False)
    ax.spines[["left", "bottom"]].set_color("#8792A2")
    ax.tick_params(colors=TEXT_COLOR, labelsize=10)

    for bars, values in ((old_bars, before), (new_bars, after)):
        for bar, value in zip(bars, values):
            ax.text(
                bar.get_x() + bar.get_width() / 2,
                value + y_max * 0.012,
                f"{value:,.0f}",
                ha="center",
                va="bottom",
                fontsize=8,
                color=TEXT_COLOR,
                rotation=0,
            )

    for bar, old, new in zip(new_bars, before, after):
        ax.text(
            bar.get_x() + bar.get_width() / 2,
            new + y_max * 0.070,
            f"+{gain(old, new):.1f}%",
            ha="center",
            va="bottom",
            fontsize=9,
            fontweight="bold",
            color=ACCENT_COLOR,
        )

    fig.subplots_adjust(left=0.075, right=0.985, top=0.89, bottom=0.10)

    stem = f"iozone_{libc}_before_after"
    png = output_dir / f"{stem}.png"
    svg = output_dir / f"{stem}.svg"
    ax.title.set_visible(False)
    fig.savefig(png, dpi=dpi, bbox_inches="tight", facecolor="white")
    ax.title.set_visible(True)
    fig.savefig(svg, bbox_inches="tight", facecolor="white")
    plt.close(fig)
    return png, svg


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
        *draw_one(output_dir, args.dpi, "glibc", GLIBC_BEFORE, GLIBC_AFTER),
        *draw_one(output_dir, args.dpi, "musl", MUSL_BEFORE, MUSL_AFTER),
    ]
    for path in outputs:
        print(path)


if __name__ == "__main__":
    main()
