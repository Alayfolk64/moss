# RISC-V BuildStorm 物理页引用直接索引优化报告

## 结论

本轮把匿名/COW 物理页引用表从 64 个分片的

```text
BTreeMap<PhysAddr, usize>
```

改成按实际物理 RAM 区间建立的紧凑直接索引：

```text
physical page
    -> compact page index
    -> shard = index % 64
    -> slot  = index / 64
    -> SpinNoPreempt<Vec<usize>>
```

结果分成两部分：

- 目标热点确实下降。上一轮属于 frame table 的 BTree 查找，在预构建和正式阶段分别有 684、348 个 CPU 样本；本轮直接索引的具名 `FrameRefTable` 路径分别为 127、49 个样本，按样本数和阶段时间归一化后约下降 81% 和 86%。
- 端到端时间仍然持平。正式 BuildStorm 为 543.10 秒，上一轮为 542.03 秒；同一内核在修复采样脚本前的一次完整运行为 542.79 秒。三次结果只支持“约 543 秒的稳定水平”，不支持直接索引已经让完整任务显著变快。

局部改动没有引入新的串行点或正确性错误，但释放出的 CPU 时间被其余页表、文件页索引、TLB shootdown、文件系统和任务依赖长尾吸收了。下一轮应继续处理完整地址空间回收中的重复 TLB shootdown，而不是继续修改已经降到约 1% 非 idle CPU 样本的 frame 引用索引。

## 修改内容

主要文件：

```text
kernel/src/mm/aspace/backend/cow.rs
kernel/src/mm/aspace/backend/mod.rs
kernel/src/mm/aspace/mod.rs
kernel/src/mm/mod.rs
kernel/src/entry.rs
```

### 紧凑物理页编号

初始化阶段读取 `phys_ram_ranges()`，按物理起始地址排序，并检查：

- 每个区间的起点和终点均按 4 KiB 对齐；
- 区间长度不会发生整数溢出；
- 不同物理 RAM 区间不重叠；
- 所有区间至少包含一个可编号页面。

每个区间保存：

```text
start_pfn
end_pfn
index_base
```

物理 RAM 之间的空洞不会占用引用计数槽位。RISC-V 的连续 RAM 和 LoongArch 的分段 RAM 都使用同一查找规则。

### 64 路锁分片

紧凑索引没有改成一把全局锁。逻辑页号仍按 64 路条带化：

```text
shard = index % 64
slot  = index / 64
```

相邻物理页落到不同分片，不同 CPU 对不同页面的引用操作仍可并行。每个分片使用 `SpinNoPreempt<Vec<usize>>`，因为该表只从普通任务上下文进入，不从中断处理器进入。

### 引用生命周期

以下语义保持不变：

- 新分配匿名页以计数 1 发布；
- fault prepare 和 fork 通过 checked increment 预留引用；
- exclusive COW 在同一分片锁内验证 `count == 1` 并条件升级 PTE 权限；
- fork 在发布子 PTE 前增加引用，失败路径回滚；
- unmap 先清 PTE、完成 TLB 失效，再减少引用；
- 计数归零后先释放分片锁，再把物理页归还分配器；
- 页面分配、释放、复制、文件读取和 TLB shootdown均不在 frame 分片锁内。

### 内存成本

8 GiB / 4 KiB 对应 2,097,152 个基页。每页保存一个 64 位 `usize`：

```text
2,097,152 × 8 bytes = 16 MiB
```

约占 8 GiB 的 0.195%。这是用固定、可预测的 16 MiB 换取每次引用操作从 `O(log n)` 降到常数时间。当前还没有把计数压成 `u32`，以免改变溢出和异常语义。

## 构建前的三遍静态检查

### 第一遍：地址到槽位的唯一性

- 物理区间先排序，再检查对齐和重叠；
- 区间空洞不分配槽位；
- `index_base + pfn - start_pfn` 对每个 RAM 页唯一；
- `shard/slot` 条带映射不会让两个逻辑页使用同一槽位；
- RISC-V 与 LoongArch 的 RAM 布局均满足边界条件。

### 第二遍：并发与引用所有权

- `with_count` 在 exclusive COW 的计数检查和 PTE 权限升级之间保持同一分片锁；
- fork 的引用预留发生在父/子 PTE 发布前；
- 新页初始化不会覆盖已有非零计数；
- 最后引用归零后才允许释放物理页；
- 分片锁内没有分配器、文件 I/O、回收或 TLB shootdown 调用；
- 64 路并行粒度未退化。

### 第三遍：初始化顺序和资源成本

- frame table 在任何用户地址空间创建前初始化；
- 此时物理内存区间和内核堆已可用；
- `spin::Once` 保证只初始化一次；
- 8 GiB 配置的 16 MiB 元数据可在启动阶段分配；
- `cargo metadata --offline`、格式检查和 `git diff --check` 通过；
- 三遍审查完成后，`make PROFILE=y kernel-rv` 构建通过。

## 测试与采样口径

测试镜像重新从下列官方压缩镜像独立解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
```

本次有效测试使用：

```text
tmp/profile-rv-frame-ref-index-symbol-safe-20260725/sdcard-rv.img
15032385536 bytes
```

运行条件：

- 宿主直接运行，不使用 Docker；
- QEMU RISC-V `virt`；
- 8 GiB、8 vCPU；
- `-snapshot`；
- 没有其他 QEMU 进程；
- 完整执行 cagent、空 target 的 tg-xtask 预构建、正式 BuildStorm、进程退出和地址空间回收；
- CPU 10 Hz；
- off-CPU 1/8 抽样；
- ext4 1/16 抽样；
- page cache 1/32 抽样；
- 地址空间写锁 1/16 抽样；
- mprotect 1/32 抽样。

cagent 10/10 通过，正式 BuildStorm 成功。既有的 `BUILDSTORM_MINIBUILD fail` 仍会打印，但不阻止 446-crate 预构建和正式任务完成。

采样完整性：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

## 采样脚本布局问题及修复

第一次完整运行结束后，CPU、off-CPU 和锁事件表本身有效，但阶段耗时字段异常。原因是旧脚本从 `ENABLED` 的地址开始连续读取 56 字节，隐含假设以下 Rust 静态变量在 ELF 中连续且偏移固定：

```text
PHASE_STARTED_NS
SKIPPED_CPU
PHASE_TOTAL_NS
```

新增 `FRAME_REFS` 后，链接器重新排列了静态变量，`PHASE_TOTAL_NS` 相对旧基址的偏移从 24 变成 40。Rust 和链接器均不保证这些独立静态变量的相对布局，因此旧假设无效。

修复后的 runner：

1. 用 `rust-nm -S` 分别解析三个精确符号；
2. 校验尺寸必须为 8、8、32 字节；
3. 在同一次 GDB 停机窗口分别抓取三个原始文件；
4. 校验 profiles shard 锁全清和所有文件尺寸；
5. 再组装成解析器兼容的 56 字节文件；
6. 在 `run.json` 中保存每个独立符号地址。

修复也完成了三遍静态检查后才重新执行全流程。本报告只用第二次、符号安全的完整采样做阶段归一化。第一次运行的 542.79 秒只作为端点重复性参考。

有效阶段数据：

```text
setup_ns    = 4,543,388,700
prebuild_ns = 1,246,587,394,800
timed_ns    = 612,090,246,200
```

## 端到端时间

| 指标 | folio batch | direct ref | frame lock preempt | 本轮 direct index |
| --- | ---: | ---: | ---: | ---: |
| tg-xtask 预构建 | 20m59s | **20m38s** | 20m48s | 20m45s |
| 正式 Cargo 输出 | 529.39s | 534.76s | 532.20s | 约 526s |
| BuildStorm | **539.38s** | 544.64s | 542.03s | 543.10s |
| profile 预构建阶段 | 1260.83s | **1239.52s** | 1249.37s | 1246.59s |
| profile 正式阶段 | **599.93s** | 621.33s | 610.57s | 612.09s |
| profile 三阶段总时间 | 1865.18s | 1865.40s | 1864.56s | **1863.22s** |
| 宿主全流程 | 1876.16s | 1876.27s | 1876.28s | 1876.30s |

与上一轮相比：

- 预构建阶段缩短 2.78 秒；
- 正式 profile 阶段增加 1.52 秒；
- 三阶段总和缩短 1.34 秒，即约 0.07%；
- BuildStorm 增加 1.07 秒，即约 0.20%；
- 宿主 15 秒轮询口径下总时基本完全相同。

这些差异低于跨次运行波动，不能解释成稳定端点收益。

## CPU 火焰图

### frame 引用目标路径

| 指标 | 上一轮预构建 | 本轮预构建 | 上一轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| frame BTree / direct index 具名样本 | 684 | **127** | 348 | **49** |
| 阶段非 idle 样本 | 11,983 | 11,345 | 5,011 | 4,838 |
| 占非 idle CPU | 5.71% | **1.12%** | 6.94% | **1.01%** |
| 样本/阶段秒 | 0.547 | **0.102** | 0.570 | **0.080** |
| 归一化变化 |  | **-81.4%** |  | **-85.9%** |

新路径的样本包含 `location`、`reference_frame`、`with_count`、`init_frame` 和 `release_frame`。函数可能被内联，因此这不是整个引用表的绝对 CPU 时间；但它与上一轮明确的 frame-table BTree 路径使用相同采样口径，足以证明目标查找成本明显下降。

所有 BTree 路径还包括 VMA、文件页索引和其他内核映射：

```text
预构建：1494 -> 863，下降 42.2%
正式：   712 -> 410，下降 42.4%
```

剩余 BTree 不能归因于匿名 frame 引用表。

### 主要内核路径

| 路径 | 上一轮预构建 | 本轮预构建 | 上一轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| page fault 总路径 | 26.60% | 27.46% | 27.10% | 26.93% |
| `CowFaultPlan::execute` | 9.47% | **8.44%** | 9.50% | **8.72%** |
| `CowBackend::commit_fault` | 6.34% | 7.02% | 7.00% | **6.88%** |
| COW unmap | 10.83% | **9.00%** | 9.80% | **8.97%** |
| resident reclaim | 14.11% | **12.53%** | 13.33% | **11.70%** |

page fault 总路径基本持平，但 execute、unmap 和 reclaim 的占比下降。commit 小幅双向波动，说明直接索引只消除了其中一层查找，没有改变页表提交本身。

火焰图：

- [完整 CPU](./guest-cpu-all.svg)
- [预构建 CPU](./guest-cpu-prebuild.svg)
- [正式 CPU](./guest-cpu-timed.svg)

## 等待与地址空间持锁

以下是抽样放大后的累计任务时间。不同任务可能并行等待，inclusive 事件也会重叠，不能把各行相加成墙钟时间。

| 事件 | 上一轮预构建 | 本轮预构建 | 上一轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| page cache inclusive | 192.01s | **159.51s** | **38.66s** | 44.46s |
| ext4 inclusive | 43.55s | **32.94s** | **42.99s** | 49.18s |
| ext4 全局锁等待 | 10.30s | **6.38s** | 1.71s | **1.63s** |
| 同步块读取 | 6.72s | **6.46s** | 2.38s | **2.14s** |
| folio load wait | 4.07s | **3.94s** | 0.63s | **0.41s** |
| mprotect total | 72.83s | **69.47s** | **30.70s** | 31.14s |

预构建文件系统等待整体更低，正式阶段 ext4/page-cache inclusive 则更高；这种双向变化与端点持平一致，不能归因成直接索引带来的稳定文件系统收益。

地址空间写锁：

| 事件 | 上一轮预构建 | 本轮预构建 | 上一轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| exit clear | 108.08s | **90.53s** | 39.90s | **34.42s** |
| exec clear | 90.34s | **74.87s** | 37.40s | **28.98s** |
| clone child | 85.11s | **83.73s** | 38.58s | **35.84s** |
| clone parent | **42.03s** | 44.03s | 21.06s | **16.33s** |
| munmap | 33.38s | **31.62s** | **12.64s** | 14.01s |

exit/exec 的 sampled count 完全相同而累计时间和最大样本同时下降：

```text
exit prebuild max: 1766.89 ms -> 1319.23 ms
exec prebuild max:  417.09 ms ->  302.27 ms
exit timed max:      819.99 ms ->  652.48 ms
exec timed max:      236.46 ms ->  207.01 ms
```

这与 frame 引用释放查找变短方向一致，但单次运行仍可能受地址空间规模分布影响。因此它是支持局部收益的证据，不是端点加速证明。

相关火焰图：

- [完整 off-CPU](./guest-offcpu-all.svg)
- [正式锁等待](./guest-offcpu-lock-timed.svg)
- [正式 I/O](./guest-fs-io-latency-timed.svg)
- [预构建 exit clear](./guest-aspace-exit-clear-write-hold-prebuild.svg)
- [预构建 exec clear](./guest-aspace-exec-clear-write-hold-prebuild.svg)
- [正式 clone child](./guest-aspace-clone-child-write-hold-timed.svg)

## 下一轮：跨 VMA 合并 clear 的 TLB shootdown

本轮之后，frame 直接索引只占约 1% 非 idle CPU。下一项具有明确调用路径和可修改边界的热点是完整地址空间回收中的 TLB shootdown：

| 路径 | 预构建样本 | 占非 idle CPU | 正式样本 | 占非 idle CPU |
| --- | ---: | ---: | ---: | ---: |
| `flush_tlb` | 482 | 4.25% | 171 | 3.53% |
| 其中 SBI RFENCE | 438 | 3.86% | 161 | 3.33% |
| resident reclaim | 1421 | 12.53% | 566 | 11.70% |

SBI RFENCE 是 `flush_tlb` 的子路径，两行不能相加。

当前 `AddrSpace::clear` 已经只创建一个 `PageTableMut` 并遍历所有 VMA，但 `CowBackend::unmap` 和 `FileBackend::unmap` 仍会在每个 VMA 结束时各自执行 `pt.finish()`，然后释放 deferred frame/folio。页表代码累计超过 16 个地址时会选择一次全地址空间 flush。因此包含很多小 VMA 的 exit/exec 仍可能反复发起全局 RFENCE。

下一轮应：

1. 为完整 `AddrSpace::clear` 建立一个跨 VMA 的 unmap gather；
2. Cow 匿名页和文件 folio 都只在清 PTE 后放入 gather；
3. gather 达到固定上限或整个 clear 结束时才执行一次 `pt.finish()`；
4. TLB 已失效后再统一减少 frame 引用和释放 folio pin；
5. 普通 `munmap`、fault、mprotect 和仍在使用的地址空间不改变；
6. 错误路径也必须先 finish 再释放 deferred 所有权；
7. 保留容量上限，不能让一次超大地址空间退出无限占用内核堆。

这不会扩大地址空间写锁范围：`clear` 原本就持有同一把写锁遍历完整地址空间。变化只是在既有临界区内减少重复 TLB finish，并把物理页/folio 的释放推迟到对应 shootdown 之后。

预期收益只能先限定为：

- 减少 exit/exec/reclaim 中重复的 SBI RFENCE；
- 缩短 clear 的 CPU 时间和地址空间写锁持有时间；
- 不保证 Cargo 依赖长尾或完整 BuildStorm 立刻同比例缩短。

实现后仍需三遍静态检查，重点覆盖所有权、错误路径、跨 backend 顺序和固定容量，再允许构建和完整采样。

## 产物

- [summary.json](./summary.json)
- [console.log](./console.log)
- [完整 CPU 火焰图](./guest-cpu-all.svg)
- [完整 off-CPU 火焰图](./guest-offcpu-all.svg)
- [完整锁等待火焰图](./guest-offcpu-lock-all.svg)
- [完整 I/O 火焰图](./guest-fs-io-latency-all.svg)
