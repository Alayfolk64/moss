# RISC-V BuildStorm 跨 VMA unmap gather 优化报告

## 结论

本轮把完整地址空间回收中的匿名页和文件页所有权放进同一个有界
`UnmapGather`，不再让每个 VMA 分别完成一次 TLB shootdown。修改后的
顺序是：

```text
遍历所有 VMA
    -> 清除 PTE
    -> 暂存匿名 frame 引用或文件 folio 引用
    -> gather 达到 8192 页时执行一次 shootdown 并释放这一批
    -> 地址空间末尾执行最后一次 shootdown
    -> 释放剩余页面所有权
```

目标路径下降非常明确：

| 路径 | 上轮预构建 | 本轮预构建 | 变化 | 上轮正式 | 本轮正式 | 变化 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 回收线程中的 `flush_tlb` | 322 | **5** | **-98.4%** | 119 | **2** | **-98.3%** |
| 回收线程中的 SBI RFENCE | 289 | **5** | **-98.3%** | 113 | **2** | **-98.2%** |
| 全部 `flush_tlb` | 482 | **172** | **-64.3%** | 171 | **63** | **-63.2%** |
| 全部 SBI RFENCE | 438 | **161** | **-63.2%** | 161 | **58** | **-64.0%** |

`exit clear` 和 `exec clear` 的调用次数没有变化，累计写锁持有时间也明显
下降：

| 事件 | 上轮预构建 | 本轮预构建 | 变化 | 上轮正式 | 本轮正式 | 变化 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `exit clear` | 90.53 s | **65.83 s** | **-27.3%** | 34.42 s | **28.49 s** | **-17.2%** |
| `exec clear` | 74.87 s | **41.31 s** | **-44.8%** | 28.98 s | **16.50 s** | **-43.1%** |

端到端结果同样变快：空 target 的 tg-xtask 预构建从 20 分 45 秒降到
20 分 06 秒，正式 BuildStorm 从 543.10 秒降到 531.20 秒。但这里仅有
一次新版本的完整冷镜像运行，crate 调度和宿主负载仍会造成波动。因此可以
确认的是“重复 RFENCE 和 clear 持锁时间已经被消除”，端到端约 2%～3% 的
改善目前是与局部证据一致的观测结果，还不能当成多次重复后的稳定均值。

## 为什么旧实现会重复 shootdown

`AddrSpace::clear` 原本已经只创建一次 `PageTableMut`，并在持有地址空间
写锁时遍历全部 VMA。但是所有权释放仍由每个 backend 自己完成：

```text
VMA 0: clear PTE -> pt.finish -> drop frames
VMA 1: clear PTE -> pt.finish -> drop frames
VMA 2: clear PTE -> pt.finish -> drop frames
...
```

页表修改器在累计超过 16 个地址后会选择全地址空间 TLB flush。对于具有很多
小 VMA 的 rustc、链接器和构建脚本进程，旧实现因此可能在同一次 exit 或
exec 中多次发起跨核 SBI RFENCE。

不能简单地先清 PTE、立即释放 frame，最后才 flush。其他 CPU 在 shootdown
完成前可能仍持有旧 TLB 项；如果物理页已经被重新分配，旧翻译就会访问新
所有者的数据。正确的生命周期必须是：

```text
清 PTE
    -> 暂时保留 frame/folio 所有权
    -> 完成所有相关 CPU 的 TLB 失效
    -> 最后释放物理页或 folio pin
```

## 修改内容

涉及文件：

```text
kernel/src/mm/aspace/backend/mod.rs
kernel/src/mm/aspace/backend/cow.rs
kernel/src/mm/aspace/backend/file.rs
kernel/src/mm/aspace/mod.rs
```

### 统一的 deferred owner

`DeferredUnmapPage` 保存两类仍不能释放的所有权：

```text
Anonymous { paddr, page_size }
File(FolioPageRef)
```

匿名页在 shootdown 后通过既有的 `drop_referenced_frame` 减少引用计数；
文件页则在 shootdown 后释放 `FolioPageRef`。两种路径共享同一个 gather，
因此跨 Cow/File VMA 也不需要分别 flush。

### 固定容量

`UNMAP_GATHER_PAGES` 为 8192 页，即按 4 KiB 基页计算每批最多覆盖 32 MiB
映射。达到上限时：

1. 先调用 `pt.finish()`；
2. 确认旧 TLB 翻译失效；
3. 再 drain deferred owner；
4. 继续收集下一批。

这使大地址空间仍能批处理，又不会让退出一个超大进程无限扩张内核 `Vec`。

### 只改变完整 clear

`Backend::unmap_for_clear` 仅供 `AddrSpace::clear` 使用。普通 `munmap`、
`madvise` 和 backend 的常规 `unmap` 仍创建局部 gather 并在本次操作结尾
立即 finish。这避免把仍在运行的地址空间的 TLB 失效推迟到 syscall 返回
之后。

Linear 和 Shared backend 仍显式调用各自原来的 `unmap`。只有能够安全转移
页面所有权的 Cow 和 File backend 使用跨 VMA gather。

### 错误路径

正常路径在释放 `MemoryArea` 前完成 gather 和页表 finish。若发现理论上不应
出现的 metadata/PTE 不一致：

1. 仍先完成已经收集页面的 shootdown；
2. 释放这些已经确认解除映射的 deferred owner；
3. 保留其余 `MemoryArea` 所有权并 panic。

致命错误路径宁可泄漏未知 owner，也不释放仍可能被 PTE 或旧 TLB 引用的
物理页。

## 构建前的三遍静态检查

### 第一遍：所有权和 TLB 顺序

- 匿名 frame 与文件 folio 都在清 PTE 后进入 gather；
- `UnmapGather::finish` 总是先执行 `pt.finish()`，再 drain owner；
- gather 满、地址空间末尾和错误路径均遵守相同顺序；
- `MemoryArea` 只在最后一次 shootdown 完成后释放；
- 致命错误路径不会提前释放所有权不明的剩余 VMA。

### 第二遍：并发范围和 backend 覆盖

- clear 原本就持有整个地址空间写锁，本轮没有扩大锁的地址范围；
- Cow、File、Linear、Shared 四种 backend 均有显式分支；
- 共享 gather 只进入 exit/exec/deferred reclaim 的完整 clear；
- 普通 `munmap` 和 `madvise` 的完成边界未被延后；
- frame 引用分片锁、文件页索引锁和 allocator 锁内均不会执行 TLB
  shootdown。

### 第三遍：容量、可见性和构建边界

- gather 容量固定为 8192 项，不随地址空间大小无界增长；
- `drop_referenced_frame` 和两个 `unmap_present_into` 仅提升为 backend
  模块内部可见；
- 没有新增 `unsafe`；
- 没有遗留每 VMA `finish_unmap_batch` 路径；
- `cargo metadata --offline`、格式检查和 `git diff --check` 通过；
- 三遍审查完成后，`make PROFILE=y kernel-rv` 构建通过。

## 完整测试口径

根文件系统重新从官方压缩镜像独立解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
tmp/profile-rv-unmap-gather-20260725/sdcard-rv.img
15032385536 bytes
```

运行条件：

- Ubuntu 宿主直接运行，不使用 Docker；
- QEMU RISC-V `virt`；
- 8 GiB、8 vCPU；
- `-snapshot`；
- 没有其他 QEMU 进程；
- 从空 target 完整执行 cagent、tg-xtask 预构建、正式 BuildStorm，以及
  最后的进程退出和地址空间回收；
- CPU 10 Hz；
- off-CPU 1/8 抽样；
- ext4 1/16 抽样；
- page cache 1/32 抽样；
- 地址空间写锁 1/16 抽样；
- mprotect 1/32 抽样。

cagent 10/10 通过，正式 BuildStorm 成功：

```text
Finished release target(s) in 8m 34s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=531.20
```

采样完整性：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

## 端到端时间

| 指标 | 上轮 direct frame index | 本轮 unmap gather | 变化 |
| --- | ---: | ---: | ---: |
| tg-xtask 预构建输出 | 20m45s | **20m06s** | **-39s** |
| 正式 Cargo 输出 | 约 526s | **514s** | 约 -12s |
| BuildStorm | 543.10s | **531.20s** | **-11.90s / -2.19%** |
| profile 预构建阶段 | 1246.59s | **1207.67s** | **-38.91s / -3.12%** |
| profile 正式阶段 | 612.09s | **599.58s** | **-12.51s / -2.04%** |
| profile 三阶段总和 | 1863.22s | **1811.64s** | **-51.58s / -2.77%** |
| 宿主全流程 | 1876.30s | **1816.15s** | **-60.15s / -3.21%** |

宿主 runner 以 15 秒轮询 ready marker，宿主总时的差值比 guest 阶段总和
多约 8.6 秒，不能把这部分归因于内核优化。guest 内部阶段时间和
`BUILDSTORM_COMPILE` 才是主要对比口径。

## CPU 火焰图分析

### 目标回收路径

| 路径 | 上轮预构建 | 本轮预构建 | 上轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| 非 idle CPU 样本 | 11,345 | 10,286 | 4,838 | 4,509 |
| resident reclaim | 1,421 / 12.53% | **930 / 9.04%** | 566 / 11.70% | **419 / 9.29%** |
| reclaim 中 Cow unmap | 812 | **216** | 341 | **112** |
| reclaim 中 `flush_tlb` | 322 | **5** | 119 | **2** |
| reclaim 中 SBI RFENCE | 289 | **5** | 113 | **2** |

按阶段秒数归一化后，resident reclaim 从 1.140 样本/秒降到 0.770
样本/秒，预构建下降约 32.5%；正式阶段从 0.925 降到 0.699 样本/秒，
下降约 24.4%。

新的 `UnmapGather::finish` 在预构建和正式阶段分别有 273、112 个 inclusive
样本。这不是新增的额外墙钟时间：该函数现在承接了 frame 引用释放、folio
释放和少量剩余 shootdown。对比旧回收路径总样本及 RFENCE 样本，合并后的
总代价仍明显更低。

### 为什么全局 RFENCE 没有归零

完整 clear 之外还有必须及时失效的路径：

- 普通 `munmap`；
- `madvise` 丢弃；
- COW 权限升级和 fault commit；
- mprotect；
- gather 每累计 8192 页的有界 flush。

本轮有意不改变这些路径。预构建中普通 `munmap` 的 flush 样本为 98，
page fault 为 47；它们解释了大部分剩余全局 `flush_tlb`。

火焰图：

- [完整 CPU](./guest-cpu-all.svg)
- [预构建 CPU](./guest-cpu-prebuild.svg)
- [正式 CPU](./guest-cpu-timed.svg)
- [预构建 exit clear](./guest-aspace-exit-clear-write-hold-prebuild.svg)
- [预构建 exec clear](./guest-aspace-exec-clear-write-hold-prebuild.svg)
- [正式 exit clear](./guest-aspace-exit-clear-write-hold-timed.svg)
- [正式 exec clear](./guest-aspace-exec-clear-write-hold-timed.svg)

## 等待时间

以下是抽样放大后的累计任务时间。多个任务可以并行等待，inclusive 事件也
可能重叠，因此不能把各行相加成墙钟时间。

| 事件 | 上轮预构建 | 本轮预构建 | 上轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| page cache inclusive | 159.51s | **143.47s** | **44.46s** | 49.84s |
| ext4 inclusive | 32.94s | **27.56s** | 49.18s | **48.84s** |
| ext4 全局锁等待 | 6.38s | **5.39s** | 1.63s | **1.36s** |
| 同步块读取 | 6.46s | **5.61s** | **2.14s** | 2.25s |
| folio load wait | 3.94s | **3.51s** | 0.41s | **0.39s** |
| mprotect total | **69.47s** | 70.03s | 31.14s | **30.80s** |

文件系统指标有下降也有上升，且本轮没有修改这些实现。它们只能说明没有
出现新的明显 I/O 回归，不能作为 unmap gather 的直接收益证据。

相关火焰图：

- [完整 off-CPU](./guest-offcpu-all.svg)
- [正式锁等待](./guest-offcpu-lock-timed.svg)
- [正式 I/O](./guest-fs-io-latency-timed.svg)
- [预构建 ext4 锁等待](./guest-ext4-lock-wait-prebuild.svg)
- [正式 folio load 等待](./guest-folio-load-wait-timed.svg)

## 下一轮热点：页分配和文件页路径中的中断保存锁

去掉重复 RFENCE 后，当前最大的内核叶热点不再是 SBI，而是
`NoPreemptIrqSave::release`：

| 叶函数 | 预构建 | 占非 idle CPU | 正式 | 占非 idle CPU |
| --- | ---: | ---: | ---: | ---: |
| `NoPreemptIrqSave::release` | 1,605 | **15.60%** | 614 | **13.62%** |
| `CurrentTask::try_get` | 695 | 6.76% | 246 | 5.46% |
| SBI RFENCE | 161 | 1.57% | 58 | 1.29% |

`NoPreemptIrqSave::release` 的叶样本中，预构建 85.2%、正式 75.0% 的直接
父节点是 `BaseSpinLockGuard::drop`。主要祖先路径包括：

```text
预构建：
  sys_write / CachedFile::write_at          403
  page fault                                386
  alloc_folio_frame                         365
  GlobalAllocator::alloc                    295
  anonymous alloc_frame                     168

正式：
  GlobalAllocator::alloc                    152
  page fault                                141
  sync / dirty-page writeback                95
  alloc_folio_frame                          77
```

这里不能直接把所有 `SpinNoIrq` 改成无锁：有些 allocator、run queue 和
中断共享状态确实需要 IRQ safety。下一轮应先审计 order-0 PCP 与本地大
folio cache 的全部调用域，确认它们只在可抢占的任务上下文进入，再选择：

1. 将仅用于任务上下文的缓存锁从 IRQ-save 锁缩小为 preempt-only；
2. 或在关闭抢占后用真正的 per-CPU 本地缓存，消除每页 cache hit 的 spin
   lock 和重复 current-task 查找；
3. refill/drain 仍通过全局页分配器锁完成，不能把中央 allocator 变成无锁；
4. OOM drain 和跨 CPU 回收必须保留安全的远程访问协议。

这项修改必须先完成三遍静态检查，重点验证迁核窗口、中断调用者、跨 CPU
drain、OOM 回收和嵌套 allocator 锁顺序，之后才允许构建及下一次完整采样。

## 产物

- [summary.json](./summary.json)
- [console.log](./console.log)
- [完整 CPU 火焰图](./guest-cpu-all.svg)
- [完整 off-CPU 火焰图](./guest-offcpu-all.svg)
- [完整锁等待火焰图](./guest-offcpu-lock-all.svg)
- [完整 I/O 火焰图](./guest-fs-io-latency-all.svg)
