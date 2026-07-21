# Moss RISC-V BuildStorm 全流程性能报告

## 结论

这次数据覆盖了 BuildStorm 的 setup、完整 446-crate 预构建和正式构建，不是固定时长截取。预构建耗时 **76 分 28.19 秒**，正式阶段到 Cargo 成功构建耗时 **42 分 04.89 秒**，其中 Cargo 自报 **41 分 25 秒**。预构建本身已经超过评测机 50 分钟限制；即使完全删除预构建，正式任务也只剩约 8 分钟余量。因此，当前版本不能只靠调小日志或调整 Cargo 参数达到稳定过线。

最重要的内核热点是文件映射缺页路径：`handle_page_fault -> BackendOps::populate -> CowBackend::alloc_new_at`。预构建和正式阶段分别有 **80.25%**、**74.00%** 的内核 mutex 等待栈经过这条路径。该路径在持有进程 `AddrSpace` mutex 时读取文件，又进入持锁跨 I/O 的页缓存和 ext4 路径，直接破坏多核 rustc 的并行性。

ext4 全局锁确实是有效热点，但不是唯一问题：其精确竞争等待为预构建 **981.46 task-s**、正式阶段 **218.92 task-s**，分别占 ext4 inclusive 时间的 **44.15%**、**32.07%**。同步块 I/O 远小于页缓存 inclusive 时间，所以优化顺序应当是：

1. 拆掉缺页处理期间的 `AddrSpace` 大锁和页缓存跨 I/O 锁。
2. 修复正式构建成功后管道不产生 EOF 的退出/FD 生命周期问题。
3. 再缩小 ext4 全局锁，最后才是同步块 I/O 和 readahead。

## 可直接查看的火焰图

整套流程：

- [全流程 CPU](./guest-cpu-all.svg)
- [全流程 workload off-CPU](./guest-offcpu-workload-all.svg)
- [全流程 mutex 等待](./guest-offcpu-lock-all.svg)
- [全流程 ext4 全局锁等待](./guest-ext4-lock-wait-all.svg)
- [全流程 ext4、页缓存和同步 I/O](./guest-fs-io-latency-all.svg)

预构建：

- [预构建 CPU](./guest-cpu-prebuild.svg)
- [预构建 workload off-CPU](./guest-offcpu-workload-prebuild.svg)
- [预构建 mutex 等待](./guest-offcpu-lock-prebuild.svg)
- [预构建 ext4 全局锁等待](./guest-ext4-lock-wait-prebuild.svg)
- [预构建 ext4、页缓存和同步 I/O](./guest-fs-io-latency-prebuild.svg)

正式任务：

- [正式任务 CPU](./guest-cpu-timed.svg)
- [正式任务 workload off-CPU](./guest-offcpu-workload-timed.svg)
- [正式任务 mutex 等待](./guest-offcpu-lock-timed.svg)
- [正式任务 ext4 全局锁等待](./guest-ext4-lock-wait-timed.svg)
- [正式任务 ext4、页缓存和同步 I/O](./guest-fs-io-latency-timed.svg)

`guest-offcpu-*.svg` 保留了包含内核后台任务的原始总图；分析时优先看 `guest-offcpu-workload-*.svg`，后者排除了长期睡眠的 `gc`、`rebalance-task` 和 `profile-watchdog`。

## 测量范围与完整性

运行配置与评测一致：RISC-V virt、8 vCPU、8 GiB、相同 raw rootfs 的一次性 qcow2 overlay。基础镜像未被写入。CPU 以每 vCPU 10 Hz 采样；mutex、精确 ext4 全局锁和同步块读写记录全部事件；ext4 inclusive、页缓存 inclusive、总 off-CPU 分别按 1/16、1/32、1/8 抽样并按采样率还原时间。

| 阶段 | Guest 单调时钟耗时 | 结果 |
|---|---:|---|
| setup | 29.86 s | 完成 |
| 446-crate 预构建 | 4,588.19 s（76:28.19） | 446/446 完成 |
| 正式任务 | 2,524.89 s（42:04.89） | Cargo 构建成功 |

正式阶段开始前输出预构建 profile 的串口文本约 14 MiB，这段导出时间不计入上表的预构建或正式阶段。主机总运行时间约 3 小时 06 分还包含这次串口导出和构建后挂起，因此不能拿它当构建耗时。

最终 profile 含 27,942 条聚合 CPU 栈和 44,547 条聚合等待栈：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=2
```

等待数据的单位是累计 task-time：多个并发进程同时睡眠时会同时累计，因此数值可以超过墙钟时间，也不能把嵌套的 ext4、页缓存和块 I/O 数值相加。抽样事件的总时间是估计值，精确 ext4 锁、mutex 和块 I/O 是全量事件。

## CPU 并行度

| 阶段 | CPU 样本 | idle 样本 | 非 idle 样本 | 非 idle 占比 | 8 核均匀采样下的活跃核等价值 |
|---|---:|---:|---:|---:|---:|
| 预构建 | 281,412 | 151,792 | 129,620 | 46.06% | 3.68 核 |
| 正式任务 | 222,578 | 156,862 | 65,716 | 29.52% | 2.36 核 |

RISC-V QEMU 重负载时可能丢失周期 tick，因此上表不是硬件 PMU 利用率；按墙钟和实际收到的非 idle 样本计算，预构建只有 2.83 个保守 core-equivalent，正式任务为 2.60 个。两种算法都说明 8 核远未被持续利用。

在非 idle 样本中，预构建由 `rustc` 占 68.47%、`cc1` 占 10.95%、RISC-V GNU 工具占 9.28%、LLVM `opt cgu.0` 占 5.15%；正式阶段 `rustc` 占 78.90%。这不是“Cargo 没有启动并行任务”：中段确实有多组编译器并行运行，但大量 worker 在 futex、文件缺页和依赖等待中睡眠，后段又受依赖图和链接单任务限制。

预构建末尾尤其明显：进度到 444/446 后只剩 `axbuild`，到 445/446 后只剩 `tg-xtask`，这两个长尾阶段无法通过增加 Cargo jobs 获得 8 倍加速。

## off-CPU 等待

以下百分比以排除永久后台任务后的 workload off-CPU 总量为分母：

| 等待原因 | 预构建 | 正式任务 |
|---|---:|---:|
| futex | 60.30% | 50.88% |
| poll | 22.23% | 11.55% |
| waitpid | 2.69% | 21.07% |
| pipe read | 6.06% | 10.21% |
| 内核 mutex | 6.99% | 6.23% |
| nanosleep | 1.61% | 0% |
| 其他 | 0.12% | 0.07% |

futex、poll 和 waitpid 大部分是 Cargo/rustc 的工作协调或依赖图等待，不能简单视为 futex 实现慢。最有价值的优化入口是其中可归因的内核 mutex 路径；把该串行区缩短后，futex 等待是否下降应当用同一套 profile 做 A/B 验证。

## mutex 热点

mutex 累计等待为预构建 **11,166.94 task-s**、正式阶段 **8,172.23 task-s**。按完整等待栈分类：

| mutex 调用路径 | 预构建 | 正式任务 |
|---|---:|---:|
| 经过 COW/文件缺页分配 | 80.25% | 74.00% |
| 其他 | 16.79% | 23.87% |
| 地址空间修改 | 0.57% | 1.18% |
| 页缓存直接路径 | 1.08% | 0.30% |
| futex bookkeeping | 0.72% | 0.54% |
| ext4 直接路径 | 0.59% | 0.12% |

“经过 COW/文件缺页分配”表示等待栈含有：

```text
handle_page_fault
  -> AddrSpace::handle_page_fault
  -> BackendOps::populate
  -> CowBackend::alloc_new_at
```

它可能进一步等待页缓存或 ext4 的 mutex，不能和下层分类相加。代码上，缺页入口持有 `ProcessData::aspace()` 的 `Mutex<AddrSpace>`，`CowBackend::alloc_new_at` 又同步执行 `file.read_at`。这使一次可睡眠的文件读取处在进程级地址空间临界区内，同一 rustc 的其他缺页和地址空间操作只能排队。

## ext4、页缓存与同步块 I/O

| 指标（inclusive task-time） | 预构建 | 正式任务 |
|---|---:|---:|
| ext4 路径 | 2,222.89 s | 682.54 s |
| 精确 ext4 全局锁竞争 | 981.46 s | 218.92 s |
| 页缓存路径 | 15,042.35 s | 9,343.23 s |
| 同步块读取 | 393.08 s | 175.67 s |
| 同步块写入 | 63.58 s | 11.72 s |

精确 ext4 锁等待占 ext4 inclusive 时间的 44.15% 和 32.07%，说明 `Ext4Filesystem::inner: Mutex<LwExt4Filesystem>` 对并发文件读取有实质影响。精确锁图中最大的单一路径同样是 rustc 文件映射缺页：预构建占 ext4 锁图 55.07%，正式任务占 64.80%。

页缓存图更突出：`CowBackend::alloc_new_at -> CachedFile::with_page_or_insert_impl` 分别占组合 FS 图的 64.69% 和 71.85%。当前 `with_page_or_insert_impl` 同时持有 `cache_op` 和 `page_cache` mutex，再在锁内执行 miss fill、ext4 读取、淘汰和可能的脏页回写。不同页面的 cache miss 因而被同一个文件级临界区串行化。

同步读写合计仅为页缓存 inclusive 时间的 3.04% 和 2.01%。这并不表示块设备没有开销，而是说明当前首先损失在上层锁覆盖范围和同步 miss 组织方式；直接增加 virtio 队列或只做 readahead，不会先解决最大的等待路径。

## 正式构建后的 EOF 挂起

日志已经出现：

```text
Finished `release` profile [optimized] target(s) in 41m 25s
```

但脚本随后超过 10 分钟仍未退出。现场中 `tee` 睡在：

```text
Pipe::read -> read_file_like -> handle_syscall
```

这表示至少还有一个写端引用没有在进程/线程退出时关闭，或共享 FD table 的最后持有者没有正确释放。这个正在进行的等待在抓取内存时仍是 pending，因此没有计入上面的已完成 off-CPU 聚合值；实际端到端时间只会更差。

当前 `Pipe::closed()` 只检查 `Arc<Shared>` 的总 strong count，不能区分读端和写端；同时 `sys_unshare(CLONE_FILES)` 仍是成功返回但不复制文件表的 stub。不能仅凭这次栈确定是哪一个调用泄漏，但应该一起审计 `CLONE_FILES`、exec unshare、`exit_group` 和最后线程退出时的 FD table 所有权。

## 优化方案与验收顺序

### P0：拆分文件缺页的大临界区

将文件映射缺页改成两阶段提交：

1. 在 `AddrSpace` 锁内确认 VMA、backend、权限和映射代数，取得独立引用。
2. 释放 `AddrSpace` 锁，在锁外分配 frame 并完成 page-cache/ext4 读取。
3. 重新取得锁，验证 VMA 和页表仍匹配；安装成功则提交，若另一个 CPU 已安装则丢弃本地 frame 并复用现有映射。

页缓存 miss 也应使用“锁内查找/登记 in-flight page，锁外读盘，锁内发布”的模式。至少按 inode+page 建立 in-flight 状态，让同一页合并等待、不同页并行；淘汰回写不能在 `page_cache` mutex 下执行同步 I/O。

验收目标：COW 缺页相关 mutex wait 降低至少 70%，预构建非 idle 样本占比提升到 70% 以上，并确认并发 fault 的 loser frame 无泄漏、truncate/unmap 与 fill 的代数校验正确。

### P0：修复管道 EOF 和进程 FD 生命周期

为 pipe 明确维护 reader/writer endpoint 计数，drop 最后写端时唤醒 reader 并返回 EOF；实现真实的 `CLONE_FILES` 文件表共享/复制语义，保证 exec、exit、`exit_group` 只在最后持有者释放时关闭全部 FD。加入回归测试：父进程通过 `cmd | tee` 等待，所有 writer 子进程退出后 reader 必须立即读到 EOF。

验收目标：正式构建成功后 5 秒内输出 BuildStorm END/RESULT，进程表中无残留 `tee`、Cargo 或 writer。

### P1：减少 ext4 全局串行化

先利用上述页缓存 miss 合并减少进入 lwext4 的次数；随后将只读数据路径与元数据修改路径分开。若 lwext4 内部不能安全并发，应至少将目录/元数据操作与已定位 inode 的数据读取分离，或给只读请求增加受控并发层，而不是直接把全局 mutex 删除。

验收目标：精确 ext4 lock wait 至少降低 50%，同时跑并发读写、rename/unlink/fsync 和断电一致性测试。

### P1：消除 50 分钟前的 446-crate 冷启动

仅靠调度器不能消除 Cargo 依赖图和 `axbuild`/`tg-xtask` 的串行长尾。评测镜像应预置与 `rust-toolchain.toml`、Cargo.lock 和源码 hash 完全匹配的 tg-xtask/host target cache，或将预构建产物作为只读基础层并在 VM 内用可写增量层复用。若评测规则要求现场冷编译，则必须在 P0 修改后重新测量，不能把旧 target 目录的热缓存结果当作内核优化。

验收目标：整个预构建阶段降到 8 分钟以内，使正式任务仍有至少 40 分钟预算；正式任务再通过并行缺页和链接器线程优化降到 35 分钟以内。

### P2：块 I/O、链接和导出开销

在上层锁拆分后，再评估 page readahead、合并相邻同步读、virtio 多请求并发和异步 writeback。Cargo 长尾可尝试线程化 linker/LTO，但要分别记录 `axbuild`、`tg-xtask` 和最终链接时长。

profile 导出不应再走 14 MiB 串口文本；本次最终结果已经通过冻结 guest 后导出二进制表完成。后续可正式增加 9p/virtio 或二进制 `/proc` 接口，避免观测工具本身占用几十分钟。

## 数据与本地原始产物

- [机器可读摘要](./summary.json)

用于重新渲染的 `final-profile.log`、完整 console、二进制 profile dump 和带符号内核只在本机留存，并由本目录的 `.gitignore` 排除。它们体积较大，不随仓库提交；报告、摘要和最终 SVG 不依赖这些文件即可查看。

关键 SHA-256：

```text
final-kernel.bin         ba6eadc782788a9fca86a456489a1462993e211826b36733cc204c2b7e46255a
final-kernel.profile.elf 6c098eb2aff89fe56d012b59736c22f0dfc828ae2656448e0d2bff28d16fa4d5
final-profiles.bin       648a8141efce870d4afca4a1756911b3c9ad176c6ce71c15f4f4f86a862
```
