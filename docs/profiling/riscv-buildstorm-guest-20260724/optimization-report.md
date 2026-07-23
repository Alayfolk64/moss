# Moss RISC-V BuildStorm 全流程性能复测与火焰图报告

## 结论

本次测量覆盖了 cagent、完整 446-crate `tg-xtask` 冷预构建、正式 BuildStorm 编译以及结束前的 `sync`，没有采用固定 180 秒截取。RISC-V 使用 8 vCPU、8 GiB、与评测一致的 QEMU 设备参数，并通过 `-snapshot` 保证基础镜像不被写入。

最重要的结果如下：

1. 上一轮会停在 `440/446` 的现场没有再次出现。本轮连续越过 `440`、`441`、`442`、`443`、`444`、`445`，最终完成 `446/446`；正式 BuildStorm 也成功结束。
2. 预构建由 2026-07-21 基线的 **76 分 28.19 秒**降到 **41 分 41.09 秒**，减少 **45.49%**，速度约为原来的 **1.83 倍**。
3. 正式阶段 profile 窗口由 **42 分 04.89 秒**降到 **20 分 49.32 秒**，减少 **50.52%**，速度约为原来的 **2.02 倍**。
4. 正式 BuildStorm 自身的考核指标为 **1178.57 秒（19 分 38.57 秒）**，成功生成 1,681,000 字节产物。相比旧测量中 Cargo 自报的 2485 秒，减少 **52.57%**，约为 **2.11 倍**速度。
5. 旧报告的主要瓶颈——文件缺页、页缓存、ext4 全局锁和同步块 I/O——已显著下降。精确 ext4 全局锁竞争在预构建和正式阶段分别下降 **99.70%**、**99.68%**。
6. 当前最大热点已经转移到 `mprotect`。它占预构建 mutex 累计等待的 **94.21%**，占正式阶段的 **95.16%**；在非 idle CPU 样本中也分别占 **58.59%** 和 **56.82%**。下一步不应继续凭感觉扩大缺页改动，而应先拆分和量化 `mprotect` 中的地址空间锁、全局 `MMAP_RECORDS` 锁以及区间重建成本。

这组 A/B 比较跨越了 7 月 21 日之后的一组内存、页缓存、ext4、块 I/O、调度与退出路径修改，因此可以证明“当前整体版本明显快于旧整体版本”，但不能把全部加速严格归因于某一个补丁。

## 火焰图入口

### 全流程

- [CPU 火焰图](./guest-cpu-all.svg)
- [workload off-CPU 火焰图](./guest-offcpu-workload-all.svg)
- [全部任务 off-CPU 原始图](./guest-offcpu-all.svg)
- [mutex 等待火焰图](./guest-offcpu-lock-all.svg)
- [ext4 全局锁等待火焰图](./guest-ext4-lock-wait-all.svg)
- [ext4、页缓存和同步 I/O 组合图](./guest-fs-io-latency-all.svg)
- [folio 加载等待火焰图](./guest-folio-load-wait-all.svg)
- [invalidate 读锁等待火焰图](./guest-invalidate-read-wait-all.svg)
- [invalidate 写锁等待火焰图](./guest-invalidate-write-wait-all.svg)

### 完整预构建

- [预构建 CPU](./guest-cpu-prebuild.svg)
- [预构建 workload off-CPU](./guest-offcpu-workload-prebuild.svg)
- [预构建 mutex 等待](./guest-offcpu-lock-prebuild.svg)
- [预构建 ext4 全局锁等待](./guest-ext4-lock-wait-prebuild.svg)
- [预构建 FS/I/O](./guest-fs-io-latency-prebuild.svg)
- [预构建 folio 加载等待](./guest-folio-load-wait-prebuild.svg)
- [预构建 invalidate 读锁等待](./guest-invalidate-read-wait-prebuild.svg)
- [预构建 invalidate 写锁等待](./guest-invalidate-write-wait-prebuild.svg)

### 正式 BuildStorm

- [正式阶段 CPU](./guest-cpu-timed.svg)
- [正式阶段 workload off-CPU](./guest-offcpu-workload-timed.svg)
- [正式阶段 mutex 等待](./guest-offcpu-lock-timed.svg)
- [正式阶段 ext4 全局锁等待](./guest-ext4-lock-wait-timed.svg)
- [正式阶段 FS/I/O](./guest-fs-io-latency-timed.svg)
- [正式阶段 folio 加载等待](./guest-folio-load-wait-timed.svg)
- [正式阶段 invalidate 读锁等待](./guest-invalidate-read-wait-timed.svg)
- [正式阶段 invalidate 写锁等待](./guest-invalidate-write-wait-timed.svg)

空的 invalidate SVG 不是生成失败，而是该事件在本次完整工作负载中确实为零。机器可读的聚合数据见 [summary.json](./summary.json)，旧基线分析见 [2026-07-21 报告](../riscv-buildstorm-guest-20260721/optimization-report.md)。

## 测试配置与完整性

### QEMU 配置

实际启动参数为：

```text
qemu-system-riscv64
  -machine virt
  -kernel kernel-rv
  -m 8G
  -nographic
  -smp 8
  -bios default
  -snapshot
  -drive file=sdcard-rv.img,if=none,format=raw,id=x0
  -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
  -no-reboot
  -device virtio-net-device,netdev=net
  -netdev user,id=net
  -rtc base=utc
```

本次从 2026-07-24 03:36:45 运行至 04:39:32，宿主墙钟共 **3766.38 秒**。采集器在 guest 输出 `MOSS GUEST PROFILE READY` 后，通过 QEMU GDB stub 冻结虚拟机并复制二进制 profile 表。8 个 CPU profile shard 的锁字节全部为零，说明抓取时没有得到半写入记录。

profile 自检结果：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

cagent 为 **10/10 通过**。正式输出为：

```text
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=1178.57 \
cores=8 bytes=1681000 arch=riscv64
```

### 阶段耗时

| 阶段 | Guest 单调时钟耗时 | 外部结果 |
|---|---:|---|
| profile setup | 5.83 s | 完成 |
| 完整 446-crate 预构建 | 2501.09 s（41:41.09） | `446/446`，Cargo 自报 41m39s |
| 正式 profile 窗口 | 1249.32 s（20:49.32） | 完成 |
| 正式 BuildStorm 编译指标 | 1178.57 s（19:38.57） | `ok=true` |
| `axbuild` 内层 Cargo 构建 | 1167.88 s（19:27.88） | 完成 |

正式 profile 窗口比 `BUILDSTORM_COMPILE` 多 **70.75 秒**。它不是新的编译死锁：脚本在打印 group END 后还执行了 `sync`，正式阶段 ext4 inclusive 时间中有 **32.51 task-s（77.07%）**落在这条最终同步路径上；profile 还覆盖了正常收尾和打印 READY 标记的时间。

### 抽样口径

| 事件 | 口径 |
|---|---|
| CPU | 每个 vCPU 10 Hz |
| ext4 inclusive | 1/16 抽样，时间乘 16 还原 |
| page-cache inclusive | 1/32 抽样，时间乘 32 还原 |
| 全部 off-CPU | 1/8 抽样，时间乘 8 还原 |
| mutex | 全量记录 |
| 同步块读/写 | 全量记录 |
| 精确 ext4 全局锁等待 | 全量记录 |
| folio loading 等待 | 全量记录 |
| invalidate 读/写锁竞争 | 全量记录 |

这里的“乘 8/16/32”只是对抽中的等待时长做统计还原。例如每 8 个 off-CPU 区间记录 1 个，记录值乘 8 后估计总体量；它不是让一次等待变长。`sampled_count` 是实际留下的样本数，而精确事件的 count 就是真实事件数。

等待时间是累计 **task-time**。8 个任务并发各等待 10 秒会得到 80 task-s，所以等待时间可以大于墙钟时间。inclusive 事件还可能互相嵌套，不能把 ext4、页缓存、块 I/O 和 off-CPU 总量直接相加。

## 与 2026-07-21 基线的墙钟对比

| 指标 | 7 月 21 日 | 本次 | 减少 | 加速比 |
|---|---:|---:|---:|---:|
| 完整预构建 | 4588.19 s | 2501.09 s | 45.49% | 1.83× |
| 正式 profile 窗口 | 2524.89 s | 1249.32 s | 50.52% | 2.02× |
| 预构建 + 正式窗口 | 7113.08 s | 3750.42 s | 47.27% | 1.90× |
| 旧 Cargo 正式时间 / 新 BuildStorm 指标 | 2485.00 s | 1178.57 s | 52.57% | 2.11× |

旧报告正式阶段在 Cargo 已成功后还出现管道 EOF 挂起。本轮 BuildStorm 输出 group END，并继续走到 profile READY，说明那条退出链路在本轮也正常结束。

## 死锁复核

### 旧现场

旧运行长期停在：

```text
Building [=======================> ] 440/446
```

代码审计发现了以下锁环：

1. 前台缺页或读取方持有 `invalidate_lock.read`。
2. 它发现目标 folio 正由 readahead worker 加载，于是等待 folio 变为 uptodate。
3. truncate 排入 `invalidate_lock.write`；这把 sleep rwlock 对等待中的 writer 有优先权。
4. readahead worker完成 I/O 后需要重新取得 `invalidate_lock.read` 才能发布结果，但被排队 writer 阻塞。
5. 前台等待 worker，worker等待读锁，writer又等待前台释放读锁，形成闭环。

修复后的原则是：**任何等待 loading folio 的操作都不能持有 invalidate 锁**。等待完成后重新取得读锁并验证 folio 仍属于当前文件地址空间；writeback/reclaim 中的相同锁序也按此处理。

### 本次证据

本轮日志明确出现：

```text
440/446
441/446
442/446
443/446
444/446
445/446
Finished `dev` ... in 41m 39s
```

并继续完成正式 BuildStorm。新增精确事件的结果为：

| 阶段 | folio loading wait | invalidate read wait | invalidate write wait |
|---|---:|---:|---:|
| 预构建 | 2.464 task-s / 1563 次 | 0 / 0 次 | 0 / 0 次 |
| 正式阶段 | 0.166 task-s / 127 次 | 0 / 0 次 | 0 / 0 次 |

folio loading wait 的预构建平均值为 **1.577 ms**，最大 **105.799 ms**；正式阶段平均 **1.308 ms**，最大 **15.736 ms**。它证明合并同一 folio 的等待确实发生，但累计量很小。两个 invalidate 竞争事件均为零，也没有出现相应的 30 秒长等待告警。

因此可以得出有限而准确的结论：**已知的 440/446 锁环在这次完整高负载运行中没有复现，修复路径经受了一次全流程验证。** 这不是对所有 truncate、readahead、writeback 竞态的数学证明，仍应保留并发 truncate/read/write 回归测试。

## 缺页、页缓存、ext4 和块 I/O 的变化

### 完整预构建

| 事件 | 旧值 | 新值 | 减少 |
|---|---:|---:|---:|
| mutex 等待 | 11166.93 task-s | 7958.51 task-s | 28.73% |
| ext4 inclusive | 2222.89 task-s | 24.31 task-s | 98.91% |
| page-cache inclusive | 15042.35 task-s | 116.42 task-s | 99.23% |
| 同步块读取 | 393.08 task-s | 3.38 task-s | 99.14% |
| 同步块写入 | 63.58 task-s | 0.16 task-s | 99.74% |
| 精确 ext4 全局锁 | 981.46 task-s | 2.90 task-s | 99.70% |
| 全部 off-CPU | 214663.88 task-s | 146180.29 task-s | 31.90% |

### 正式 BuildStorm

| 事件 | 旧值 | 新值 | 减少 |
|---|---:|---:|---:|
| mutex 等待 | 8172.23 task-s | 2819.88 task-s | 65.49% |
| ext4 inclusive | 682.54 task-s | 42.18 task-s | 93.82% |
| page-cache inclusive | 9343.23 task-s | 37.52 task-s | 99.60% |
| 同步块读取 | 175.67 task-s | 1.85 task-s | 98.95% |
| 同步块写入 | 11.72 task-s | 0.54 task-s | 95.42% |
| 精确 ext4 全局锁 | 218.92 task-s | 0.69 task-s | 99.68% |
| 全部 off-CPU | 149651.89 task-s | 62340.97 task-s | 58.34% |

精确 ext4 锁等待的新结果为：

| 阶段 | 总等待 | 次数 | 平均 | 最大 |
|---|---:|---:|---:|---:|
| 预构建 | 2.901 task-s | 1620 | 1.791 ms | 107.471 ms |
| 正式阶段 | 0.692 task-s | 462 | 1.499 ms | 88.552 ms |

这说明 ext4 全局锁已不再是当前一阶瓶颈。它仍然存在，正确方向是继续缩短元数据临界区并验证 lwext4 并发安全，而不是看到一个 global mutex 就直接删除。页面缓存也不再表现为旧版那种持锁跨 I/O 的巨大等待。

## CPU 并行度

### Guest 内核周期样本

| 阶段 | 总样本 | idle | 非 idle | 非 idle 占比 | 8 核样本折算 |
|---|---:|---:|---:|---:|---:|
| 预构建 | 146929 | 108119 | 38810 | 26.41% | 2.11 核 |
| 正式阶段 | 76963 | 60214 | 16749 | 21.76% | 1.74 核 |

这个折算值不能当作硬件 PMU 利用率。RISC-V QEMU 在 guest 重负载时周期 tick 会受延迟、合并和 guest 调度状态影响；本轮 host 对 QEMU 进程的 15 秒采样则显示：

| 阶段 | 样本数 | 平均 QEMU CPU | 中位数 | P90 | 最大 |
|---|---:|---:|---:|---:|---:|
| 预构建 | 167 | 377.29% | 391.19% | 526.86% | 634.45% |
| 正式阶段 | 83 | 328.17% | 321.06% | 566.78% | 716.58% |

宿主的 100% 代表一个逻辑 CPU。因此 QEMU 确实曾长期并行使用约 3～4 个宿主核，并在部分区间达到 6～7 核；不能再说“始终只有一个 CPU 工作”。但它也没有持续占满 8 核。末尾 `444/446` 只剩 `axbuild`、`445/446` 只剩 `tg-xtask`，Cargo 依赖图和链接长尾天然会降低平均并行度。

Guest 样本与 host 样本绝对值不一致，所以报告不以其中任何一个单独证明利用率；更可靠的判断来自墙钟改善、多个编译进程的交叠、host CPU 曲线以及锁等待同时下降。

## off-CPU 分析

排除 readahead worker、块完成 worker、`gc`、`rebalance-task`、profile watchdog 等长期后台等待后，workload off-CPU 估计为：

| 原因 | 预构建 | 占比 | 正式阶段 | 占比 |
|---|---:|---:|---:|---:|
| futex | 73651.08 task-s | 65.70% | 24090.71 task-s | 64.93% |
| poll | 22335.30 task-s | 19.92% | 5867.90 task-s | 15.82% |
| mutex | 7891.05 task-s | 7.04% | 2795.22 task-s | 7.53% |
| pipe | 5001.13 task-s | 4.46% | 3644.09 task-s | 9.82% |
| waitpid | 1833.32 task-s | 1.64% | 357.74 task-s | 0.96% |
| nanosleep | 732.23 task-s | 0.65% | 0 | 0 |
| socket | 175.23 task-s | 0.16% | 87.57 task-s | 0.24% |
| 其他 | 480.56 task-s | 0.43% | 259.89 task-s | 0.70% |

futex、poll、pipe 和 waitpid 主要反映 Cargo/rustc 的线程协调、依赖等待和子进程通信，不能仅凭数值认定相应 syscall 慢。真正可以从内核栈直接归因的最大项仍是 mutex，其中绝大多数已经转移到 `mprotect`。

## 当前第一热点：`mprotect`

### 等待归因

按 mutex 完整栈分类：

| mutex 路径 | 预构建 | 占本阶段 mutex | 正式阶段 | 占本阶段 mutex |
|---|---:|---:|---:|---:|
| `mprotect` | 7498.09 task-s | 94.21% | 2683.28 task-s | 95.16% |
| `mmap` | 304.72 task-s | 3.83% | 75.60 task-s | 2.68% |
| `munmap` | 141.82 task-s | 1.78% | 56.95 task-s | 2.02% |
| concurrent page fault | 0.106 task-s | 0.0013% | 0.004 task-s | 0.00015% |

旧报告中，经过文件/COW 缺页的 mutex 等待占预构建 **80.25%**、正式阶段 **74.00%**。现在对应项已经接近零，说明缺页大临界区不再是当前主要 mutex 阻塞点。与此同时，`mprotect` 成为压倒性热点，这是一种明确的瓶颈迁移。

CPU 图给出同一结论：

| 路径 | 预构建非 idle CPU 占比 | 正式阶段非 idle CPU 占比 |
|---|---:|---:|
| `mprotect` | 58.59% | 56.82% |
| concurrent page fault | 9.90% | 10.44% |

`mprotect` 不仅让其他任务等待，它自己还消耗大量 CPU。

### 代码原因

当前 `sys_mprotect`：

1. 取得进程的 `AddrSpace` 独占 mutex。
2. 调用 `aspace.protect` 修改整个目标范围的页表权限。
3. 在地址空间锁仍未释放时调用 `update_mmap_prot`。
4. `update_mmap_prot` 再取得全局 `MMAP_RECORDS` mutex。
5. 它为每次调用分配 `records.len() + 2` 容量的新 `Vec`，`drain` 并扫描全部进程的全部记录，再用拆分后的记录替换原表。

也就是说，频繁执行 `mprotect` 的 rustc/LLVM 线程不只串行化同一进程的 VMA/PTE 修改，还会在一个系统级平面 `Vec` 上做 O(全局记录数) 的重建。火焰图中的 `sys_mprotect`、`update_mmap_prot`、`Vec` 和 BTree/PTE 路径互相印证，因此这个归因的可信度高。

## 下一步优化建议

### P0：先把 `mprotect` 拆成可测量的子项

在改数据结构前增加以下精确探针：

- `mprotect` 调用次数、页数和区间大小直方图；
- 等待进程 `AddrSpace` 锁的时间；
- `aspace.protect` 自身执行时间；
- 等待 `MMAP_RECORDS` 的时间；
- 扫描记录数、拆分数和合并数；
- 实际修改的 PTE 数和 TLB shootdown 次数。

这样可以回答当前 7498 task-s 主要是在等 `AddrSpace`，还是等全局记录表；CPU 的 58% 又主要花在页表遍历还是平面 Vec 重建。没有这层拆分就直接换结构，仍有误判风险。

### P0：将全局平面 `MMAP_RECORDS` 改为 per-mm 有序区间

建议每个 `AddrSpace/mm` 自己持有有序 VMA 区间结构：

- 以起始虚拟地址索引，而不是把所有 PID 混在一个全局 Vec；
- 从第一个可能重叠的区间开始，只访问目标范围覆盖的记录；
- 只拆分左右边界，更新中间区间；
- 提交后合并相邻且属性完全相同的 VMA；
- 预留拆分节点可在短临界区前完成，但最终更新必须在锁内重新验证。

这会把记录维护从 O(系统全部映射数) 降为接近 O(log N + K)，其中 N 是本进程 VMA 数，K 是真正重叠的区间数。

Linux 也使用 `mmap_lock` 保护 VMA 拓扑，所以正确目标不是让同一 mm 的 `mprotect/mmap/munmap` 完全无锁并行，而是：

- 锁只覆盖必须原子提交的 VMA/PTE 变更；
- 不在临界区扫描无关进程和无关区间；
- 不重复分配并重建整张表；
- 避免对权限本就相同的 PTE 做无效写入和 TLB 刷新。

### P1：批量页表权限更新和 TLB 失效

按连续页表层级批量修改 PTE，记录实际发生权限变化的连续范围，再合并 TLB shootdown。对于重复的相同权限请求可以快速返回。需要验证：

- 跨 VMA 边界时权限和 file sealing 语义正确；
- COW/共享文件写权限不能被错误放宽；
- 多线程同时 fault、unmap、mprotect 时使用 generation 或锁内重验；
- RISC-V 与 LoongArch 的远端 TLB shootdown 都覆盖目标 CPU。

### P1：保留 folio/invalidate 死锁探针

虽然本轮为零，仍应保留 `FolioLoadWait`、`InvalidateReadWait`、`InvalidateWriteWait` 以及长等待 watchdog。性能回归时，比重新依赖“卡在某个 crate”更快定位锁序问题。

### P2：最后再处理 Cargo 依赖长尾

即使内核并发完全改善，`444/446` 和 `445/446` 仍然只有单个依赖节点。应分别记录 `axbuild`、`tg-xtask`、最终链接和 LTO 的墙钟，判断能否：

- 复用评测允许的 host 工具预构建缓存；
- 降低不必要的 LTO/codegen 成本；
- 让链接器使用可控的线程数；
- 减少重复的 `mprotect` 调用或 JIT/链接器内存权限翻转。

这属于构建图优化，不能用增加内核 worker 数替代。

## `BUILDSTORM_MINIBUILD fail` 的说明

本次完整性能测量开始时输出了：

```text
BUILDSTORM_TOOLCHAIN ok
BUILDSTORM_MINIBUILD fail
```

随后完整预构建和正式 BuildStorm 均成功，因此这个 fail 没有污染正式 1178.57 秒的成功判定。检查官方干净 rootfs 后确认 `/work/tgoskits/target` 父目录不存在，而 Cargo 的全局 target 配置会直接尝试在其下创建子目录，导致 minibuild 的早期探测失败。

测量结束后已经在 `init_final.sh` 的 BuildStorm 入口增加：

```sh
mkdir -p /work/tgoskits/target
```

这是基于明确错误和镜像目录状态做的正确性修复，但它发生在本轮 profile 内核构建之后，**不能声称已被这组火焰图动态验证**。它只创建缺失父目录，不改变本报告中的正式编译性能代码；下一次正常短验收应确认 `BUILDSTORM_MINIBUILD` 变为 pass。

## 复现方式

构建 profiling 内核后执行：

```sh
scripts/run_guest_profile_full.py \
  --out docs/profiling/riscv-buildstorm-guest-20260724
```

采集结束后：

```sh
scripts/extract_guest_profile_dump.py \
  --elf kernel-rv.profile.elf \
  --profiles docs/profiling/riscv-buildstorm-guest-20260724/profiles.bin \
  --statics docs/profiling/riscv-buildstorm-guest-20260724/profile-statics.bin \
  > docs/profiling/riscv-buildstorm-guest-20260724/profile.log

scripts/render_guest_profile.py \
  --profile docs/profiling/riscv-buildstorm-guest-20260724/profile.log \
  --out docs/profiling/riscv-buildstorm-guest-20260724 \
  --flamegraph /tmp/moss-flamegraph/flamegraph.pl
```

本次 SVG 使用 Brendan Gregg FlameGraph 仓库 commit：

```text
41fee1f99f9276008b7cd112fca19dc3ea84ac32
```

## 产物与校验和

大体积 console、二进制 profile 表和 folded 中间文件保留在本机，由本目录 `.gitignore` 排除；报告、`summary.json` 和 SVG 可以提交。

```text
kernel-rv
  6b51e92f577e818bf3c04a74bb720cc3979bbcadd9487461e6896daa17072fe4

kernel-rv.profile.elf
  42576dc245c1dc783755d7b0c78e99d709d8a7bdd77aad497a9c35b4295de40d

profiles.bin
  b3400a1269bd80b37bbf0535b3cc63e8b8e8066f77aea0c4d483be279ae5d4a5

profile-statics.bin
  10b788b523b04f72ccbbde11474324bff969c3dee193f58c12206ed803876f61

profile.log
  cfea8d8810e3fa72b57ce31703f6c722bd8623a969e7553dfa65f59fdb7db02f

summary.json
  0b3cbe6235ed66e3799a03995597e4a034abcd246093c73eb9b70da7f4cb4a78
```

## 最终判断

这次结果已经同时回答了三个问题：

- **死锁是否还在：** 已知 440/446 锁环在一次完整运行中没有复现，446-crate 和正式构建都完成。
- **之前的缺页/文件系统优化是否有效：** 整体墙钟约减半，page-cache、ext4 锁和同步 I/O 等待下降 93%～99% 以上；缺页 mutex 已从第一热点降到可忽略的等待占比。
- **为什么仍未吃满 8 核：** 当前可操作的内核第一热点已经变成 `mprotect` 的进程地址空间串行化和全局映射记录重建，此外还存在 Cargo 依赖图与最终链接的单任务长尾。

所以下一轮应围绕 `mprotect` 做精确拆分和 per-mm VMA 数据结构优化，再用同一套完整采样流程 A/B 对比；继续扩大 folio 或 ext4 改动已经不是当前证据支持的最高优先级。
