# RISC-V BuildStorm folio 批量提交优化报告

## 结论

本轮把文件私有映射的 fault-around 从“同一 folio 的每个 4 KiB 页面各自查询、验证并提交”改为“需求页加载一次、同一 folio 派生兄弟页、整批验证一次、逐 PTE 原子安装”。完整测试使用重新解压的官方 RISC-V 镜像、8 GiB 内存、8 个 vCPU 和 QEMU `-snapshot`，没有继承上一轮 guest `target`。

与同口径的 `resident-reclaim` 基线相比：

| 指标 | 上一轮 | 本轮 | 变化 |
| --- | ---: | ---: | ---: |
| tg-xtask 预构建 | 22 分 03 秒 | **20 分 59 秒** | **-4.84%** |
| 正式 Cargo 编译 | 572.37 秒 | **529.39 秒** | **-7.51%** |
| BuildStorm | 582.76 秒 | **539.38 秒** | **-7.44%** |
| profile 三阶段总时间 | 1989.0 秒 | **1865.2 秒** | **-6.22%** |
| 宿主全流程 | 1996.18 秒 | **1876.16 秒** | **-6.01%** |

这是一次完整 A/B 结果，不能单凭一次运行给出置信区间；但两轮的工作量计数非常接近，例如正式阶段 `mprotect` 调用分别为 661,489 和 660,881，差异仅 0.09%，而目标缺页路径的 CPU 占比、缺页采样速率和端到端时间同时下降，因此结果与代码改动方向一致。

## 修改内容

### 1. 从一个已固定的 folio 派生兄弟页面

`FolioPageRef` 新增：

- `folio_page_offset()`：返回该 4 KiB 页在 folio 内的下标；
- `folio_page_count()`：返回 folio 包含的基页数；
- `folio_page_ref(page)`：从同一个 `Arc<Folio>` 派生指定兄弟页的引用。

原路径在一个 64 KiB folio 内最多调用 16 次 `cached_page_ref_at()`。每次调用都要重新进入页缓存分片、查询 BTreeMap、锁定 folio 状态并验证。新路径只对需求页执行一次 `page_ref_at()`，其余最多 15 页复用已经固定的 folio。

### 2. 一次验证覆盖整批 PTE 安装

`CachedFile::with_valid_folio_ref()` 在单 inode 的 invalidation 读锁下完成：

1. 查询一次规范 folio；
2. 使用 `Arc::ptr_eq` 确认调用者持有的仍是缓存中的同一对象；
3. 锁定一次 folio 状态；
4. 验证 `!loading && uptodate && !truncated`；
5. 在验证有效期内执行最多 16 个 PTE 的提交闭包。

这里取得的是单 inode 的共享读锁，不是全局写锁；不同文件以及同一文件的其他读侧操作仍可并发。临界区有明确的 16 页上限，里面没有磁盘 I/O、folio 分配或页面复制。

### 3. 删除重复页表查询

原提交路径先 `pt.query(addr)`，随后再调用 `map_4k_concurrent()`。后者本身通过 CAS 返回 `AlreadyMapped`，所以前一次查询既不能消除竞争，也重复执行了页表遍历。

新路径直接尝试原子映射。每个地址仍在对应的 `FilePageRefs` 分片锁下同时发布 PTE 和 folio pin，避免出现“PTE 可见但所有权引用尚未安装”的窗口。

### 4. 保留需求页优先和竞争语义

需求页在批次中排第一。即使某个兄弟页已经被另一线程安装，当前批次也只跳过该可选页；最后重新查询需求页，只有需求页满足访问权限才返回 `Handled`。若 folio 在 execute 与 commit 之间被截断或替换，则整批返回 `Stale`，由上层有限次数重新 prepare。

## 构建前的三遍静态检查

### 第一遍：所有权与正确性

- 检查 `FolioPageRef` 的兄弟引用始终克隆同一个 `Arc<Folio>`；
- 检查页内偏移不会越过 folio；
- 检查每个成功安装的文件页都同步写入 `FilePageRefs`；
- 发现并补上 `installed += 1`，确保成功安装后执行 `mark_mapped()`；
- 删除无法提供竞争保证的预查询。

### 第二遍：锁顺序与错误路径

- invalidation 锁是单 inode 共享读锁；
- folio 状态锁只在一次有界验证和提交期间持有；
- `FilePageRefs` 只锁当前虚拟地址所在分片；
- `AlreadyMapped` 不遗留错误的 folio pin；
- validation 失败、需求页被抢先安装和部分兄弟页安装失败均保持可重试语义。

### 第三遍：边界与性能回归

- 普通 order-4 folio 的缓存/状态查询由最多 16 次降为 1 次；
- 批次严格限制在实际 folio、VMA、文件映射和完整文件页的交集；
- 没有引入进程级或全局锁；
- `cargo fmt --all -- --check` 通过；
- `git diff --check` 通过；
- `vendor/axfs/.cargo-checksum.json` 与修改后的源码一致；
- profiling RISC-V 内核构建通过。

## 测试环境与流程

镜像来源：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
```

本轮重新解压到：

```text
tmp/profile-rv-folio-batch-20260725/sdcard-rv.img
```

QEMU 的关键参数：

```text
-machine virt -m 8G -smp 8 -snapshot
```

采样完整覆盖：

- cagent；
- tg-xtask 从空 target 开始的 446 单元预构建；
- 正式 arceos-helloworld BuildStorm；
- BuildStorm shell 退出和地址空间清理；
- profile 停止及二进制表导出。

采样口径与上一轮一致：

- CPU：10 Hz；
- off-CPU：1/8 抽样并按 8 估算；
- ext4：1/16 抽样并按 16 估算；
- page cache：1/32 抽样并按 32 估算；
- mprotect：1/32；
- 地址空间写锁持有：1/16。

三类表均无丢样：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

## CPU 火焰图结果

### 目标路径

下表的占比以该阶段非 idle CPU 样本为分母。采样次数不同主要来自本轮运行时间缩短，因此同时列出占比和每秒采样率。

| 路径 | 预构建上一轮 | 预构建本轮 | 正式上一轮 | 正式本轮 |
| --- | ---: | ---: | ---: | ---: |
| `handle_page_fault_concurrent` | 31.86% | **28.46%** | 32.10% | **27.67%** |
| `CowFaultPlan::execute` | 11.69% | 11.47% | 12.05% | 11.57% |
| `CowBackend::commit_fault` | 9.14% | **6.11%** | 9.86% | **6.19%** |
| 页缓存 `cached_page_ref_at` | 1.08% | **0%** | 1.14% | **0%** |

按墙钟时间归一化：

| 路径 | 预构建样本/秒变化 | 正式样本/秒变化 |
| --- | ---: | ---: |
| `handle_page_fault_concurrent` | 3.35 → **2.85**（-14.9%） | 2.94 → **2.37**（-19.2%） |
| `CowBackend::commit_fault` | 0.96 → **0.61**（-36.4%） | 0.90 → **0.53**（-41.1%） |

目标改动主要作用在 commit，因此 `execute` 占比基本不变而 commit 明显下降，符合预期。`cached_page_ref_at` 从目标调用栈消失，说明兄弟页不再重复进入页缓存索引。

火焰图：

- [预构建 CPU](./guest-cpu-prebuild.svg)
- [正式阶段 CPU](./guest-cpu-timed.svg)
- [全流程 CPU](./guest-cpu-all.svg)

### 当前新的最大可操作 CPU 路径

本轮正式阶段共有 5,136 个非 idle CPU 样本，主要内核路径为：

| 路径 | 样本 | 非 idle 占比 |
| --- | ---: | ---: |
| `handle_page_fault_concurrent` | 1,421 | 27.67% |
| `NoPreemptIrqSave::release` | 1,255 | 24.44% |
| COW `unmap` | 881 | 17.15% |
| resident reclaim worker | 727 | 14.15% |
| `CowFaultPlan::execute` | 594 | 11.57% |
| `CowBackend::commit_fault` | 318 | 6.19% |
| clone | 250 | 4.87% |
| `flush_tlb` | 199 | 3.87% |
| SBI call | 183 | 3.56% |

`NoPreemptIrqSave::release` 不是一把锁的名字，而是多种 spin lock 退出时共同落到的底层函数。按其上级调用点拆分后，最大的单项是：

- 匿名页 fault 在 `CowFaultPlan::execute` 第 303 行分配并登记 frame；
- 全局物理页分配器；
- COW clone 的 frame 引用计数；
- `unmap_present` 的 frame 引用计数；
- folio frame 分配。

其中匿名页分配/登记在预构建和正式阶段分别占通用 spin-guard release 样本的 16.70% 和 18.43%，是下一轮最适合拆解的目标。

## 等待和 I/O 回归检查

| 指标 | 预构建变化 | 正式阶段变化 | 判断 |
| --- | ---: | ---: | --- |
| folio load wait | -2.75% | -31.26% | 没有回归 |
| page-cache inclusive | -0.75% | -4.32% | 没有回归 |
| 同步块写等待 | -51.89% | -74.03% | 没有回归 |
| mprotect 总耗时 | -6.11% | -8.60% | 工作量相同且下降 |
| exit clear 写锁持有 | -4.69% | -7.10% | 随总时间下降 |
| exec clear 写锁持有 | -3.46% | -8.12% | 随总时间下降 |
| clone parent 写锁持有 | -14.08% | -9.06% | 没有回归 |

ext4 全局锁估算等待在正式阶段从 1.40 秒升到 2.42 秒，但仅有 811/835 个抽样事件，最大单次样本也发生变化；同步写、页缓存、正式阶段 ext4 inclusive 总时间均下降，端到端时间也下降。因此这项目前更像低绝对量下的运行波动，不能据此声称 ext4 退化，也不应在没有第二轮证据时围绕它改代码。

等待火焰图：

- [正式阶段 off-CPU](./guest-offcpu-timed.svg)
- [正式阶段锁等待](./guest-offcpu-lock-timed.svg)
- [正式阶段 ext4 锁等待](./guest-ext4-lock-wait-timed.svg)
- [正式阶段文件系统 I/O](./guest-fs-io-latency-timed.svg)
- [正式阶段 folio load 等待](./guest-folio-load-wait-timed.svg)

## 下一轮修改依据

当前 frame 引用表仍使用：

```text
64 个分片
  -> 每分片一棵 BTreeMap
    -> 每个物理页一个 Arc<SpinNoIrq<FrameRefCnt>>
```

一个新匿名页至少涉及：

1. PCP/物理页分配；
2. frame table 分片锁；
3. BTreeMap 查询和插入；
4. 为 `Arc<SpinNoIrq<FrameRefCnt>>` 单独进行一次内核堆分配；
5. 后续引用变化再取得一次 per-frame spin lock。

下一轮将把表值改成表内的直接引用计数，由分片锁保护短小的 `init/reference/count/drop` 操作，去掉每页一次的 `Arc` 堆分配和第二层 per-frame spin lock。unmap 的 PTE 引用在 TLB flush 前本来就不能减少，因此 deferred 队列只需保存物理地址，不需要额外 `Arc` pin。

该方案不会改回地址空间大锁，也不会把磁盘 I/O、页面清零、复制或整段 unmap 放进 frame-table 锁。clone 的计数预留会在短临界区完成，映射失败时回滚；exclusive COW 只在检查计数并执行现有单 PTE CAS 的短窗口持有对应分片锁。

## 产物

- [summary.json](./summary.json)
- [完整 CPU 火焰图](./guest-cpu-all.svg)
- [完整 off-CPU 火焰图](./guest-offcpu-all.svg)
- [完整锁等待火焰图](./guest-offcpu-lock-all.svg)
- [完整文件系统 I/O 火焰图](./guest-fs-io-latency-all.svg)
- [完整 ext4 锁等待火焰图](./guest-ext4-lock-wait-all.svg)

大体积二进制转储、原始控制台日志和 folded 中间文件由 profiling `.gitignore` 排除；报告、`summary.json` 和 SVG 保留为可提交产物。
