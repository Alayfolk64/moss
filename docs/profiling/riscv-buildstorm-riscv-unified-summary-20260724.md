# RISC-V BuildStorm 2026-07 系列（仅 RISCV）统计汇总报告

本报告只汇总本次你要求的 RISCV 路线：

- 2026-07-21 基线完整 profiling（`riscv-buildstorm-guest-20260721`）
- 2026-07-24 两次完整 profiling（`riscv-buildstorm-guest-20260724`、`riscv-buildstorm-mprotect-20260724`）
- 2026-07-24 `mprotect` per-VMA 结果（`riscv-buildstorm-mprotect-per-vma-20260724`）
- 失败样本 `riscv-buildstorm-guest-recheck-20260724`（用于排障，但不计入主对比）
- Linux 对照（`riscv-linux-buildstorm-20260721`）

## 1. 口径确认

- guest 侧阶段统一口径：`setup`、`prebuild`（完整 446-crate tg-xtask）、`timed`（正式 buildstorm 及收尾）
- guest 采样：CPU 10Hz，off-cpu 1/8，ext4 1/16，page-cache 1/32（与 2026-07-24 报告口径一致）
- 本次汇总使用 `summary.json` 与 `run.json`（有的文件）作为机器可读来源，`mprotect` 事件拆分与子项也来自该口径
- 2026-07-24 之后每次均使用 `-snapshot`、8 vCPU、8 GiB、`run_rv.sh` 风格的命令

## 2. 样本列表与结果表（RISCV）

> 单位：秒。`host_elapsed` 为宿主侧墙钟（无的样本标 `—`）。

| 样本 | setup | prebuild（guest） | timed（guest） | host_elapsed | 状态 | 备注 |
|---|---:|---:|---:|---:|---|---|
| 2026-07-21 基线 (`riscv-buildstorm-guest-20260721`) | 29.86 | 4,588.19 | 2,524.89 | — | complete | 旧版完整 profiling，基线 |
| 2026-07-24 Full (`riscv-buildstorm-guest-20260724`) | 5.83 | 2,501.09 | 1,249.33 | 3,766.38 | complete | 上一轮缺页优化后第一次 full |
| 2026-07-24 mprotect (`riscv-buildstorm-mprotect-20260724`) | 5.76 | 2,363.83 | 1,321.04 | 3,706.29 | complete | 聚焦 mprotect 子项可观测 |
| 2026-07-24 mprotect-per-VMA (`riscv-buildstorm-mprotect-per-vma-20260724`) | 6.13 | 1,318.94 | 637.58 | 1,981.23 | complete | 你要求的第 1-5 点实现后 |
| 2026-07-24 recheck（失败样本） | 6.04 | 0.00 | 0.00 | 2,896.09 | stalled | no progress 1080s 且 15s 窗口 <20% CPU |

### 与基线加速比（对比 2026-07-21）

| 目标样本 | prebuild 加速 | timed 加速 |
|---|---:|---:|
| 2026-07-24 full | **1.83×** | **2.02×** |
| 2026-07-24 mprotect | **1.94×** | **1.91×** |
| 2026-07-24 per-VMA | **3.48×** | **3.96×** |

### 与 Linux 对照

| 对象 | setup | prebuild | timed |
|---|---:|---:|---:|
| Linux RISCV (`riscv-linux-buildstorm-20260721`) | 5.50 | 919.05 | 454.22 |
| Moss 2026-07-21 基线 | 29.86 | 4,588.19 | 2,524.89 |
| Moss 2026-07-24 Full | 5.83 | 2,501.09 | 1,249.33 |
| Moss 2026-07-24 mprotect | 5.76 | 2,363.83 | 1,321.04 |
| Moss 2026-07-24 per-VMA | 6.13 | 1,318.94 | 637.58 |

> Moss 对 Linux 的 timed 相对差：
> - Full：**2.75×** 慢
> - mprotect：**2.91×** 慢
> - per-VMA：**1.40×** 慢

## 3. 关键热点（`event_totals_us`，转成 task-s）

### 3.1 2026-07-21 基线

| 事件 | prebuild(task-s) | timed(task-s) |
|---|---:|---:|
| `mutex-wait` | 11,166.93 | 8,172.23 |
| `off-cpu-all` | 214,663.88 | 149,651.89 |
| `page-cache-inclusive` | 15,042.42 | 9,343.23 |
| `ext4-inclusive` | 2,222.89 | 682.54 |
| `ext4-global-lock-wait` | 981.46 | 218.92 |

### 3.2 2026-07-24 Full

| 事件 | prebuild(task-s) | timed(task-s) |
|---|---:|---:|
| `mutex-wait` | 7,958.51 | 2,819.89 |
| `off-cpu-all` | 146,180.29 | 62,340.97 |
| `page-cache-inclusive` | 116.42 | 37.52 |
| `ext4-inclusive` | 24.31 | 42.18 |
| `ext4-global-lock-wait` | 2.90 | 0.69 |
| `folio-load-wait` | 2.46 | 0.17 |

### 3.3 2026-07-24 mprotect

| 事件 | prebuild(task-s) | timed(task-s) |
|---|---:|---:|
| `mutex-wait` | 7,021.33 | 3,317.64 |
| `off-cpu-all` | 158,096.45 | 83,090.27 |
| `page-cache-inclusive` | 104.08 | 38.69 |
| `ext4-inclusive` | 21.23 | 47.89 |
| `ext4-global-lock-wait` | 1.92 | 0.68 |
| `mprotect-total` | 8,931.02 | 4,312.28 |
| `mprotect-seal-lock-wait` | 3,565.99 | 1,755.63 |
| `mprotect-record-lock-wait` | 3,025.80 | 1,409.25 |
| `mprotect-record-update` | 1,365.60 | 639.57 |
| `mprotect-aspace-lock-wait` | 97.89 | 55.65 |
| `mprotect-page-table` | 418.04 | 224.80 |

### 3.4 2026-07-24 mprotect-per-VMA

| 事件 | prebuild(task-s) | timed(task-s) |
|---|---:|---:|
| `mutex-wait` | 6.68 | 1.42 |
| `off-cpu-all` | 61,676.72 | 30,487.31 |
| `page-cache-inclusive` | 203.78 | 45.53 |
| `ext4-inclusive` | 38.11 | 44.89 |
| `ext4-global-lock-wait` | 6.52 | 1.23 |
| `mprotect-total` | 65.13 | 28.55 |
| `mprotect-aspace-lock-wait` | 1.89 | 1.53 |
| `mprotect-local-update` | 40.57 | 17.48 |
| `folio-load-wait` | 3.78 | 0.41 |

## 4. 结论（按“先验证、再优化”顺序）

1. **这次迭代成功把大问题往后推了**：  
   - `mprotect` 在 `2026-07-24 mprotect` 中仍是主瓶颈（`prebuild 8,931.02s`、`timed 4,312.28s`），而 per-VMA 后降到 `65.13s` / `28.55s`，下降量接近两个数量级。  
   - 与此同时，`ext4-global-lock-wait` 已降到几秒量级，不再是主线瓶颈。

2. **off-CPU 没有消失**：  
   仍是主要占比（尤其在 timed 阶段），说明后续主要收益不在 addrspace 锁和单点全局锁上，而在更深层并行与编译并发行为（依赖/IO/系统调用交互）上。

3. **本轮 `recheck` 不纳入主对比**：  
   该轮 `stalled`，有明确 reason “无工作进度 1080s 且 CPU 低于 20% 持续 180s”，不具备可比性。

4. **CPU 利用率侧可见并未整体退化到单核**：  
   - full 轮：平均 qemu CPU 约 **360%**，median ~**365%**（`run.json` `cpu_samples`）  
   - mprotect 轮：平均 **362%**，median ~**378%**  
   - per-VMA 轮：平均 **552%**，median ~**762%**，说明峰值并发更强  
   - recheck（失败样本）：平均 **276.7%**，并出现大量低速段，与挂起行为一致

## 5. 当前建议下一步

1. 继续保持 4 步目标：
   - 仍按 `per-VMA` 路线保留；
   - 仅在 `riscv-buildstorm-guest-recheck-20260724` 复测样本之外再加一轮短窗口 sanity；
   - 再次复测 full（不变口径）确认 `off-cpu + page-cache` 是否还在 timed 阶段占主导；
   - 与 Linux 口径对齐 ext4/pagecache 指标进行逐项减项对比。
2. 暂不把 `recheck` 的耗时参与任何趋势图或速度曲线，否则会把“卡住异常”当成性能回退。

## 6. 相关报告与机器可读摘要

- 主表：
  - `riscv-buildstorm-guest-20260721/optimization-report.md`
  - `riscv-buildstorm-guest-20260724/optimization-report.md`
  - `riscv-buildstorm-mprotect-20260724/mprotect-report.md`
  - `riscv-buildstorm-mprotect-per-vma-20260724/mprotect-per-vma-report.md`
- 机器可读摘要：
  - `riscv-buildstorm-guest-20260721/summary.json`
  - `riscv-buildstorm-guest-20260724/summary.json`
  - `riscv-buildstorm-mprotect-20260724/summary.json`
  - `riscv-buildstorm-mprotect-per-vma-20260724/summary.json`
  - `riscv-linux-buildstorm-20260721/summary.json`

Linux 对照报告可直接看：
- `riscv-linux-buildstorm-20260721/optimization-report.md`

