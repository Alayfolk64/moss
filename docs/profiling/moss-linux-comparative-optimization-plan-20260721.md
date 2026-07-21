# Moss 对比 Linux 的 BuildStorm 优化方案

本文基于同一台主机、同一份 `sdcard-rv.img`、相同的 RISC-V QEMU 配置、8 vCPU、8 GiB 内存和同一份 `/glibc/buildstorm_testcode.sh`，对照以下两套全流程 profile：

- [Moss 全流程报告](./riscv-buildstorm-guest-20260721/optimization-report.md)
- [Linux 全流程报告](./riscv-linux-buildstorm-20260721/optimization-report.md)

分析对应 Moss 提交 `8f4089991ecb093b759da052a80c6dc97b32656f`。本文不给没有验证路径的预估加速比；每项建议都必须能由现有数据解释，并附带可证伪的 A/B 验收条件。

## 结论

Moss 当前不是 Cargo 没有启用多核，也不是 QEMU `-snapshot` 天生把编译拖慢。主因是一条跨层串行链：

```text
用户文件缺页
  -> 进程 AddrSpace mutex
  -> MAP_PRIVATE 的 CowBackend 分配并清零匿名 frame
  -> 文件 page cache 的 cache_op + page_cache mutex
  -> ext4 全文件系统 mutex
  -> 单队列同步块读
  -> 把 page-cache 页复制到匿名 frame
```

它同时造成两种损失：

1. 多个本可并行的 rustc/LLVM 线程睡在锁上，Moss 只能利用 2.36～3.68 个核，而 Linux 能利用 5.48～6.20 个核。
2. Moss 为同一份只读文件数据同时创建 page-cache 页和匿名 COW 页，反复分配、清零、复制、淘汰；按 CPU 样本折算，预构建和正式阶段的有效 CPU 工作量分别约为 Linux 的 2.96 倍和 2.39 倍。

因此，正确的主线不是继续调 Cargo job 数或增加任务迁移，而是按下面顺序推进：

1. 先修复正式构建完成后的 pipe/FD EOF 挂起，并补齐精确的分层计数器；否则不能可靠取得端到端成绩。
2. 将文件缺页改为“锁内准备、锁外分配/读取、锁内校验提交”的两阶段 fault。
3. 将 page cache 改为带页级 `Loading/Ready/Writeback` 状态和稳定页引用的结构：同页 miss 合并，不同页并行，任何磁盘 I/O 都不在索引锁内执行。
4. 用全局内存预算替代当前每文件固定 1 MiB 的硬上限，并把脏页淘汰移交给有界后台 writeback；这一步负责减少额外工作量，而不只是减少锁等待。
5. 稳定页引用具备后，让只读 `MAP_PRIVATE` 首次缺页直接映射 page-cache frame，只在第一次写时复制，消除当前双份页和一次 4 KiB copy。
6. 在上层 miss 次数已经下降后，再把 lwext4 的普通文件数据读取拆成“锁内解析 extent、锁外读取数据”；元数据修改和 journal 暂时继续串行，不能把现有 `Mutex` 机械替换为 `RwLock`。
7. 最后再做异步块队列、预读和调度器微调。它们在当前 profile 中都不是第一瓶颈。

最终性能验收应使用无 profile 的原始评测流程，要求预构建和正式阶段合计不超过 45 分钟，并在 Cargo 成功后 5 秒内输出最终结果。45 分钟给 50 分钟限制保留了 10% 的环境抖动余量。

## 数据能证明什么

### 墙钟与 CPU 利用率

| 阶段 | Moss | Linux | Linux 相对加速 | Moss 活跃核 | Linux 活跃核 |
| --- | ---: | ---: | ---: | ---: | ---: |
| setup | 29.86 s | 5.50 s | 5.43x | - | - |
| 446-crate 预构建 | 4,588.19 s | 919.05 s | 4.99x | 3.68 | 6.20 |
| 正式构建 | 2,524.89 s | 454.22 s | 5.56x | 2.36 | 5.48 |
| 预构建 + 正式构建 | 7,113.08 s | 1,373.27 s | 5.18x | - | - |

Linux 并没有一直占满 8 核：依赖 DAG、LTO 和链接长尾仍使其保留 22%～31% idle 样本。因此目标不是不现实的“永远 8 核满载”，而是先达到 Linux 已经证明可行的 5.5～6.2 个活跃核。

这次 Linux 重测按 Moss 的同一事件模型和采样策略采集：CPU 10 Hz，off-CPU 1/8，ext4 1/16，page cache 1/32；锁等待只统计真正睡眠区间，块 I/O 以调用者同步等待为主口径。Linux 和 Moss 的实现函数不同，所以 page-cache/ext4 探针只能选择语义等价的边界，不能把 inclusive 比值解释为单个函数的微基准倍率。5.18x 是相同 workload、虚拟硬件和采样口径下的完整阶段墙钟比值。等待值是累计 task-time，可以大于墙钟，嵌套路径也不能相加；最终过线仍需用关闭 profile 的同脚本复测。

### 锁、页缓存和 I/O

| 累计 task-time | Moss 预构建 | Linux 预构建 | Moss 正式 | Linux 正式 |
| --- | ---: | ---: | ---: | ---: |
| 内核 mutex/rwsem 慢等待 | 11,166.94 s | 1.503 s | 8,172.23 s | 1.251 s |
| ext4 全局/内部慢锁 | 981.46 s | 0 s | 218.92 s | 0.01040 s |
| ext4 read/write inclusive | 2,222.89 s | 70.05 s | 682.54 s | 22.49 s |
| page-cache/filemap inclusive | 15,042.35 s | 53.57 s | 9,343.23 s | 22.00 s |
| 前台同步块 I/O 等待 | 456.66 s | 6.386 s | 187.39 s | 2.673 s |

Linux 和 Moss 现在使用相同的事件含义、区间边界和采样率；不同内核只能落在各自语义等价的实现函数上。inclusive 比值不能解释为某个函数精确慢了多少倍，但以下因果关系有多份独立证据互相印证：

- Moss 的 mutex 等待中，80.25% 和 74.00% 的栈经过 `handle_page_fault -> CowBackend::alloc_new_at`。
- Moss 的 ext4 精确全局锁等待中，文件映射缺页分别占最大单一路径的 55.07% 和 64.80%。
- page-cache 组合图中，`CowBackend::alloc_new_at -> CachedFile::with_page_or_insert_impl` 占 64.69% 和 71.85%。
- 同步块读写只占 Moss page-cache inclusive 时间约 3.04% 和 2.01%；这说明磁盘请求存在，但上层锁的覆盖时间和缓存组织方式才是首先要消除的放大器。
- Linux 的 ext4 写 bio 累计时间并不低，但预构建和正式阶段分别有 95.04% 和 89.12% 的 bio 延迟发生在 workload 以外的后台任务；前台 workload 的同步 I/O 等待只有 4.994 和 2.167 task-s。关键不是“完全不写盘”，而是不让前台 fault/syscall 带着大锁同步完成所有写回。bio 图仅作为设备完成延迟的补充证据，不进入主对比表。

对应火焰图：

- [Moss 文件系统与 I/O](./riscv-buildstorm-guest-20260721/guest-fs-io-latency-all.svg)
- [Moss mutex 等待](./riscv-buildstorm-guest-20260721/guest-offcpu-lock-all.svg)
- [Linux 文件系统与 I/O](./riscv-linux-buildstorm-20260721/linux-fs-io-latency-all.svg)
- [Linux mutex/rwsem 等待](./riscv-linux-buildstorm-20260721/linux-offcpu-lock-all.svg)

### 只提高并行度不够

用“阶段墙钟 × 活跃核等价值”近似计算有效 CPU 工作量：

| 阶段 | Moss active core-s | Linux active core-s | Moss/Linux |
| --- | ---: | ---: | ---: |
| 预构建 | 16,884.54 | 5,702.29 | 2.96x |
| 正式构建 | 5,958.74 | 2,491.01 | 2.39x |

这是采样估算，不是硬件性能计数器，但足以反证“只要调度到 6 核就能过线”：假定 Moss 工作量完全不变，只把并行度提高到 Linux 的 6.20/5.48 核，两阶段仍约需 45.36 + 18.11 = **63.46 分钟**。

反过来，若只把有效 CPU 工作量降到 Linux 水平、并行度仍保持 Moss 当前水平，理论值约为 **43.42 分钟**，虽然可能勉强过线，但余量很小。这说明最终必须同时降低等待和额外工作量，不能用单一指标宣布成功。

CPU 图也支持“额外工作量”这一判断。预构建中，单个 rustc 的 `alloc_new_frame`、`PageCache::new`、`PageCache::drop` 和 lwext4 `memset` 代表栈分别占该阶段样本约 5.34%、4.17%、3.94% 和 3.76%。前三个直接位于文件 COW fault；`memset` 的栈展开较短，至少能证明 lwext4/buffer 清零本身也是显著 CPU 消耗，不能武断地全部归到同一种页清零。正式阶段同类代表栈仍然明显存在。

## 已排除的错误方向

### 不是 Cargo 单任务

Linux 使用相同脚本和依赖图，`nproc=8`，能维持约 6 核活跃。日志中目标工程的 `SMP=1` 是被编译 ArceOS 的配置，不是 Cargo job 数。Moss 日志也同时存在 rustc、多个 codegen/LLVM 和 C 编译进程；问题是它们大量进入内核等待，而不是没有创建。

### 不是先继续改调度器

当前调度器已经具有按负载放置、CPU sticky wakeup、安全点 rebalance 和 idle IPI 合并。此前调度日志经常显示 `ready=0/1`，profile 又把主要睡眠定位到文件 fault 的 mutex。调度器只能运行 ready task，不能让睡在 page-cache/ext4 锁上的任务变 runnable。此时增加迁移只会增加 IPI、缓存失效和迁移栈开销。

### 不是先改 virtio 多队列

当前动态块驱动确实只有一个 `Mutex<CmdQueue>`，并调用 `read_blocks_blocking`/`write_blocks_blocking`。但同步块读写只占 page-cache inclusive 的很小部分。若 page-cache miss 仍持有 `AddrSpace`、文件缓存和 ext4 大锁，增加设备队列并不会释放这些上层串行区。多队列应在上层能提交多个独立请求之后再测。

### 不能只改 ext4 全局锁

ext4 全局锁是真实热点，但它只解释整条嵌套链的一部分。Moss 更大的 mutex 等待和 page-cache 时间发生在 `CowBackend`/cache 路径。只改 ext4 后，同一进程 fault 仍被 `AddrSpace` 串行，同一 inode 的不同页仍被 `cache_op` 串行，页的双重分配与复制也仍存在。

### 不能把 ext4 `Mutex` 直接换成 `RwLock`

`third_party/lwext4_rust/src/fs.rs` 中的 `Ext4Filesystem` 持有一个可变的 C `ext4_fs` 和一个共享 `Ext4BlockDevice`/bcache。读取 inode、extent 和数据的接口全部需要 `&mut self`，底层缓存、引用计数和 journal 状态并非只读。直接允许多个“reader”同时进入会制造数据竞争和文件系统损坏。

### 不是取消 snapshot

Linux 基线也用同一 raw rootfs 的一次性 snapshot，仍在 23 分钟级完成。snapshot 可能改变绝对 I/O 延迟，但不能解释 Moss/Linux 的数量级差距；取消它还会污染测试镜像，降低 A/B 的可复现性。

## 当前代码中的串行链

### 1. 地址空间锁跨越整个 fault

- `kernel/src/task/mod.rs`：进程持有 `RwLock<Arc<Mutex<AddrSpace>>>`。
- `kernel/src/task/user.rs` 和 `kernel/src/mm/access.rs`：用户 fault 和内核访问用户内存触发的 fault 都先锁住整个 `AddrSpace`。
- `kernel/src/mm/aspace/mod.rs`：`handle_page_fault()` 在该锁下调用 backend `populate()`，并把 `PageTableMut` 传入后端。
- `kernel/src/mm/aspace/backend/cow.rs`：`alloc_new_at()` 在锁内分配、清零 frame，再调用 `file.read_at()`，最后才安装 PTE。

这意味着同一 rustc 进程内多个线程即使 fault 到不同 VMA、不同页，也必须排队等待最慢的文件读取结束。

### 2. page cache 在锁内做 miss fill 和淘汰

`vendor/axfs/src/highlevel/file.rs` 当前每个 inode 有：

```text
page_cache: Mutex<LruCache<u32, PageCache>>
cache_op:   Mutex<()>
```

`with_page_or_insert_impl()` 同时持有二者，然后执行页分配、清零、ext4 读取、回调，以及容量满时的淘汰。淘汰到 dirty 页时，普通 LRU 路径仍可能同步单页写回并提交长度。全量 `sync/drop` 已经能在 page-cache 索引锁外批量写回，这一点应该保留；问题是 `cache_op` 仍跨越 I/O，且日常 miss/单页淘汰仍在前台串行。

当前普通文件缓存硬限制为每 inode 256 页，即 1 MiB。这个值是针对早期 iozone/lmbench 小工作集调出的，不适合 rustc、rlib、链接器和工具链大文件。它会让大文件在 8 GiB guest 中仍频繁分配/销毁缓存页，且 dirty 大文件超过 1 MiB 后进入前台单页淘汰。不能简单把常量无限放大，因为最多保留 256 个 inactive file record，固定 per-file 上限会转化为不可控的最坏内存占用；正确方向是全局预算和按压力回收。

### 3. `MAP_PRIVATE` 为同一数据保留两份页

私有文件映射走 `CowBackend`。缺页时它先创建并清零一个匿名 frame，然后 `CachedFile::read_at()` 又创建/读取 page-cache 页，再把数据复制到匿名 frame。对编译器、动态库和链接器的大量只读映射，这比 Linux “先只读映射 page-cache 页，写 fault 时再 COW”多一次 frame、一次清零和一次复制。

现有 `PageCache` 只在 LRU 内以值对象存在，离开锁后没有稳定引用；淘汰依靠回调反向解除 shared file PTE。这也是当前不能直接把 page-cache frame 交给私有映射长期使用的原因。必须先解决页生命周期和 pin，再消除复制。

### 4. ext4 和块设备继续放大上层串行

`vendor/axfs/src/fs/ext4/fs.rs` 用单个 `Mutex<LwExt4Filesystem>` 包住全部 inode、目录、数据、元数据和 flush 操作。`vendor/axfs/src/fs/ext4/inode.rs` 的 `read_at()`/`write_at()` 在该锁内完成整个 lwext4 操作和同步块 I/O。

块驱动的动态实现又用单个 `Mutex<CmdQueue>` 包住阻塞请求。因此当前一次冷 miss 会从进程锁一直串到设备完成。不过数据表明应从上向下拆：先让 fault/cache 不跨 I/O 持锁，再判断 ext4 和设备还剩多少瓶颈。

## 分阶段实现方案

## P0-A：先让测量结果可闭环

### 修复构建成功后的 EOF 挂起

Moss 日志已经出现 `Finished release ... in 41m 25s`，但没有继续输出 `BUILDSTORM_COMPILE` 和 group end；`tee` 仍睡在 `Pipe::read`。Linux 在同脚本下能正常结束。性能改动在这个问题解决前不能算端到端有效。

不应仅凭一次栈猜某一个 FD 泄漏。先在复现时导出进程树、每个 FD table 的共享身份、pipe 对象地址、读写端计数和每端持有者，再修复确认的生命周期错误。当前有两个需要一并补齐的确定性缺口：

1. `Pipe::closed()` 只看 `Arc<Shared>` 总引用数。匿名 pipe 恰好只有两个 endpoint 对象时通常可用，但 FIFO 可以有多个同侧 endpoint，此时无法判断“写端为 0”。应在 `Shared` 中显式维护 readers/writers，并在最后一个对端 drop 时唤醒 poll/read/write 等待者。计数对象应是 open-file-description/`Pipe` endpoint：新建 endpoint 时加一，最后一个 `Arc<Pipe>` drop 时减一；`dup` 和普通 fork 只共享同一个 `Arc<Pipe>`，不能重复增加 endpoint 计数。
2. `sys_unshare(CLONE_FILES)` 当前成功返回但不复制 FD table；exec 也需要在共享 FD table 时先执行 Linux 语义的 unshare，再处理 `CLOEXEC`，否则可能影响其他共享者。`close_range(UNSHARE)` 已有复制并替换 scope-local FD table 的代码，可以抽成一个经过测试的公共 helper，避免再实现一套不同逻辑。

回归测试至少覆盖：`producer | tee file`、嵌套 shell pipeline、fork 后多 writer、`CLONE_FILES + exec`、`exit_group`、FIFO 的多个 reader/无 writer。所有 writer 退出后 reader 必须立即读到 EOF，正式 BuildStorm 成功后 5 秒内必须输出最终结果。

### 增加可归因计数器

现有 generic mutex 栈能证明 fault 链很热，但不能稳定区分等待的是 AddrSpace、page-cache 索引还是其他动态 mutex。下一轮 profile 应新增仅在 `profile` feature 下启用的事件：

- `AddrSpaceWait` 和 `AddrSpaceHold`；
- `PageIndexWait/Hold`、`PageLoadWait`、`PageWritebackWait`；
- fault prepare/commit/retry/loser-frame 计数和分段耗时；
- page hit/miss/coalesced miss、clean/dirty eviction、resident/dirty/peak pages；
- ext4 lock wait/hold、实际进入 lwext4 的 read/write 次数和字节数；
- writeback 队列长度、合并页数、前台被迫回写次数。

这些计数器用于证明后续变化发生在预期层级。若一项改动只是把热点从一个未命名 mutex 移到另一个 mutex，墙钟又不降，应立即回退。

## P0-B：两阶段文件 fault

新增统一的 `FaultPlan`/`PreparedFault` 接口，替换 backend 直接持有 `PageTableMut` 完成所有工作的模式。

### prepare：短暂持有 AddrSpace 锁

1. 对 fault 地址对齐，查找 VMA 并校验权限。
2. 记录稳定的 VMA identity/generation、backend `Arc`、期望 PTE 状态、映射 flags 和文件偏移。
3. 对已有 COW 页先取得物理页 pin，防止锁外 copy 时被回收。
4. 释放 `PageTableMut` 和 AddrSpace 锁。任何页表修改器都不能跨越这一步存活。

### execute：锁外慢工作

- 未映射的匿名页：分配/清零 frame。
- 未映射的文件私有页：分配目标 frame，并完成 page-cache/ext4 读取。
- COW 写 fault：复制已 pin 的旧 frame。

### commit：重新持锁并校验

1. 重新查找 VMA，校验 identity/generation、权限和 backend 未变化。
2. 重新查询 PTE。若其他 CPU 已经安装满足条件的映射，释放本地 frame，计为 loser/reuse，并返回成功。
3. 若 `munmap/mprotect/exec` 已改变 VMA，释放本地资源并按当前状态重试或返回 fault，不能安装过期页。
4. 只有状态仍匹配时才 map/remap，完成页表更新和必要的本地/远端 TLB 处理。

所有会改变 VMA 结构或权限的路径都要递增 generation。内核 copy_to/from_user 触发的 fault 必须复用同一协议，不能保留另一条仍跨 I/O 持锁的路径。

不要把 AddrSpace `Mutex` 直接改成 `RwLock`：page fault 会修改页表、COW 引用和 backend 状态，多个 reader 同时 populate 并不安全。两阶段协议通过 commit 时的重新验证提供并发，而不是把写操作伪装成读操作。

### P0-B 验收

- 8 线程同进程、不同页的 `MAP_PRIVATE` fault 吞吐相对 1 线程至少有明确扩展，不能仍接近 1x。
- fault 相关 mutex wait 比当前 11,166.93/8,172.23 task-s 至少下降 80%。
- `munmap/mprotect/truncate` 与 fault 并发压力测试无错误映射、use-after-free 和 frame 泄漏。
- loser frame 数和释放数一致；VMA generation retry 不出现无界活锁。
- 全流程活跃核提升但 active core-s 不应明显增加；若只增加 CPU 消耗而墙钟不降，不通过。

## P0-C：page slot、稳定页引用和锁外 fill

page cache 应从“锁内持有 `PageCache` 值对象”改成“索引指向稳定的页槽”。建议的最小状态机：

```text
Vacant
  -> Loading(owner, generation)
  -> Ready(PageRef, clean/dirty, generation)
  -> Writeback(PageRef, sequence)
  -> Ready / Failed
```

实现原则：

1. 索引按“稳定 `CachedFileRecord`/inode incarnation + page_no”分片；锁只保护查找和状态转换。不能只依赖可复用的裸 inode 号，unlink 后的新 inode 绝不能命中旧文件页。
2. miss 的第一个任务插入 `Loading` 后立即释放索引锁，在锁外分配和读盘；同页其他任务等待该 slot，不再重复读盘。
3. 不同页的 `Loading` 可以并行，不能再由每 inode 的 `cache_op` 串行。
4. 完成后重新锁 slot 发布 `Ready` 并唤醒等待者；失败时发布错误/移除 slot，保证可重试。
5. 调用者取得 `PageRef` 后立即释放索引锁。数据复制、PTE map、listener 通知、writeback 和内存释放都不能在索引锁内执行。
6. dirty、writeback 和 invalidate 使用页级状态/序列；truncate 递增 inode data generation，使旧 `Loading` 结果无法重新发布。

`PageRef` 必须真正拥有 frame 生命周期。shared mmap 应在 backend 的 mapped-page 表中保留 pin，unmap 时 drop；不能只把物理地址写进 PTE 后依赖 LRU 中的值对象存活。这样可以逐步移除当前复杂的 eviction listener/try_lock 反向解映射路径。

第一步可以暂时保留现有 1 MiB 容量，仅验证“同页合并、不同页并行、I/O 不持索引锁”。容量策略放到独立提交，避免一次改动同时改变并发与命中率，无法判断收益来源。

### P0-C 验收

- 8 进程读取同 inode 同一冷页只产生一次底层 fill；`coalesced` 计数为 7。
- 8 进程读取同 inode 的 8 个不同冷页能并行，不再由 `cache_op` 串成 8 次。
- page-cache 索引锁持有时间不得包含 ext4/block 事件。
- fill 失败、任务取消、truncate 和 invalidate 都会唤醒等待者，无永久 `Loading`。
- PageCache 分配/销毁代表栈显著下降；若 mutex wait 下降但 miss/eviction 数不降，继续进入下一阶段，不能提前宣布完成。

## P0-D：全局预算与后台 writeback

当前每 inode 1 MiB 硬上限应改为全局内存预算下的软公平策略：

- 以全 guest 可用内存比例和绝对上限共同确定 high/low watermark，而不是“文件数 × 固定页数”。
- 保留 per-inode 最近使用信息，避免单个大文件驱逐全部小热文件，但允许热点大文件超过 1 MiB。
- clean 且未 pin 的页可直接淘汰；dirty 页只从可见索引中隔离并排入有界 writeback，不在触发 fault/read/write 的前台线程中单页同步落盘。
- writeback 按 inode 和连续页合并，保留现有 64 KiB batch 的正确逻辑，再用数据决定是否增大 batch。
- 达到 dirty high watermark 时允许前台节流，但节流应等待后台进度，不能重新拿着 page-cache/ext4 大锁自己逐页写。

预算数值不能拍脑袋固定。先用计数器记录 BuildStorm 的进程 RSS、cache resident/dirty 峰值和 eviction reuse distance，再对 256 MiB、512 MiB、1 GiB 三个受控预算做相同 snapshot 的短测和一次完整预构建。只有命中率、PageCache drop 栈和墙钟同时改善，才采用更高预算。

fsync/fdatasync 的语义通过 writeback sequence 保证：调用时截取目标序列，等待此前脏页完成，再执行必要的文件系统 flush；后台错误必须记录并返回。普通 close 不应隐式等价于 fsync。`O_SYNC/O_DSYNC`、msync、truncate、unlink 和掉电一致性测试必须作为合入门槛。

### P0-D 验收

- 前台因 LRU 满而执行的单页 dirty writeback 降到接近 0。
- 大于 1 MiB 的 rustc/rlib/目标文件不再周期性抖动；miss、clean/dirty eviction 和 page alloc/free 数量明显下降。
- active core-s 朝 Linux 收敛，而不仅是活跃核数上升。
- 8 GiB 配置下无 OOM；小内存配置能通过 watermark 回收，不依赖 BuildStorm 专用常量。

### 可先做、但不能当最终架构的容量实验

完整 cache manager 开发量较大，可以先用两个严格受控的实验判断缓存抖动能贡献多少收益：

1. 将 per-file cache 从 256 页提高到 1,024/2,048 页时，同步把 inactive record 上限从 256 降到 64/32，使理论 inactive data 上限仍约为 256 MiB；分别记录 hit/miss、eviction、page alloc/free、RSS 和墙钟。它只用于验证“大文件被 1 MiB 硬上限反复驱逐”这一假设，不能作为长期内存管理方案。
2. 当前 lwext4 bcache 为 4,096 个块，4 KiB 块时约 16 MiB。可对 16/64/128 MiB 做 A/B；只有底层 block-read 次数和墙钟同时下降、且 ext4 lock hold/RSS 没有不可接受的增长，才暂时采用更大值。

这两个实验必须独立提交、独立回退。若只增加 RSS 而未降低 active core-s 或墙钟，就说明热点不是容量，应停止继续放大。

## P1-A：只读 `MAP_PRIVATE` 直接映射 page-cache frame

完成稳定 `PageRef` 后，`CowBackend` 的文件私有映射可按 Linux 方式处理：

1. 首次 read/exec fault 取得 page-cache `PageRef`，以去掉 WRITE 的权限直接 map。
2. backend 保存该页的 pin 和“file-backed clean”状态。
3. 第一次 write fault 才分配匿名 frame、复制 4 KiB、切换 PTE，并释放 page-cache pin。
4. fork 后的匿名页继续使用现有 COW frame refcount；未写的文件页可以继续共享。

这会直接删除当前 `alloc_new_frame(true) -> CachedFile page -> copy` 的双份页路径，预期同时降低 frame 分配、清零、page-cache drop 和内存带宽。它不能在 P0-C 之前做，因为当前 LRU 淘汰会直接释放 `PageCache` frame，PTE 没有独立所有权。

第一版可以让映射持有 pin 到 unmap，但所有 pinned file pages 仍必须计入全局 cache/memory budget。若长生命周期 mmap 使 pinned 峰值过高，后续要增加安全的 reverse-map reclaim：先撤销 PTE、完成 TLB shootdown，再释放最后一个 `PageRef`；绝不能为了回收直接释放仍在 PTE 中的物理页。

正确性重点是 EOF/truncate 后 SIGBUS、文件写入与私有映射可见性、mprotect、fork、exec 映射权限和 W^X。至少运行并发 truncate+mmap、fork+COW、动态链接器启动和完整 BuildStorm。

## P1-B：拆出 lwext4 普通文件数据读

上层优化后重新 profile。若 ext4 全局锁仍占主要等待，建议先拆普通文件 data read，不要一次重写全部 lwext4 锁模型：

1. 在 ext4 mutex 内取得 inode pin，解析逻辑页到物理 extent/block 列表，记录 inode extent generation。
2. 生成的 read plan 必须区分 hole/zero、可从已 pin 的 bcache buffer 复制的数据，以及允许从设备读取的 clean extent。不能绕过 lwext4 bcache 中尚未落盘的 dirty/up-to-date block，否则会读到旧磁盘数据。
3. 释放 ext4 mutex，在锁外完成已规划的数据复制或共享块设备读取。
4. 对与 truncate/hole-punch/extent 修改并发的请求，在提交前校验 generation；发生变化则丢弃结果并重试。修改方必须先发布新 generation，再释放或复用旧物理块，避免过期读取被提交。
5. 写入、分配块、目录修改、rename/unlink、journal 和 metadata flush 先继续使用全局修改锁。
6. 每个改变 extent 或文件长度的操作递增对应 inode generation，并与在途 data-read pin 协调。

这条路径需要把 `Ext4Disk` 从 lwext4 独占的 `&mut` 设备中抽出为安全共享的请求接口。第一版即使仍由设备队列串行，也能把 extent 解析和设备等待从全文件系统临界区分离；待出现多个并发请求后，再决定增加 virtio queue depth 或多队列。

“建立多个彼此独立的只读 lwext4 实例”只能作为实验，不应直接上线：它们各自的 bcache 与写实例之间存在 inode、extent 和 journal 一致性问题。若采用，必须有全局写屏障、epoch 失效和全部 reader drain，复杂度不低于上述 read-plan 方案。

### P1-B 验收

- 精确 ext4 全局锁等待相对当前至少下降 90%，且普通 data read 的设备等待不再出现在锁持有区。
- 同一文件系统的不同 inode 冷读能并行；metadata 修改仍保持正确顺序。
- 并发读写、truncate、rename、unlink、fsync、崩溃恢复和镜像 fsck 全部通过。
- 如果上层 P0 已使 ext4 wait 降为非主要热点，则暂停此高风险改造，转而优化新的实测第一热点。

## P1-C：设备并发和预读

只有在 P0/P1-B 后 profile 证明块层上升为主要等待时才实施：

- 将 `read_blocks_blocking`/`write_blocks_blocking` 改成 request submit + completion，允许多个在途请求。
- 若 virtio 设备协商到多个队列，再按 CPU/NUMA 选择队列；否则单队列也可用多个 descriptor 提高 queue depth。
- 相邻冷页由 cache 层合并，避免每页一个 virtio transaction。
- 当前 shared-file fault 的 4 页 readahead 是同步执行，而且仍处在 AddrSpace fault 调用内。应改成 fault 提交后由低优先级 worker best-effort 预读；在大锁未拆之前增加预读窗口只会延长临界区。

验收必须看前台 wall time 和队列深度，不能用“发出的 I/O 更多”当成功。随机负载下若预读命中率低、驱逐热点页或增加 ext4 wait，应自动收缩窗口。

## P2：调度器只做复测后的微调

P0 完成后重新观察每 CPU ready depth、idle-with-global-runnable、迁移、IPI 和 cache miss。只有出现“某些 CPU ready 堆积而其他 CPU idle”的证据，才继续改负载均衡。

当前每次返回用户态都会调用 `rebalance_current()` 并扫描 CPU load；它没有出现在本轮主要 CPU 栈中，因此不是当前 P0。若后续锁拆分使 syscall/fault 频率成为热点，可以用负载 epoch/imbalance flag 限制无效扫描，而不是提高迁移频率。任务 wake 后保持原 CPU 对内核上下文和缓存局部性仍是合理默认。

## 实验与合入门槛

### 每个提交只验证一个机制

建议保持以下提交边界：

1. `pipe/fd correctness + diagnostics`
2. `fault prepare/execute/commit`
3. `page slot + lock-free I/O interval`
4. `global cache budget + writeback worker`
5. `MAP_PRIVATE page-cache mapping`
6. `ext4 read plan`
7. `block request concurrency/readahead`

不要把这些修改压成一次大提交。否则即使总时间变快，也无法判断哪一项有效、哪一项引入了潜在错误。

### 固定的测试矩阵

每次从同一只读基础镜像和 snapshot 开始，固定 QEMU、CPU、内存、Rust toolchain、源码和 Cargo.lock。先跑以下短测三次，记录中位数、最差值和 profile 计数：

| 测试 | 隔离的机制 |
| --- | --- |
| 1/2/4/8 线程同进程 fault 不同私有文件页 | AddrSpace 两阶段 fault |
| 1/2/4/8 进程 fault 同 inode 的同页/不同页 | page-slot 合并与页级并行 |
| 8 进程 fault 不同 inode | ext4 全局锁与块设备并发 |
| 大文件两遍读、反向读和随机读 | 全局预算、命中率和淘汰 |
| 8 路顺序写，分别 close/fsync | dirty budget、writeback 和 fsync 序列 |
| mmap + truncate/mprotect/fork 压力 | generation、pin 和 COW 正确性 |
| shell pipeline/FIFO/CLONE_FILES+exec | EOF 与 FD 生命周期 |

短测收益必须超过同配置运行噪声；默认至少 5% wall-time 改善并有对应计数器变化。随后才跑一次完整带 profile 的 BuildStorm，最后跑三次无 profile 官方流程。

### 最终硬指标

1. Cargo 完成后 5 秒内产生 `BUILDSTORM_COMPILE ... ok=true` 和 group end。
2. 文件 fault 相关 mutex wait 相对当前下降至少 80%，随后继续逼近 Linux 的数量级。
3. 预构建/正式阶段活跃核至少达到约 5.5/5.0；同时 active core-s 分别降到 Linux 的 1.5 倍以内，防止以更多 CPU 忙等换取表面利用率。
4. PageCache 分配、销毁和 `memset` 代表栈各自降到阶段 CPU 样本的 1% 以下，前台 dirty eviction 接近 0。
5. ext4 全局锁等待下降至少 90%；若此时已不在 top hotspot，可停止高风险 ext4 重构。
6. 无 profile 的预构建 + 正式阶段三次运行均完成，最差不超过 50 分钟，中位数不超过 45 分钟。
7. RV/LA 的 `make all`、cagent、文件系统/pipe/mmap/fork/fsync 回归和并发压力测试无新增失败。

## 不应采用的“快速优化”

- 不要只设置 `CARGO_BUILD_JOBS=8` 或 `MAKEFLAGS=-j8`：Linux 对照已经证明 job 创建不是限制。
- 不要把 ext4 `Mutex` 改成 `RwLock` 或直接删除锁：lwext4 的共享可变状态不支持这样做。
- 不要只把每文件 cache 从 1 MiB 改成一个巨大常量：它没有全局内存压力控制，也不能解决锁内 I/O。
- 不要在现有临界区里扩大 readahead：会把一次 fault 持锁时间进一步放大。
- 不要继续提高 task migration 频率：当前主要问题是没有 runnable task，而不是 runnable task 放错 CPU。
- 不要以删除 snapshot 换取成绩：Linux 同 snapshot 已证明可达，且污染镜像会破坏实验可复现性。
- 不要把预置 target 缓存当作内核优化结论：评测脚本会删除目标目录的一部分，缓存 hash 也受 toolchain/Cargo.lock/源码影响；它可以作为额外工程优化，但不能掩盖冷编译内核瓶颈。

## 推荐的实际执行顺序

若目标是尽快稳定进入 50 分钟，最合理的前三个开发迭代是：

1. **EOF/FD 正确性 + 分层计数器**：让每次完整实验都有可信终点。
2. **两阶段 fault + page-slot 锁外 fill**：直接命中 74%～80% 的 mutex 热链，恢复多核并行。
3. **全局 cache budget + 私有映射共享 clean page**：删除 2.4～3.0 倍 active CPU 工作量中的缓存抖动、双重页和 copy。

完成第三步后再重新生成同一套 CPU/off-CPU/FS 图。此时若 ext4 全局锁仍居首位，进入 read-plan；若已经被新的热点取代，就按新证据调整，而不是机械执行旧计划。这是保证优化有效、避免拍脑袋的核心约束。

## 数据校验

本报告使用的机器摘要：

```text
Moss  summary.json  sha256 4ac56aea8cff856f7128f8be574710abbacd8fad038ec3ba33ef48a9201db8f7
Linux summary.json  sha256 d1297c1b30fcfb746a8f5e2c2d8c1d58b375c5c0df2a111e707d1ac7648055f5
```

原始 profile、完整 console 和带符号内核保留在本机并由各 profile 目录的 `.gitignore` 排除；报告引用的摘要和 SVG 可以直接复核上述结论。
