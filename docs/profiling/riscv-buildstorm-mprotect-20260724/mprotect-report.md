# RISC-V BuildStorm `mprotect` 详细测量报告

## 结论

本轮测量已经把旧报告中的 `mprotect` 热点拆开。结论很明确：

1. 当前第一瓶颈不是进程 `AddrSpace` 锁，而是全系统共享的
   `MMAP_RECORDS: Mutex<Vec<MapRecord>>`。
2. 预构建阶段，两次 `MMAP_RECORDS` 锁等待合计为 **6591.79 task-s**，
   占 `mprotect` 累计任务时间的 **73.81%**；正式阶段为
   **3164.88 task-s**，占 **73.39%**。
3. `AddrSpace` 锁等待只占 `mprotect` 累计任务时间的 **1.10%**
   和 **1.29%**。继续只拆 `AddrSpace` 锁不会解决当前主瓶颈。
4. 每次 `mprotect` 平均只覆盖约 5～6 个 4 KiB 页，约 74% 的调用只覆盖
   1 页；但是每次权限更新平均扫描约 **3.16 万条全系统映射记录**。
5. 每次扫描通常只命中约 1 条记录并产生约 1 次边界拆分。当前实现用
   O(全系统记录数) 的工作处理 O(1) 个真正相关区间。
6. 约 99% 的调用请求 `PROT_READ | PROT_WRITE`。所有被统计的调用均成功，
   因此 write-seal 检查几乎每次都做了一次昂贵的全表否定查询。
7. 预构建期间两个全表扫描合计检查 **897.32 亿**条记录，正式阶段检查
   **408.90 亿**条。这个数据直接解释了 CPU 开销和全局锁竞争。

下一步应把平面的全局 `Vec<MapRecord>` 改成每个进程地址空间自己的有序
VMA 区间结构，并在一次 per-mm 写事务中完成 seal 验证、页表权限修改、
区间拆分和相邻区间合并。

## 测量对象与结果

### 运行环境

- 日期：2026-07-24
- 架构：RISC-V 64
- QEMU：`virt`、8 vCPU、8 GiB
- 根文件系统：`sdcard-rv.img`
- 磁盘模式：`-snapshot`
- 网络和块设备：与 `run_rv.sh`/评测命令相同
- profiling 内核 SHA-256：
  `a73e8809f555f8d331f7d0aabc2e23950868613f9fcedb8b5fd673b4d407deff`
- profiling ELF SHA-256：
  `fac3bdfacdc3eeb498878b68bda4c71e68d8694d2fc45429b935e8ef7d16e7dc`
- runner 状态：`complete`
- GDB 导出：第 1 次成功，8 个 profile shard 均不处于写入状态
- 宿主起止时间：06:42:36～07:44:22
- 宿主总墙钟：**3706.29 秒（61 分 46.29 秒）**

实际 QEMU 命令和每 15 秒一次的宿主 CPU 采样保存在
[run.json](./run.json)。

### 功能结果

- cagent：10/10 通过
- Rust 工具链检查：通过
- tg-xtask 预构建：成功
- 正式 BuildStorm：成功
- 正式产物：1,681,000 字节
- `BUILDSTORM_MINIBUILD`：仍然为 fail，详见后文
- panic：无
- profile 丢样：
  - `dropped_cpu=0`
  - `dropped_wait=0`
  - `dropped_pending=0`
  - `skipped_cpu=0`

### 时间

| 指标 | 本轮 |
|---|---:|
| profiling setup | 5.76 秒 |
| tg-xtask 预构建 phase | 2363.83 秒（39 分 23.83 秒） |
| Cargo 自报预构建 | 39 分 22 秒 |
| 正式 BuildStorm | **1243.89 秒（20 分 43.89 秒）** |
| Cargo 自报 release | 20 分 24 秒 |
| profiling timed phase | 1321.04 秒（22 分 01.04 秒） |
| 宿主总墙钟 | 3706.29 秒（61 分 46.29 秒） |

`timed phase` 比 `BUILDSTORM_COMPILE elapsed_s` 多 77.15 秒，因为 phase
直到测试脚本返回后才停止，包含产物查找、结果输出和末尾 `sync`。宿主采样
显示 BuildStorm group 结束后的约 75 秒平均只有 85.16% QEMU CPU，符合
同步和收尾阶段，而不是正式 Cargo 编译。

## 测量口径

### 100% 精确计数

以下数据不抽样，每个通过初始 `prot` 参数验证并进入 `sys_mprotect` 主体的
调用都会计数：

- 调用、成功、失败；
- 请求页数、单次最大页数；
- 请求大小直方图；
- 8 种 R/W/X 权限组合；
- seal 检查扫描记录数；
- 权限更新扫描记录数；
- 真正重叠记录数；
- 拆分数；
- 更新后记录数。

精确计数保存在独立的 896 字节
[mprotect-stats.bin](./mprotect-stats.bin)，不依赖耗时火焰图抽样。

### 耗时与调用栈

| 数据 | 采样口径 |
|---|---:|
| guest CPU | 10 Hz |
| off-CPU | 每 8 次取 1 次，耗时乘 8 |
| ext4 | 每 16 次取 1 次，耗时乘 16 |
| page cache | 每 32 次取 1 次，耗时乘 32 |
| 7 类 mprotect 耗时 | 每 CPU、每事件每 32 次取 1 次，耗时乘 32 |

`mprotect` 使用 1/32 抽样的原因是调用量达到百万级。如果每次调用的每个
子阶段都抓内核栈并写 profile 哈希表，测量本身会显著延长锁临界区并扰动
结果。精确次数和区间分布仍然是 100% 计数。

抽样是每 CPU、每事件独立的确定性抽样，因此：

- 累计耗时乘 32 是估计值；
- `sampled_count × 32` 只能估计发生次数；
- `max_sample` 是被抽中的样本最大值，不是全体调用的绝对最大值；
- 精确调用数、权限、页数和记录扫描数不受这项近似影响。

### 7 个 mprotect 子阶段

| 事件 | 含义 |
|---|---|
| `mprotect-total` | 一个有效 `mprotect` 的端到端时间 |
| `mprotect-aspace-lock-wait` | 进程 `AddrSpace` 锁发生竞争后的等待 |
| `mprotect-seal-lock-wait` | write-seal 检查前等待 `MMAP_RECORDS` |
| `mprotect-seal-scan` | 扫描记录判断共享文件映射是否 write-sealed |
| `mprotect-page-table` | `aspace.protect` 修改页表权限 |
| `mprotect-record-lock-wait` | 更新映射记录前再次等待 `MMAP_RECORDS` |
| `mprotect-record-update` | 全表 drain、拆分并重建映射记录 |

只有 `try_lock` 失败后真正发生的等待才进入三个 lock-wait 事件。无竞争快
路径不会产生一条零等待记录。

`task-s` 是所有任务耗时相加后的累计任务时间，不是阶段墙钟。多个任务可以
同时等待，所以它可以明显大于墙钟。

## 调用规模

| 指标 | 预构建 | 正式阶段 |
|---|---:|---:|
| 调用 | 1,421,438 | 650,266 |
| 成功 | 1,421,438 | 650,266 |
| 失败 | 0 | 0 |
| 调用频率 | 601.33 次/秒 | 492.24 次/秒 |
| 累计请求页数 | 7,363,186 | 3,800,085 |
| 累计请求范围 | 28.09 GiB | 14.50 GiB |
| 平均页数/调用 | 5.18 | 5.84 |
| 最大页数/调用 | 2873（11.22 MiB） | 3104（12.13 MiB） |

累计请求范围会重复计算同一虚拟地址，不代表实际新增了相同大小的常驻内存。

### 请求大小

| 4 KiB 页数 | 预构建 | 占比 | 正式阶段 | 占比 |
|---|---:|---:|---:|---:|
| 1 | 1,051,788 | 73.995% | 479,050 | 73.670% |
| 2～4 | 219,843 | 15.466% | 96,929 | 14.906% |
| 5～16 | 102,265 | 7.194% | 50,822 | 7.816% |
| 17～64 | 37,963 | 2.671% | 18,989 | 2.920% |
| 65～256 | 5,402 | 0.380% | 2,082 | 0.320% |
| 257～1024 | 3,288 | 0.231% | 1,855 | 0.285% |
| 1025+ | 889 | 0.063% | 539 | 0.083% |

因此热点不是少量超大范围权限修改，而是海量 4 KiB 小范围调用。

### 权限组合

| 权限 | 预构建 | 占比 | 正式阶段 | 占比 |
|---|---:|---:|---:|---:|
| `PROT_NONE` | 4,649 | 0.327% | 2,764 | 0.425% |
| `PROT_READ` | 13,987 | 0.984% | 3,796 | 0.584% |
| `PROT_READ \| PROT_WRITE` | 1,402,798 | **98.689%** | 643,688 | **98.988%** |
| `PROT_READ \| PROT_EXEC` | 2 | 0.00014% | 9 | 0.00138% |
| `PROT_READ \| PROT_WRITE \| PROT_EXEC` | 2 | 0.00014% | 9 | 0.00138% |
| 其余组合 | 0 | 0 | 0 | 0 |

写权限请求会先进入 write-seal 检查。由于本轮所有调用都成功，绝大多数
`PROT_READ | PROT_WRITE` 调用付出的都是“完整扫描后确认不存在 sealed
匹配”的否定查询成本。

## 分阶段耗时

### 预构建

| 子阶段 | 累计 task-s | 占 mprotect | 按全部调用摊薄 | 抽样数 | 被抽中最大值 |
|---|---:|---:|---:|---:|---:|
| mprotect total | 8931.02 | 100% | 6.283 ms/次 | 44,419 | 299.357 ms |
| AddrSpace 锁等待 | 97.89 | 1.10% | 0.069 ms/次 | 581 | 78.985 ms |
| seal 记录锁等待 | 3565.99 | **39.93%** | 2.509 ms/次 | 32,514 | 149.363 ms |
| seal 扫描 | 301.89 | 3.38% | 0.212 ms/次 | 43,837 | 3.243 ms |
| 页表权限修改 | 418.04 | 4.68% | 0.294 ms/次 | 44,419 | 4.731 ms |
| update 记录锁等待 | 3025.80 | **33.88%** | 2.129 ms/次 | 29,372 | 101.603 ms |
| 记录重建 | 1365.60 | **15.29%** | 0.961 ms/次 | 44,419 | 6.408 ms |

### 正式阶段

| 子阶段 | 累计 task-s | 占 mprotect | 按全部调用摊薄 | 抽样数 | 被抽中最大值 |
|---|---:|---:|---:|---:|---:|
| mprotect total | 4312.28 | 100% | 6.632 ms/次 | 20,320 | 206.086 ms |
| AddrSpace 锁等待 | 55.65 | 1.29% | 0.086 ms/次 | 477 | 37.293 ms |
| seal 记录锁等待 | 1755.63 | **40.71%** | 2.700 ms/次 | 13,232 | 177.489 ms |
| seal 扫描 | 131.07 | 3.04% | 0.202 ms/次 | 20,116 | 1.394 ms |
| 页表权限修改 | 224.80 | 5.21% | 0.346 ms/次 | 20,320 | 5.485 ms |
| update 记录锁等待 | 1409.25 | **32.68%** | 2.167 ms/次 | 11,328 | 71.283 ms |
| 记录重建 | 639.57 | **14.83%** | 0.984 ms/次 | 20,320 | 6.190 ms |

各子事件独立抽样，因此子项之和不会严格等于 total。预构建可解释
98.25%，正式阶段可解释 97.77%，误差与未单列的普通 syscall 开销都很小。

### 锁竞争频率估计

| 阶段 | 锁 | 估计竞争比例 | 被抽中竞争的平均等待 | 被抽中最大等待 |
|---|---|---:|---:|---:|
| 预构建 | AddrSpace | 1.31% | 5.27 ms | 78.99 ms |
| 预构建 | seal `MMAP_RECORDS` | 74.17% 的写请求 | 3.43 ms | 149.36 ms |
| 预构建 | update `MMAP_RECORDS` | 66.12% | 3.22 ms | 101.60 ms |
| 正式 | AddrSpace | 2.35% | 3.65 ms | 37.29 ms |
| 正式 | seal `MMAP_RECORDS` | 65.78% 的写请求 | 4.15 ms | 177.49 ms |
| 正式 | update `MMAP_RECORDS` | 55.75% | 3.89 ms | 71.28 ms |

竞争比例由 `sampled_count × 32 / 精确调用数` 得到，是系统抽样估计，不是
精确计数。数量级足以区分约 1% 的 AddrSpace 竞争和 56%～74% 的全局记录
锁竞争。

### 对并行度的实际影响

| 阶段 | phase 墙钟 | mprotect task-s | 平均处于 mprotect 的任务 | 两段记录锁等待 task-s | 平均等待记录锁的任务 |
|---|---:|---:|---:|---:|---:|
| 预构建 | 2363.83 秒 | 8931.02 | **3.78** | 6591.79 | **2.79** |
| 正式 | 1321.04 秒 | 4312.28 | **3.26** | 3164.88 | **2.40** |

“平均等待任务”是累计等待时间除以阶段墙钟。例如预构建的 2.79 表示整个
阶段平均等价于 2.79 个任务处在两段 `MMAP_RECORDS` 等待中。它不是 CPU
利用率，也不能直接换算为可节省的墙钟秒数，但能说明锁 convoy 的规模。

两段专用记录锁等待占通用 mutex 等待的比例为：

| 阶段 | 两段记录锁等待 | 全部 mutex 等待 | 占比 |
|---|---:|---:|---:|
| 预构建 | 6591.79 task-s | 7021.34 task-s | **93.88%** |
| 正式 | 3164.88 task-s | 3317.64 task-s | **95.40%** |

这与旧报告通过通用 mutex 栈得到的归因一致，现在已经进一步定位到同一把
全局记录锁的两个具体获取点。

## 映射记录扫描与碎片化

| 指标 | 预构建 | 正式阶段 |
|---|---:|---:|
| seal 扫描记录总数 | 44,582,080,338 | 20,361,864,950 |
| seal 平均扫描/写调用 | 31,780.78 | 31,632.69 |
| seal 单次最大扫描 | 83,637 | 71,108 |
| update 扫描记录总数 | 45,149,576,336 | 20,527,736,724 |
| update 平均扫描/调用 | 31,763.31 | 31,568.21 |
| update 单次最大扫描 | 83,638 | 71,108 |
| 真正重叠记录 | 1,418,352 | 649,369 |
| 重叠记录/调用 | 0.9978 | 0.9986 |
| 拆分数 | 1,418,303 | 649,296 |
| 拆分/调用 | 0.9978 | 0.9985 |
| 两遍扫描合计 | **89,731,656,674** | **40,889,601,674** |

当前代码的行为与数据完全对应：

1. [`MMAP_RECORDS`](../../../kernel/src/syscall/mm/mmap.rs#L157) 是所有 PID
   共享的一张平面 `Vec`。
2. write-seal 检查从尾到头遍历记录。不存在匹配时必须扫完整张表，见
   [`find_recorded_write_sealed_shared_file`](../../../kernel/src/syscall/mm/mmap.rs#L502)。
3. 权限更新新分配 `records.len() + 2` 的 `Vec`，对旧表执行 `drain(..)`，
   再逐条复制和拆分，见
   [`update_mmap_prot_records`](../../../kernel/src/syscall/mm/mmap.rs#L303)。
4. `sys_mprotect` 在持有本进程 `AddrSpace` 锁时，先为 seal 检查取得一次
   全局锁，修改页表后又为记录更新取得第二次全局锁，见
   [`sys_mprotect`](../../../kernel/src/syscall/mm/mmap.rs#L806)。
5. 更新函数没有把属性完全相同的相邻区间重新合并。接近每次调用一次的拆分
   会持续制造更多记录，使后续每次全表扫描和复制更贵。

所以这里同时存在三种成本：

- 所有进程争用一把系统级 mutex；
- 每次调用做两遍约 3.16 万条记录的线性扫描；
- 第二遍还要重新分配并复制整张表。

只把一把 mutex 改成另一种锁，而保留平面全表扫描，不能解决 CPU 和内存
复制成本。只优化页表遍历也只能处理约 5% 的 mprotect 时间。

## 哪些任务在付出成本

| 阶段 | 子项 | rustc task-s | rustc 占该子项 |
|---|---|---:|---:|
| 预构建 | seal 记录锁等待 | 3148.61 | 88.30% |
| 预构建 | update 记录锁等待 | 2622.40 | 86.67% |
| 预构建 | 记录重建 | 1184.69 | 86.75% |
| 正式 | seal 记录锁等待 | 1562.37 | 88.99% |
| 正式 | update 记录锁等待 | 1218.61 | 86.47% |
| 正式 | 记录重建 | 551.83 | 86.28% |

其余主要来自 LLVM `opt cgu.*`、LTO、链接器和少量 coordinator。热点高度
集中在真实编译负载，不是 profiler watchdog、gc 或后台协调线程制造的
假象。

## CPU 并行度

### Guest 10 Hz 周期样本

| 阶段 | 总样本 | idle | 非 idle | 非 idle 占比 | 8 核折算 |
|---|---:|---:|---:|---:|---:|
| 预构建 | 135,592 | 98,015 | 37,577 | 27.71% | 2.22 核 |
| timed phase | 83,127 | 65,609 | 17,518 | 21.07% | 1.69 核 |

RISC-V QEMU 的 guest tick 会受中断延迟和合并影响，不能把这张表当成硬件
PMU 利用率。

### 宿主 QEMU 进程

宿主 100% 表示一个逻辑 CPU：

| 阶段 | 样本 | 平均 | 中位数 | P90 | 最大 |
|---|---:|---:|---:|---:|---:|
| 预构建 | 157 | 394.35% | 410.12% | 538.92% | 635.11% |
| 正式 BuildStorm group | 84 | 320.36% | 325.86% | 529.25% | 650.84% |

运行中可以明确观察到：

- 并行密集阶段长期使用约 3～5 个宿主核，峰值约 6.5 核；
- `444/446` 只剩 `axbuild`、`445/446` 只剩 `tg-xtask` 时约为 1 核；
- 长尾结束后会再次出现多核工作波次；
- 没有“整个流程始终只有一个有效编译进程”的现象；
- 但全局 `MMAP_RECORDS` 平均阻塞 2.4～2.8 个任务，仍然显著压低可用并行度。

## 观察者效应控制

最初的试运行对 7 类 mprotect 事件全部逐次抓栈，得到：

| 指标 | 逐次抓栈试运行 | 最终 1/32 运行 |
|---|---:|---:|
| 预构建 phase | 3283.87 秒 | 2363.83 秒 |
| Cargo 自报预构建 | 54 分 42 秒 | 39 分 22 秒 |
| 正式 BuildStorm | 1526.83 秒 | 1243.89 秒 |

逐次抓栈会在百万级调用上执行栈回溯、profile 哈希查找和表更新，还会延长
被测锁附近的代码路径，因此不适合作为最终绝对时间。最终版本：

- 保留全部精确计数；
- 仅对耗时栈做 1/32 抽样；
- 在释放 `MMAP_RECORDS` 后再提交精确统计和 record-update profile；
- 把逐次抓栈试运行排除在正式报告之外。

与[上一份有效完整报告](../riscv-buildstorm-guest-20260724/optimization-report.md)
相比：

| 指标 | 上一份有效报告 | 本轮低扰动测量 | 差值 |
|---|---:|---:|---:|
| 预构建 phase | 2501.09 秒 | 2363.83 秒 | -5.49% |
| 正式 BuildStorm | 1178.57 秒 | 1243.89 秒 | +5.54% |
| 宿主总墙钟 | 3766.38 秒 | 3706.29 秒 | -1.60% |

两轮包含的 profiling 事件不同，而且 QEMU 本身存在运行波动，所以这张表
只能说明最终探针扰动已经从约 30% 降到个位数范围，不能把 ±5% 当成产品
性能优化或回退。

## 优化方案

### P0：每个 mm 独立的有序 VMA 区间

将 `MapRecord` 从全局 `Vec` 迁移到对应 `AddrSpace`/进程内：

- 以起始虚拟地址为键；
- 从第一个可能重叠的区间开始访问；
- 查询成本接近 O(log N + K)，N 是当前 mm 的 VMA 数，K 是真正重叠区间数；
- 不再扫描其他 rustc、链接器和 Cargo 进程的记录；
- 不同进程的 `mprotect` 不再争用同一把记录锁。

可以先使用 Rust `BTreeMap` 完成正确实现，不必一开始复刻 Linux Maple Tree
的全部复杂度。重要的是 per-mm、有序、范围查询和局部修改。

### P0：一次 per-mm 写事务

Linux 的目标不是让同一 mm 的 VMA 拓扑修改完全无锁，而是避免系统级全局
串行。建议一次写事务完成：

1. 定位目标 VMA 范围；
2. 检查 write-seal；
3. 验证权限和覆盖完整性；
4. 修改需要变化的 PTE；
5. 只拆分左右边界；
6. 更新中间区间；
7. 合并属性完全相同的相邻 VMA；
8. 合并 TLB shootdown 后释放锁。

这样可以删除 `sys_mprotect` 当前两次 `MMAP_RECORDS` 获取，也不会在持有
本进程 `AddrSpace` 时等待其他进程的记录更新。

### P0：write-seal 快速否定

本轮约 99% 请求包含写权限，但 seal 检查全部是否定结果。每个 mm 可以维护：

- 是否存在 write-sealed shared-file VMA；
- 或 sealed VMA 的有序子集/计数。

当计数为零时直接跳过 seal 区间扫描。存在 sealed VMA 时仍必须做精确范围
查询，不能为了性能放宽 seal 语义。

### P0：拆分后立即合并

本轮每个调用约产生一次拆分。如果相邻区间的以下属性完全相同，应立即合并：

- `prot`
- shared/private
- file-backed/anonymous
- write-sealed
- locked
- 连续文件偏移和相同后端对象

合并必须考虑文件偏移等当前 `MapRecord` 尚未完整表达的语义，不能只比较
当前几个布尔字段后盲目合并。

### P1：无变化权限快速路径

精确计数表明绝大多数调用是小范围 RW，但本轮没有统计“旧权限是否已经等于
新权限”。迁移到有序 VMA 后，应先增加 `unchanged_pages` 和
`changed_ptes` 计数，再决定是否：

- 对权限完全相同的完整 VMA 直接返回；
- 跳过未映射 PTE；
- 只刷新真正改变的连续 TLB 范围。

在没有旧/新权限证据前，不能把所有 RW 调用直接当成冗余调用。

### 验收条件

先做静态正确性检查和针对性小测试，最后只跑一次完整一小时验收：

1. `PROT_NONE/R/RW/RX` 和部分 VMA 区间拆分正确；
2. 相邻相同 VMA 能重新合并；
3. shared file write-seal 仍返回 `EPERM`；
4. `mmap/munmap/mremap/fork/exec/exit` 后元数据不残留；
5. 同一 mm 多线程和不同 mm 多进程并发 `mprotect` 不死锁；
6. 并发 page fault 与 `mprotect` 不安装过期权限；
7. `update_records_scanned/call` 从约 31,000 降到与本 mm 重叠区间数量同阶；
8. 两类全局记录锁事件消失，或迁移为只反映同一 mm 必要串行的事件；
9. `dropped_*` 和 `skipped_cpu` 保持 0；
10. cagent 10/10、BuildStorm 成功，正式墙钟至少不回退。

不能把 6591.79/3164.88 task-s 直接当作可节省的墙钟，因为等待是并发发生
的。优化效果必须通过同一口径 A/B 实测确认。

## `BUILDSTORM_MINIBUILD fail`

本轮仍输出：

```text
BUILDSTORM_TOOLCHAIN ok
BUILDSTORM_MINIBUILD fail
```

随后 tg-xtask 预构建和正式 BuildStorm 都成功，所以它没有污染
`BUILDSTORM_COMPILE ok=true` 和 1243.89 秒计时。

此前加入的 `/work/tgoskits/target` 父目录创建没有让该项变为 pass，说明
“只缺少 target 父目录”的诊断不完整。官方脚本把 minibuild 的 stderr
重定向到 `/dev/null`，本轮性能测量不能进一步证明具体失败点。应单独复制
该探测命令并保留 stderr 做一次短正确性排查，不应为此重跑完整 profile。

## 火焰图

### 预构建

- [CPU](./guest-cpu-prebuild.svg)
- [全部 mprotect](./guest-mprotect-total-prebuild.svg)
- [AddrSpace 锁等待](./guest-mprotect-aspace-lock-wait-prebuild.svg)
- [seal 记录锁等待](./guest-mprotect-seal-lock-wait-prebuild.svg)
- [seal 扫描](./guest-mprotect-seal-scan-prebuild.svg)
- [页表权限修改](./guest-mprotect-page-table-prebuild.svg)
- [update 记录锁等待](./guest-mprotect-record-lock-wait-prebuild.svg)
- [记录重建](./guest-mprotect-record-update-prebuild.svg)
- [通用 mutex 等待](./guest-offcpu-lock-prebuild.svg)
- [workload off-CPU](./guest-offcpu-workload-prebuild.svg)

### 正式阶段

- [CPU](./guest-cpu-timed.svg)
- [全部 mprotect](./guest-mprotect-total-timed.svg)
- [AddrSpace 锁等待](./guest-mprotect-aspace-lock-wait-timed.svg)
- [seal 记录锁等待](./guest-mprotect-seal-lock-wait-timed.svg)
- [seal 扫描](./guest-mprotect-seal-scan-timed.svg)
- [页表权限修改](./guest-mprotect-page-table-timed.svg)
- [update 记录锁等待](./guest-mprotect-record-lock-wait-timed.svg)
- [记录重建](./guest-mprotect-record-update-timed.svg)
- [通用 mutex 等待](./guest-offcpu-lock-timed.svg)
- [workload off-CPU](./guest-offcpu-workload-timed.svg)

每个 mprotect 事件还生成了 `setup` 和 `all` 版本，共 28 张 mprotect SVG。

## 原始产物与复现

大体积的可复现中间文件由本目录 `.gitignore` 排除：

- `console.log`
- `profiles.bin`
- `profile-statics.bin`
- `mprotect-stats.bin`
- `profile.log`
- `*.folded`
- `run.json`

提交保留：

- 本报告；
- [summary.json](./summary.json)；
- 所有 SVG 火焰图。

关键文件校验：

```text
profiles.bin       2ec3e1dd6e0a8cc9311e5896cbb78e4af4b4dddacff4f08abe8e4a8585ebb23b
mprotect-stats.bin 8ee91fa6bbece41d1acbf551ca4e6ada3ff1b0b086e798c555ba90c15ab62dee
summary.json       d56b9a9acf7dd1e350ec317563c879f44ff6de1efe7067dd2765d88c6e4c4b4e
```

复现命令：

```sh
make PROFILE=y ARCH=riscv64 MEM=8G SMP=8 kernel-rv

scripts/run_guest_profile_full.py \
  --out docs/profiling/riscv-buildstorm-mprotect-20260724

python3 scripts/extract_guest_profile_dump.py \
  --profiles docs/profiling/riscv-buildstorm-mprotect-20260724/profiles.bin \
  --statics docs/profiling/riscv-buildstorm-mprotect-20260724/profile-statics.bin \
  --mprotect-stats docs/profiling/riscv-buildstorm-mprotect-20260724/mprotect-stats.bin \
  --output docs/profiling/riscv-buildstorm-mprotect-20260724/profile.log

python3 scripts/render_guest_profile.py \
  --log docs/profiling/riscv-buildstorm-mprotect-20260724/profile.log \
  --elf kernel-rv.profile.elf \
  --out docs/profiling/riscv-buildstorm-mprotect-20260724 \
  --flamegraph-dir /tmp/moss-flamegraph
```
