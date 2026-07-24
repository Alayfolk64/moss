# RISC-V BuildStorm 最新完整探测瓶颈报告

## 结论先行

最新一次可用的完整样本是
`riscv-buildstorm-mprotect-per-vma-20260724`。它从没有 `target` 编译产物的
官方镜像开始，使用 8 vCPU、8 GiB 和 `-snapshot`，完整通过 cagent、
tg-xtask 预构建和正式 BuildStorm。

这次重新从 `summary.json`、CPU/off-CPU folded 栈和 `run.json` 计算后，
“谁占大头”需要分成三层回答：

1. **CPU 图上最宽的是 Moss 的 idle/run-queue 路径。**
   预构建 63.50%、正式阶段 71.89% 的已记录 CPU 样本标记为
   `task:idle -> axtask::run_queue::current_run_queue`。这不是 rustc 在执行，
   也不是有用的内核工作。
2. **off-CPU 图上数值最大的是 futex。**
   futex 占 workload off-CPU 的 66.02%/70.80%，但 Linux 同口径是
   62.55%/69.50%。它主要是 rustc、LLVM、Cargo 和 Tokio 的线程协调，
   不能把全部 futex 睡眠当成 Moss 内核性能缺陷。
3. **当前最值得处理的 Moss 内核热点是 AddrSpace，而不是旧的
   `mprotect` 全局记录锁，也不是 ext4 全局锁。**
   AddrSpace 读写锁的实际睡眠区间估算为 98.06/64.45 task-s，其中
   55.93%/65.56% 发生在缺页或内核代用户填页时。CPU 活跃样本中，
   地址空间销毁/逐页 unmap 路径约占 33%，缺页路径约占 23%。

因此，下一轮不应继续围绕已经消失的 `MMAP_RECORDS` 热点优化。优先级应是：

1. 测出 AddrSpace **写锁持有者**及持锁时长，重点看
   `clear/unmap/mmap/mprotect` 对缺页读锁的阻塞；
2. 将进程退出和 `execve` 的逐 VMA/PTE 销毁拆成“锁内摘除、锁外回收”，
   同时批量刷新 TLB；
3. 再拆 page-cache 的查找、分配、填充、dirty/writeback 和最终 `sync`
   成本；
4. 最后才是 ext4 全局锁；它现在只有 6.52/1.23 task-s。

## 样本与结果

| 项目 | 预构建 | 正式阶段 |
|---|---:|---:|
| guest 阶段墙钟 | 1,318.94 s | 637.58 s |
| 主要构建自报时间 | 21 分 57 秒 | 567.41 s |
| Linux 同口径墙钟 | 919.05 s | 454.22 s |
| Moss / Linux | **1.435×** | **1.404×** |

完整 profiling 宿主墙钟为 1,981.23 秒。`run.json` 状态为 `complete`，
CPU、wait、pending 和 CPU try-lock 丢样计数全部为 0。

采样口径为 CPU 10 Hz、off-CPU 1/8、ext4 1/16、page-cache 1/32、
`mprotect` 1/32。off-CPU 的 98.06/64.45 task-s 等待值是抽样区间乘 8
后的总体估算；它表示任务实际睡眠的区间类型，但不是逐次精确计数。

与修改前的 `mprotect` 样本相比，最新样本已经将：

- 预构建从 2,363.83 秒降到 1,318.94 秒；
- 正式阶段从 1,321.04 秒降到 637.58 秒；
- `mprotect` 累计 task-time 从 8,931.02/4,312.28 秒降到
  65.13/28.55 秒。

这证明 per-VMA 改造有效；本报告关注的是它之后暴露出来的新瓶颈。

## 第一大块：CPU 图中的 idle/run-queue

### 已记录样本

| 阶段 | CPU 样本 | idle 样本 | idle 占比 | 非 idle 样本 |
|---|---:|---:|---:|---:|
| 预构建 | 45,802 | 29,084 | **63.50%** | 16,718 |
| 正式阶段 | 25,848 | 18,582 | **71.89%** | 7,266 |

idle 样本几乎全部落在：

```text
task:idle
└─ axtask::run_queue::current_run_queue
```

对应图：

- [预构建 CPU 火焰图](./riscv-buildstorm-mprotect-per-vma-20260724/guest-cpu-prebuild.svg)
- [正式阶段 CPU 火焰图](./riscv-buildstorm-mprotect-per-vma-20260724/guest-cpu-timed.svg)

这里要特别区分“guest idle”和“宿主 QEMU CPU 使用率”。预构建期间
`run.json` 中 QEMU 平均使用 580.30% CPU、中位数 769.45%；正式阶段平均
516.18%、中位数 649.51%。这并不与 idle 图矛盾：Moss 的 idle 路径会在
run queue 周围继续执行，QEMU/TCG 会把这些指令统计成宿主 CPU 消耗。

所以：

- QEMU 接近 800% 只能证明 8 个 vCPU 都在被模拟；
- 它不能证明 8 个 vCPU 都在执行 rustc；
- 当前 CPU 图中最大的框确实是 guest idle/run-queue。

### 采样覆盖限制

Moss 报告的目标频率是每 CPU 10 Hz。如果 8 个 vCPU 在完整阶段都稳定收到
定时器采样，理论样本约为：

| 阶段 | 理论样本 | 实际样本 | 实际/理论 |
|---|---:|---:|---:|
| 预构建 | 105,515 | 45,802 | 43.41% |
| 正式阶段 | 51,006 | 25,848 | 50.68% |

`dropped_cpu=0`、`skipped_cpu=0`，因此少掉的不是哈希表满或 profiler
try-lock 失败。当前导出格式又没有保留每条记录来自哪个 CPU，无法判断是某些
hart 定时器采样不足，还是采样时机本身有偏差。

因此，本报告可以用 CPU 图判断“哪些路径宽”，但不再把样本数直接换算成
“等价活跃核心数”。下一次 profiler 应输出每 CPU 的 timer tick、accepted
sample、idle sample 和 user sample，先把这个测量缺口补上。

## 非 idle CPU 中谁最大

下面是对 folded 栈做的 inclusive 路径归类。只要一条栈经过该路径，就计入
该类；一条栈可能同时经过缺页、page-cache 和文件系统，因此各行不能相加。

| inclusive 路径 | 预构建：占非 idle | 正式：占非 idle | 说明 |
|---|---:|---:|---|
| 地址空间回收/unmap 家族 | **33.32%** | **33.22%** | `clear`、`unmap_area`、backend unmap、PTE 回收 |
| guard/关抢占/中断保存家族 | **32.23%** | **29.38%** | 多条路径共享的锁和 guard 基础成本 |
| 并发缺页家族 | **23.46%** | **22.93%** | prepare/execute/commit、COW、页表安装 |
| axfs/lwext4 文件系统家族 | **21.17%** | **25.71%** | page-cache、extent、读写、writeback |
| 页分配/清零/复制家族 | 8.82% | 7.35% | frame/folio 分配、`memset`、`memmove` |
| clone/clone_map 家族 | 5.13% | 5.90% | fork/clone 地址空间复制 |
| `mprotect` 家族 | **2.21%** | **2.12%** | 已不再是 CPU 主热点 |

地址空间回收家族进一步拆看：

| 路径 | 预构建：占非 idle | 正式：占非 idle |
|---|---:|---:|
| `reclaim_process_address_space` | 16.56% | 15.99% |
| `AddrSpace::drop` | 12.10% | 12.73% |
| `execve` 引发的旧地址空间 unmap | 11.92% | 12.54% |
| 显式 `munmap` | 4.21% | 4.06% |

这些子项会重叠。例如 `execve -> AddrSpace::drop -> unmap_area` 同时属于三行。
它们共同说明的是：**逐 VMA、逐 PTE 销毁已经成为当前最宽的有效内核路径之一**，
而不是四项可直接求和。

缺页家族中：

- 全部并发缺页路径占非 idle 的 23.46%/22.93%；
- `CowFaultPlan::execute` 占 8.46%/8.60%；
- 最终退回独占 `handle_page_fault` 的 fallback 只有 1.39%/1.10%。

这说明新的并发缺页协议确实覆盖了绝大多数 CPU 可见缺页；当前问题主要不是
fallback 频繁发生，而是缺页自身、提交和它与地址空间写操作的竞争仍然存在。

## 第二大块：off-CPU 的 futex 很宽，但不是首要内核证据

排除 GC、readahead 等长期后台任务后的 workload off-CPU 为：

| 等待原因 | Moss 预构建 | Moss 正式 | Linux 预构建 | Linux 正式 |
|---|---:|---:|---:|---:|
| futex | **66.02%** | **70.80%** | 62.55% | 69.50% |
| poll/epoll | 16.77% | 18.77% | 15.94% | 13.23% |
| pipe read | 7.96% | 7.30% | 18.96% | 16.55% |
| waitpid | 6.63% | 2.08% | 1.38% | 0.64% |

对应图：

- [预构建 workload off-CPU](./riscv-buildstorm-mprotect-per-vma-20260724/guest-offcpu-workload-prebuild.svg)
- [正式阶段 workload off-CPU](./riscv-buildstorm-mprotect-per-vma-20260724/guest-offcpu-workload-timed.svg)

Moss workload off-CPU 累计为 37,432.25/18,233.68 task-s；Linux 为
39,829.71/16,676.33 task-s。按各自墙钟归一后，平均同时睡眠的 workload
任务数是：

| 系统 | 预构建 | 正式阶段 |
|---|---:|---:|
| Moss | 28.38 | 28.60 |
| Linux | 43.34 | 36.71 |

Linux 更快，却有更多累计睡眠任务。这是因为构建系统会创建大量工作线程，
其中一部分在 futex、poll 和 pipe 上等待依赖或工作；其他线程仍可并行执行。
所以 off-CPU 总量越大不等于系统越慢。

真正需要从 off-CPU 中挑出来的是**内核造成的等待点**，而不是把 rustc 的
全部 futex 都归罪于 futex 实现。

## 当前最明显的内核等待：AddrSpace 读写锁

从 workload off-CPU folded 栈中按最终阻塞点重新聚合。以下均为 1/8
抽样后乘 8 的 task-time 估算：

| AddrSpace 等待 | 预构建 | 正式阶段 |
|---|---:|---:|
| 读锁 `lock_shared` | 88.25 task-s | 59.46 task-s |
| 写锁 `lock_exclusive` | 9.81 task-s | 4.99 task-s |
| 合计 | **98.06 task-s** | **64.45 task-s** |

等待来源：

| 等待者路径 | 预构建 | 正式阶段 |
|---|---:|---:|
| 缺页/内核代用户填页 | 54.84 s（55.93%） | 42.25 s（65.56%） |
| futex 用户地址访问 | 31.33 s（31.95%） | 16.20 s（25.14%） |
| `munmap` | 4.06 s（4.14%） | 1.25 s（1.94%） |
| `mprotect` | 2.94 s（3.00%） | 1.76 s（2.73%） |
| `mmap` | 2.24 s（2.29%） | 1.98 s（3.07%） |
| 其他用户内存访问 | 1.37 s（1.39%） | 0.74 s（1.14%） |
| 进程退出 | 0.71 s（0.73%） | 0.17 s（0.26%） |
| `madvise` 和其他 | 0.56 s（0.57%） | 0.10 s（0.16%） |

这里列的是**等待者**，还不知道当时是谁持有写锁。当前
`RawAddrSpaceRwLock` 在存在 waiting writer 时拒绝新 reader，因此即便
`mprotect` 自身已经很快，高频 writer 到达也可能给缺页和 futex 用户地址
检查制造 reader 停顿。

表中的 `mprotect` 等待来自通用 off-CPU 采样；后文 1.89/1.53 task-s
来自独立的 `mprotect` 1/32 专用事件。两套抽样相位和倍率不同，数值存在
正常方差，不应相减。

最重要的下一项测量不是再扩大 off-CPU 图，而是增加：

- 写锁 acquire 到 release 的持有时间；
- 按 `mmap/munmap/mprotect/madvise/clear/exec/fork` 区分 owner；
- writer 队列长度；
- reader 被 writer preference 阻塞的次数与时间；
- `clear/unmap` 中 VMA 数、PTE 数和 TLB flush 次数。

有了 owner-side 数据，才能决定是拆 `clear/unmap`，调整 writer preference，
还是将 fault commit 下沉为更细粒度的锁。

## 已有专用事件中谁最大

| 事件（task-s） | 预构建 | 正式阶段 |
|---|---:|---:|
| page-cache inclusive | **203.78** | 45.53 |
| AddrSpace 读写锁等待（off-CPU 重聚合） | 98.06 | **64.45** |
| `mprotect` total | 65.13 | 28.55 |
| ext4 inclusive | 38.11 | **44.89** |
| 同步块读写 | 8.21 | 2.74 |
| mutex wait | 6.68 | 1.42 |
| ext4 全局锁等待 | 6.52 | 1.23 |
| folio load wait | 3.78 | 0.41 |
| `mprotect` 专用 AddrSpace 等待 | 1.89 | 1.53 |

这些值存在嵌套，不能相加，也不能直接当作墙钟占比。例如 page-cache
inclusive 可能包含 ext4 和同步块 I/O；AddrSpace 等待可能落在
`mprotect-total` 内。

但它们足以排定量级：

- **预构建的已命名子系统大头是 page-cache inclusive；**
- **正式阶段最大的明确内核等待是 AddrSpace 锁；**
- ext4 inclusive 在正式阶段也明显，但 ext4 的全局锁等待很小；
- `mprotect` 已从数千 task-s 降到几十 task-s；
- folio 同页加载合并和同步块 I/O 都不是当前第一大头。

page-cache 预构建有 203.78 task-s，但同步块读写只有 8.21 task-s、
folio-load wait 只有 3.78 task-s。这意味着 page-cache 的多数可见成本不是
“等磁盘”，更可能在：

- cache 索引查找和更新；
- folio/frame 分配及清零；
- dirty 标记和 writeback bookkeeping；
- COW fault 对 page-cache 的引用和验证；
- 文件结束时的集中 `sync`。

正式阶段 CPU 图还出现 `task:sync -> crc32`，说明最终 writeback/校验也值得
单独计时，但当前事件没有把 page-cache CPU bookkeeping、ext4 CPU 和设备
等待完全拆开。

## 为什么现在不能说“调度器就是主因”

CPU 图的 idle 很宽，说明采样时经常没有有效任务在该 vCPU 上运行；但这可以
同时由以下原因造成：

- Cargo 依赖图和链接阶段本来就没有 8 个 runnable 编译任务；
- rustc/LLVM worker 正常在 futex 等待；
- AddrSpace 写锁阻塞缺页和用户地址访问；
- pipe/poll/waitpid 的生产者还未完成；
- scheduler 唤醒、迁移或 IPI 延迟。

现有图只能证明“有效运行和宿主 800% CPU 不是一回事”，不能单独证明
run queue 算法有错。Linux 的 futex/poll 结构与 Moss 很接近，也说明
“futex 很宽”本身不是证据。

要把调度器定为主因，下一次需要同时记录：

- 每 CPU runnable queue 长度；
- wake 到首次运行的延迟；
- 唤醒目标 CPU、实际运行 CPU 和迁移次数；
- idle 进入/退出时间；
- user、kernel、idle 的每 CPU 累计时间；
- timer tick 和 profile sample 的每 CPU 数量。

如果 `runnable > 0` 时 CPU 仍长期 idle，才是直接的调度器证据；如果
`runnable == 0`，应继续沿锁、依赖和 I/O 找生产者。

## 优化顺序

### P0：AddrSpace writer 持锁和地址空间销毁

理由：

- 地址空间回收/unmap 经过约 33% 的非 idle CPU 样本；
- AddrSpace 实际睡眠等待为 98.06/64.45 task-s；
- 等待者主要是 fault 和 futex 用户地址检查；
- Linux 全部 mutex+rwsem 实际睡眠锁等待只有 1.503/1.251 task-s。

先补 owner-side lock hold 探针。若 `clear/unmap` 是长持锁者，实现：

```text
锁内：从 MemorySet 摘除 VMA/PTE 所有权，发布新代数
锁外：逐页减引用、释放 folio/frame、回收页表
最后：合并并批量执行 TLB shootdown
```

这个方向同时对应 CPU 回收热点和 fault 锁等待，证据最完整。

### P1：page-cache CPU bookkeeping 与 sync/writeback

理由：

- 预构建 page-cache inclusive 为当前最大已命名子系统，203.78 task-s；
- 但同步块 I/O 和 folio-load wait 很小，不能继续只优化读盘；
- 正式阶段 ext4 inclusive 44.89 task-s，并出现 `sync/crc32` CPU 栈。

需要将现有一个 page-cache inclusive 事件拆为 lookup、miss owner、frame
allocation、fill、commit、dirty、writeback 和 eviction，再决定数据结构或
批量策略。

### P2：调度和唤醒延迟

先补 runnable/wakeup/idle 的直接指标。没有这些指标前，不根据 idle 图或
futex 宽度直接改调度策略。

### P3：ext4 全局锁

ext4 全局锁只剩 6.52/1.23 task-s，分别比 AddrSpace 锁小约 15 倍和 52 倍。
它仍可以继续细化到 per-inode/extent cache，但不应排在 AddrSpace 和
page-cache 之前。

### 已完成：`mprotect` 全局记录锁

旧的 seal/record 全局锁和全表扫描均为 0；`mprotect` 只占非 idle CPU
约 2.1%～2.2%。保留现有 per-VMA 实现和回归计数即可。

## 数据来源与复核

最新 Moss：

- [运行元数据](./riscv-buildstorm-mprotect-per-vma-20260724/run.json)
- [机器可读汇总](./riscv-buildstorm-mprotect-per-vma-20260724/summary.json)
- [原完整报告](./riscv-buildstorm-mprotect-per-vma-20260724/mprotect-per-vma-report.md)
- [串口日志](./riscv-buildstorm-mprotect-per-vma-20260724/console.log)

Linux 对照：

- [Linux 汇总](./riscv-linux-buildstorm-20260721/summary.json)
- [Linux 等价口径报告](./riscv-linux-buildstorm-20260721/optimization-report.md)

样本对应提交：`97196f4195e6`。

校验和：

```text
summary.json  923b65a06bcf85cbbd8206a4b46805596149634370d1e5cbf46592bbf8550f8f
run.json      c9600880405bf75621f3d763b1c7b116874084d9b185530108c063246c968840
```

本报告没有把 task-time 当墙钟，也没有把嵌套事件相加。CPU inclusive
路径只用于确定热点家族；off-CPU 总量只用于看等待结构；优化优先级主要依据
“CPU 路径、实际锁睡眠、专用事件和 Linux 对照”四项同时指向的位置。
