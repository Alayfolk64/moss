# Linux RISC-V BuildStorm 等价口径性能报告

## 结论

本轮重新运行了完整 BuildStorm，而不是复用旧 raw 数据。Linux guest 使用与 Moss profiler 相同的事件模型、begin/end 区间、采样频率和缩放规则：CPU 10 Hz，ext4 1/16，page-cache 1/32，off-CPU 1/8，锁等待和同步 I/O 全量记录。三个阶段都由独立 profiler 实例覆盖，探针 attach 和聚合表导出不计入阶段时间。

结果成功完成：446-crate 预构建耗时 **919.05 秒（15:19.05）**，正式阶段耗时 **454.22 秒（07:34.22）**，两段合计 **1,373.27 秒（22:53.27）**。QEMU 端到端墙钟为 1,417.728 秒（23:37.728），包含启动、三次 attach/导出和关机。正式产物为 1,681,000 字节，`status=0`。

相同工作负载下，Moss 的预构建和正式阶段分别为 4,588.19 秒和 2,524.89 秒；Linux 新基线分别快 **4.99 倍**和 **5.56 倍**，两段合计快 **5.18 倍**。Linux 的活跃核等价值为 6.20/5.48 核，Moss 为 3.68/2.36 核。

等价口径进一步确认首要瓶颈仍在 Moss 文件缺页链：

- Linux 实际睡眠的 mutex+rwsem 等待只有 1.503/1.251 task-s；Moss 是 11,166.94/8,172.23 task-s，相差约 7,430/6,535 倍。
- Linux page-cache inclusive 为 53.57/22.00 task-s；Moss 为 15,042.35/9,343.23 task-s，相差约 281/425 倍。
- Linux 前台同步 I/O 等待为 6.386/2.673 task-s；Moss 同步块读写为 456.66/187.39 task-s，相差约 71.5/70.1 倍。
- Linux ext4 read/write inclusive 为 70.05/22.49 task-s；Moss 为 2,222.89/682.54 task-s，相差约 31.7/30.3 倍。

因此，先拆掉 Moss 的 `AddrSpace -> CowBackend -> page-cache -> ext4 -> block` 跨层大临界区，仍然是有数据支持的第一优先级。

## 火焰图

| 指标 | 全流程 | setup | 预构建 | 正式阶段 |
|---|---|---|---|---|
| CPU | [all](./linux-cpu-all.svg) | [setup](./linux-cpu-setup.svg) | [prebuild](./linux-cpu-prebuild.svg) | [timed](./linux-cpu-timed.svg) |
| 全部 off-CPU | [all](./linux-offcpu-all.svg) | [setup](./linux-offcpu-setup.svg) | [prebuild](./linux-offcpu-prebuild.svg) | [timed](./linux-offcpu-timed.svg) |
| 工作负载 off-CPU | [all](./linux-offcpu-workload-all.svg) | [setup](./linux-offcpu-workload-setup.svg) | [prebuild](./linux-offcpu-workload-prebuild.svg) | [timed](./linux-offcpu-workload-timed.svg) |
| 实际睡眠锁等待 | [all](./linux-offcpu-lock-all.svg) | [setup](./linux-offcpu-lock-setup.svg) | [prebuild](./linux-offcpu-lock-prebuild.svg) | [timed](./linux-offcpu-lock-timed.svg) |
| ext4 内部锁等待 | [all](./linux-ext4-lock-wait-all.svg) | [setup](./linux-ext4-lock-wait-setup.svg) | [prebuild](./linux-ext4-lock-wait-prebuild.svg) | [timed](./linux-ext4-lock-wait-timed.svg) |
| ext4/page-cache/同步 I/O | [all](./linux-fs-io-latency-all.svg) | [setup](./linux-fs-io-latency-setup.svg) | [prebuild](./linux-fs-io-latency-prebuild.svg) | [timed](./linux-fs-io-latency-timed.svg) |
| 补充 bio 请求延迟 | [all](./linux-bio-latency-all.svg) | [setup](./linux-bio-latency-setup.svg) | [prebuild](./linux-bio-latency-prebuild.svg) | [timed](./linux-bio-latency-timed.svg) |

前六组 24 张图是 Moss 对齐的主结果；最后四张 bio 图用于观察 Linux 异步块请求，不进入 Moss 同步块 I/O 的直接倍数比较。

## 等价采样方法

### 事件定义

| 事件 | Moss 边界 | Linux 边界 | 采样 |
|---|---|---|---:|
| CPU | Guest profile timer 捕获当前内核/用户栈 | `profile:hz:10` | 10 Hz/CPU |
| off-CPU | `blocked_resched()` 前到任务恢复 | 阻塞态 `sched_switch` 切出到再次切入 | 1/8，时间乘 8 |
| mutex wait | `Mutex::lock()` 真正阻塞前到恢复 | mutex/rwsem slowpath 活跃期间真正切出的睡眠区间 | 全量 |
| page-cache | `with_page_or_insert_impl()` begin/end | `filemap_fault/filemap_read` begin/end | 1/32，时间乘 32 |
| ext4 inclusive | 获取 ext4 guard 前到 guard drop | `ext4_file_read_iter/ext4_buffered_write_iter` begin/end | 1/16，时间乘 16 |
| 同步 I/O | Moss 同步块驱动调用 begin/end | 前台 `io_schedule()` begin/end | 全量 |
| bio 延迟 | 无直接等价事件 | `submit_bio_noacct` 到 `bio_endio` | 全量、补充 |

旧 Linux 锁图用 slowpath 函数入口到返回，包含仍在 CPU 上运行的自旋和调度开销；本轮在 slowpath 活跃期间只累计真实阻塞的 `sched_switch` 区间，与 Moss `MutexWait` 的含义一致。

Linux 的 buffered I/O 在 page-cache 以下是异步的，不存在与 Moss 同构的同步块驱动 API。因此主图使用当前任务在 `io_schedule()` 中实际等待 I/O 的时间；bio 提交到完成独立绘图，不能与 Moss `BlockRead/BlockWrite` 相加或直接比较。

### 阶段和完整性

| 阶段 | Guest 单调时钟 | 工作结果 |
|---|---:|---|
| setup | 5.50 s | toolchain、minibuild 成功 |
| 446-crate 预构建 | 919.05 s | Cargo 自报 15:17，完成 |
| 正式任务 | 454.22 s | `ok=true`，Cargo 自报 7:22 |

CPU 理论样本覆盖率为预构建 **98.77%**、正式阶段 **98.64%**。三份 profile 全部成功 attach 22 个探针并完整导出。bpftrace 在高并发首次创建聚合键时出现 3 次 `map_update_elem: EEXIST`：预构建 rwsem 1 次，正式阶段 rwsem 和同步 I/O 各 1 次；相对于 886/1,341 个 rwsem 睡眠区间和 9,781/4,087 个同步 I/O 区间，不改变结论，但原始数据不是数学意义上的零损失。

所有等待值都是累计 task-time。多个任务同时睡眠时会同时累计，因此可以大于墙钟；ext4、page-cache 和同步 I/O 还可能嵌套，不能把三者相加当作总耗时。

## CPU 并行度

| 阶段 | CPU 样本 | idle | 非 idle | 非 idle 占比 | 活跃核等价值 | Moss |
|---|---:|---:|---:|---:|---:|---:|
| 预构建 | 72,619 | 16,298 | 56,321 | 77.56% | **6.20 核** | 3.68 核 |
| 正式阶段 | 35,843 | 11,272 | 24,571 | 68.55% | **5.48 核** | 2.36 核 |

预构建非 idle 样本中，rustc 占 54.95%，LLVM opt/LTO 占 35.54%，C/汇编工具链占 6.10%，链接器占 2.24%。正式阶段分别为 51.45%、44.12%、0.07% 和 1.97%。

这说明 Linux 并非每时每刻完成八个 crate，但前中段能持续维持 5.5～6.2 个 vCPU 做有效工作。末端仍然存在 `ostool -> axbuild -> tg-xtask` 以及标准库/LTO/链接串行长尾。

## off-CPU

为了与 Moss 一致，Linux off-CPU 同样采用每 CPU 的确定性 1/8 抽样，并将完成区间乘 8。排除 kworker、jbd2、RCU 等长期内核后台线程后：

| 原因 | 预构建 | 正式阶段 |
|---|---:|---:|
| futex | 62.55% | 69.50% |
| poll/epoll | 15.94% | 13.23% |
| pipe read | 18.96% | 16.55% |
| waitpid | 1.38% | 0.64% |
| nanosleep | 1.13% | 0% |
| 同步 I/O | 0.0087% | 0.0090% |
| 内核锁睡眠 | 0.0037% | 0.0064% |

1/8 抽样命中长生命周期 futex/poll 时会放大方差，所以 off-CPU 总 task-time 不适合与旧全量 Linux 结果混用；本表的价值是与同样采用 1/8 的 Moss 做同口径结构分析。关键事实不变：Linux 协调线程睡眠时，仍有约 5.5～6.2 个核心执行编译；真正归入内核锁和同步 I/O 的比例极低。

## 锁等待

| 实际睡眠锁等待 | 预构建 | 正式阶段 |
|---|---:|---:|
| mutex | 0.01055 s | 0.00121 s |
| rwsem | 1.49245 s | 1.24942 s |
| 合计 | **1.50300 s** | **1.25063 s** |
| 确认属于 ext4 内部 | 0 s | **0.01040 s** |

主要热点仍是 `mmap_lock`：

```text
handle_page_fault -> lock_mm_and_find_vma -> rwsem_down_read_slowpath
mmap/munmap/mprotect -> rwsem_down_write_slowpath
```

预构建最大的单栈是 Cargo fault 等待 `mmap_lock`，占锁等待图 35.34%。正式阶段存在约 10.40 ms 的真实 ext4 writeback rwsem 等待。

有一类栈表面同时出现 ext4 和 `mmap_lock`：`ext4_readdir -> copy_to_user -> user page fault -> mmap_lock`。它等待的是嵌套用户缺页的地址空间锁，不是 ext4 锁，报告按 slowpath 附近的实际锁路径将其排除。因此预构建的 ext4 内部锁值为 0，而不是把 12.92 ms 的嵌套 fault 错算为 ext4 竞争。

## page-cache、ext4 与同步 I/O

| inclusive task-time | Linux 预构建 | Moss 预构建 | Linux 正式 | Moss 正式 |
|---|---:|---:|---:|---:|
| ext4 read/write | 70.05 s | 2,222.89 s | 22.49 s | 682.54 s |
| page-cache/filemap | 53.57 s | 15,042.35 s | 22.00 s | 9,343.23 s |
| 前台同步 I/O 等待 | 6.386 s | 456.66 s | 2.673 s | 187.39 s |

Linux 预构建 page-cache 的主要使用者是 `ld` 44.82%、rustc 37.16%、cc1 9.41%；正式阶段是 rustc 48.24%、`ld` 45.38%。ext4 inclusive 预构建由 `ld` 占 56.65%、rustc 19.88%，正式阶段 `ld` 占 72.69%、rustc 21.35%。这些主要是实际文件处理，不是锁等待。

同步 I/O 的前台部分为预构建 4.994 task-s、正式阶段 2.167 task-s；剩余 1.392/0.506 task-s 属于 jbd2/kworker。即便将后台也计入，仍远低于 Moss 在缺页和前台文件操作中同步等待块设备的时间。

## 补充 bio 图

bio submit-to-completion 延迟为：

| 请求 | 预构建 | 正式阶段 |
|---|---:|---:|
| read | 4.503 s | 2.368 s |
| write | 102.713 s | 19.456 s |

预构建 bio 总延迟的 95.04%、正式阶段的 89.12% 由 jbd2/kworker 提交。它证明 Linux 把大部分写盘工作移到了后台，而不是证明前台同步等待了同样长的时间。主对照必须看 `io_schedule()` 前台等待图，不能再把异步 bio 数值直接与 Moss 同步驱动调用比较。

## 对 Moss 第一阶段修复的约束

本轮使用相同 profiler 方法后，优化顺序没有改变：

1. 为文件缺页增加 prepare/execute/commit 两阶段协议，让 frame 分配、page-cache miss 和 ext4 读取离开 `AddrSpace` mutex。
2. page-cache 使用稳定 page slot：同页 miss 合并，不同页 fill 并行，索引锁外完成 I/O。
3. 把 dirty eviction/writeback 从 fault 和前台 syscall 临界区移走。
4. 重新用同一组事件和采样率 A/B：文件 fault 相关 mutex wait 至少下降 80%，活跃核至少接近 Linux 的 5.5～6.2 核，同时 active core-s 不应上升。

在上述路径拆分前，单独增加调度迁移、扩大 readahead 或取消 snapshot 都没有证据能消除当前数量级差距。
