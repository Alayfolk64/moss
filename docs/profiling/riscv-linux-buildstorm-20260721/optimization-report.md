# Linux RISC-V BuildStorm 全流程性能报告

## 结论

这次 Linux 基线使用与 Moss 相同的 RISC-V QEMU、8 vCPU、8 GiB、`sdcard-rv.img` 和 `/glibc/buildstorm_testcode.sh`，完整覆盖 setup、446-crate 预构建和正式 BuildStorm。磁盘以 `-snapshot` 启动，基础镜像未被写入。

Linux 的 446-crate 预构建耗时 **15 分 33.60 秒**，正式阶段耗时 **7 分 31.93 秒**，两段合计 **23 分 05.53 秒**；正式构建结果成功，产物为 1,681,000 字节。对应的 Moss 数据是 76 分 28.19 秒和 42 分 04.89 秒。相同工作负载下，Linux 分别快 **4.91 倍**和 **5.59 倍**，两段合计快 **5.13 倍**。这证明 50 分钟限制并非 Cargo 依赖图或 RISC-V TCG 本身必然导致，当前主要差距仍然在 Moss 的内核实现。

CPU 与等待数据给出了直接原因：Linux 在预构建和正式阶段分别维持 **6.06 核**、**5.54 核**的活跃核等价值，而 Moss 只有 3.68 核、2.36 核。Linux 的内核 mutex+rwsem 慢路径累计等待仅为 **1.53 task-s**、**1.42 task-s**，热点主要是 `mmap_lock`；ext4 内部关联的锁竞争只有 **12.10 ms**、**0.583 ms**。Moss 对应的内核 mutex 等待是 11,166.94 task-s、8,172.23 task-s，其中 ext4 全局锁本身就有 981.46 task-s、218.92 task-s。

Linux 仍有约 24.23% 和 30.71% 的 idle CPU 样本，说明依赖 DAG、LTO 和链接长尾确实限制了完美的 8 核利用率；但它们只能解释 Linux 剩余的空闲，不能解释 Moss 与 Linux 之间约 5 倍的差距。

## 可直接查看的火焰图

整套流程：

- [全流程 CPU](./linux-cpu-all.svg)
- [全流程 workload off-CPU](./linux-offcpu-workload-all.svg)
- [全流程 mutex/rwsem 等待](./linux-offcpu-lock-all.svg)
- [全流程 ext4 内部锁等待](./linux-ext4-lock-wait-all.svg)
- [全流程 ext4、页缓存和块 I/O](./linux-fs-io-latency-all.svg)

预构建：

- [预构建 CPU](./linux-cpu-prebuild.svg)
- [预构建 workload off-CPU](./linux-offcpu-workload-prebuild.svg)
- [预构建 mutex/rwsem 等待](./linux-offcpu-lock-prebuild.svg)
- [预构建 ext4 内部锁等待](./linux-ext4-lock-wait-prebuild.svg)
- [预构建 ext4、页缓存和块 I/O](./linux-fs-io-latency-prebuild.svg)

正式任务：

- [正式任务 CPU](./linux-cpu-timed.svg)
- [正式任务 workload off-CPU](./linux-offcpu-workload-timed.svg)
- [正式任务 mutex/rwsem 等待](./linux-offcpu-lock-timed.svg)
- [正式任务 ext4 内部锁等待](./linux-ext4-lock-wait-timed.svg)
- [正式任务 ext4、页缓存和块 I/O](./linux-fs-io-latency-timed.svg)

`linux-offcpu-*.svg` 保留所有已完成睡眠，包括内核后台线程；分析时优先看排除了 `bpftrace`、`kthreadd`、kworker、RCU、jbd2 等后台任务的 `linux-offcpu-workload-*.svg`。setup 的同名 SVG 也已生成，便于检查工具链探测和 minibuild。

## 测量范围与完整性

运行环境如下：

| 项目 | 配置 |
|---|---|
| 虚拟机 | `qemu-system-riscv64 -machine virt -smp 8 -m 8G` |
| 内核 | Ubuntu `7.0.0-27-generic`, RISC-V 64 |
| 根文件系统 | 与 Moss 相同的 `sdcard-rv.img`，`-snapshot` |
| 工作负载 | `/glibc/buildstorm_testcode.sh` |
| Rust | `nightly-2026-05-28`, rustc/cargo 1.98 nightly |
| 采样工具 | bpftrace 0.23.2，guest 内运行 |
| CPU 采样 | 每个 vCPU 19 Hz，包含 idle |
| 事件追踪 | sched switch、mutex/rwsem 慢路径、filemap、ext4、bio 提交到完成 |

三个阶段分别启动独立的 BPF 程序，探针完全 attach 后才开始计时；停止计时后才导出聚合表。因此探针编译、attach 和 profile 文本导出不计入各阶段耗时。CPU 实际样本覆盖率为预构建 **99.77%**、正式阶段 **99.72%**，没有 QEMU 周期 tick 大量丢失的问题。

| 阶段 | Guest 单调时钟耗时 | 工作结果 |
|---|---:|---|
| setup | 5.58 s | 工具链与 minibuild 成功 |
| 446-crate 预构建 | 933.60 s（15:33.60） | Cargo 自报 15:32，完成 |
| 正式任务 | 451.93 s（07:31.93） | `ok=true`, Cargo 自报 07:19 |

QEMU 主机总墙钟为 1,422.157 秒（23:42.157），包含 Linux 启动、三次探针 attach/导出和关机。即使用这个更保守的端到端数字，距离 50 分钟仍有约 26 分钟余量。

追踪有三个边界需要明确：

1. CPU 图记录内核栈；在用户态执行的 rustc/LLVM 按线程名归类为 `[user-space]`，不会给出 rustc 内部函数符号。锁、文件系统和 off-CPU 图有完整内核热点路径。
2. 当前 Ubuntu 内核未启用 `CONFIG_LOCK_STAT`，所以锁图通过 `__mutex_lock_slowpath` 和 `rwsem_down_*_slowpath` 记录已发生的睡眠等待，不能覆盖纯自旋锁竞争，也不显示锁对象地址。
3. bpftrace 在高并发首次创建聚合键时报告了 8 次 `map_update_elem: EEXIST`（预构建 off-CPU 3 次、正式 ext4 5 次）。相对于数千个聚合栈影响可以忽略，但原始数据并非数学意义上的零损失。

所有等待值都是累计 task-time：多个线程并行睡眠会同时累计，数值可以大于墙钟；ext4、filemap 和 bio 路径还会嵌套，不能直接相加。

## 与 Moss 的时间对照

| 阶段 | Linux | Moss | Linux 加速比 |
|---|---:|---:|---:|
| setup | 5.58 s | 29.86 s | 5.35x |
| 446-crate 预构建 | 933.60 s | 4,588.19 s | 4.91x |
| 正式任务 | 451.93 s | 2,524.89 s | 5.59x |
| 预构建 + 正式任务 | 1,385.53 s | 7,113.08 s | 5.13x |

正式日志中的 `SMP=1` 是正在构建的 ArceOS 目标配置，不是 Cargo 的 job 数，也不是 Linux guest 的 CPU 数。guest 中 `nproc=8`，CPU 样本和宿主侧约 6–7 核的 QEMU 占用都证明并行编译已实际生效。

## CPU 并行度

| 阶段 | CPU 样本 | idle | 非 idle | 非 idle 占比 | 活跃核等价值 | Moss 活跃核等价值 |
|---|---:|---:|---:|---:|---:|---:|
| 预构建 | 141,575 | 34,304 | 107,271 | 75.77% | 6.06 核 | 3.68 核 |
| 正式任务 | 68,498 | 21,038 | 47,460 | 69.29% | 5.54 核 | 2.36 核 |

预构建的非 idle 样本中，rustc 占 54.76%，LLVM `opt` 占 35.26%，C/汇编工具链占 6.34%，链接器占 2.65%。正式阶段中 rustc 占 52.52%，LLVM `opt`/LTO 占 43.28%，链接器占 2.35%。bpftrace 自身只占全部 CPU 样本的 0.03% 和 0.02%，观测开销在 CPU 图中可以忽略。

Linux 并不是始终同时完成八个 crate。依赖图末端仍依次收敛到 `ostool -> axbuild -> tg-xtask`，正式阶段也有标准库、LTO 和链接的串行长尾。但前中段可以持续让 6 个左右的 vCPU 工作，已经足以在 50 分钟内完成整套任务。

## off-CPU 等待

排除长期内核后台线程后，Linux 的 workload off-CPU 累计为预构建 30,499.86 task-s、正式阶段 16,576.10 task-s：

| 已完成睡眠原因 | 预构建 | 正式任务 |
|---|---:|---:|
| futex | 66.35% | 70.86% |
| poll/epoll | 17.75% | 15.45% |
| pipe read | 12.30% | 12.83% |
| waitpid | 2.20% | 0.79% |
| nanosleep | 1.33% | 0% |
| 同步 I/O 等待 | 0.01% | 0.01% |
| 内核锁睡眠 | <0.01% | 0.01% |

最大的栈是：

```text
rustc -> futex_wait -> schedule
cargo -> ppoll -> schedule
rustc/tee/cargo -> anon_pipe_read -> schedule
```

大量 futex、poll 和 pipe task-time 主要来自 rustc worker、Cargo jobserver、ctrl-c/coordinator 线程以及依赖等待。它表示线程睡着的累计时间，不表示 Linux futex 实现很慢。关键区别是：这些协调线程睡眠的同时，Linux 仍保持 5.5–6.1 个核心执行有效编译；真正归入内核锁和同步 I/O 的 off-CPU 比例都只有约 0.01%。

## 内核锁热点

| 慢路径累计等待 | 预构建 | 正式任务 |
|---|---:|---:|
| mutex | 0.0086 s | 0.0125 s |
| rwsem | 1.5171 s | 1.4064 s |
| 合计 | **1.5257 s** | **1.4189 s** |
| 其中 ext4 内部路径 | **0.01210 s** | **0.000583 s** |

Linux 最大的锁热点不是 ext4，而是进程地址空间的 `mmap_lock`：

```text
handle_page_fault -> lock_mm_and_find_vma -> rwsem_down_read_slowpath
mprotect -> rwsem_down_write_slowpath
mmap/munmap -> rwsem_down_write_slowpath
```

预构建最大单栈是 Cargo 页错误进入 `lock_mm_and_find_vma`，占全部 mutex/rwsem 图的 28.94%；正式阶段同一路径占 17.60%。即便如此，全部地址空间和其他内核锁竞争合计只消耗约 1.5 task-s，不足以成为分钟级瓶颈。

ext4 锁图只保留慢路径探针与 ext4/jbd2 之间未跨越嵌套异常入口的栈，避免把 `ext4_readdir -> copy_to_user` 中发生的用户页错误误标成 ext4 锁。预构建的 12.10 ms 主要来自后台 writeback 的 `ext4_map_blocks -> rwsem_down_write_slowpath`；正式阶段只有 `ext4_page_mkwrite` 的 0.405 ms 和 `ext4_orphan_del` mutex 的 0.178 ms。

这与 Moss 的 `Ext4Filesystem::inner: Mutex<LwExt4Filesystem>` 有本质差别：Linux 使用页缓存、inode、extent、journal 等更细粒度机制，前台读路径没有被单个文件系统全局 mutex 包住。

## ext4、页缓存与块 I/O

| inclusive task-time | Linux 预构建 | Moss 预构建 | Linux 正式 | Moss 正式 |
|---|---:|---:|---:|---:|
| ext4 read/write 路径 | 66.69 s | 2,222.89 s | 23.91 s | 682.54 s |
| page cache/filemap | 34.64 s | 15,042.35 s | 15.08 s | 9,343.23 s |
| 块读取 bio 延迟 | 6.06 s | 393.08 s | 2.17 s | 175.67 s |
| 块写入 bio 延迟 | 92.45 s | 63.58 s | 31.08 s | 11.72 s |

两套内核的探针位置和页缓存实现不同，因此不能把比值当成严格微基准；数量级仍然有明确意义。Linux 的 ext4 inclusive 时间约低 29–33 倍，filemap 时间低 434–619 倍，块读取累计延迟低 65–81 倍。Linux 页错误不会在一个进程级地址空间 mutex 下继续持有 Moss 的文件级 page-cache mutex 和 ext4 全局 mutex。

Linux 的块写入累计 bio 延迟反而比 Moss 高，但它几乎全在 `jbd2/vda-8` 和 `kworker/u32:*`：预构建占 98.8%，正式阶段接近 100%。这是 delayed allocation、journal 和异步 writeback 在后台并发工作的结果，不代表前台 rustc 同步等待了 92 秒。对应的用户任务 off-CPU 同步 I/O 只有 4.32 task-s 和 1.85 task-s。

Linux ext4 图的前台最大使用者是链接器 `ld`，占预构建 ext4 时间 60.5%、正式阶段 72.4%；pagecache 则主要由 `ld` 与 rustc 构成。它们是实际数据处理，不是锁等待。正式阶段仍有 30.71% idle 的主要原因更接近标准库依赖链、LTO codegen 和链接长尾，而不是文件系统把并行任务串行化。

## 对 Moss 的优化结论

### P0：把缺页和 page-cache miss 的 I/O 移出大锁

Linux 基线进一步验证了 Moss 报告中的首要结论。Moss 的：

```text
handle_page_fault
  -> AddrSpace::handle_page_fault
  -> BackendOps::populate
  -> CowBackend::alloc_new_at
  -> CachedFile/ext4 同步读取
```

不能继续完整处在 `AddrSpace`、page-cache 和 ext4 大锁的覆盖范围内。应使用“锁内验证并登记、锁外分配/读取、锁内校验并提交”的两阶段 fault；page cache 按 inode+page 维护 in-flight 状态，同页合并、不同页并行。

### P0：将写回从前台临界区分离

Linux 的写 bio 主要由 jbd2/kworker 承担，说明较高的物理写入累计延迟并不必然拖慢编译。Moss 应把脏页 writeback、淘汰和 journal/元数据落盘移出 page-cache 全局锁及前台 syscall/fault 临界区；不能在 `with_page_or_insert_impl` 持锁时同步回写。

### P1：消除 ext4 全局 mutex

Linux ext4 内部慢锁只出现毫秒级等待，而 Moss 的单个 lwext4 全局 mutex 是数百 task-s。先通过 page-cache miss 合并减少调用次数，再把只读数据、目录/元数据和 journal 修改分成可控的细粒度并发域。直接删除锁不安全，但维持全文件系统唯一 mutex 也无法接近 Linux 基线。

### P1：用相同 profile 做验收

下一轮 Moss 优化的直接验收目标应是：

1. 预构建活跃核等价值从 3.68 提升到至少 5.5，正式阶段从 2.36 提升到至少 5.0。
2. COW/文件缺页相关 mutex wait 下降至少 90%。
3. ext4 全局锁等待从数百 task-s 降到个位 task-s，再逐步逼近 Linux 的细粒度锁水平。
4. 预构建先降到 20 分钟以内、正式阶段降到 10 分钟以内，使总流程稳定小于 50 分钟。

### P2：最后处理 Cargo/LTO 长尾与缓存

Linux 也无法让依赖 DAG 末端保持 8 核满载，所以在内核并发问题解决后，预置与源码、Cargo.lock、toolchain hash 匹配的 host 工具缓存仍然有价值。它属于消除固有串行长尾的第二阶段优化，不能替代缺页、页缓存和 ext4 锁修复。

## 数据与复现入口

- [机器可读摘要](./summary.json)
- 采样脚本：`scripts/linux_rv_profile.bt`
- guest 驱动：`scripts/linux_rv_profile_buildstorm.sh`
- SVG 渲染器：`scripts/render_linux_profile.py`
- 启动入口：`scripts/run_linux_rv.sh profile`

本目录保留本机原始 `setup/prebuild/timed.raw`、console 和 benchmark 日志，但由 `.gitignore` 排除；仓库只需要提交报告、机器摘要和 24 张最终 SVG。原始文件可以用上述渲染器重新生成相同图表。
