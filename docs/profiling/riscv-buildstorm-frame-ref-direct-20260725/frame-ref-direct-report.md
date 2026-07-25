# RISC-V BuildStorm 直接 frame 引用计数优化报告

## 结论

本轮根据上一轮 CPU 火焰图，将 COW frame 表从：

```text
64 个分片 BTreeMap<PhysAddr, Arc<SpinNoIrq<FrameRefCnt>>>
```

改为：

```text
64 个分片 BTreeMap<PhysAddr, usize>
```

每个物理页不再单独分配 `Arc<SpinNoIrq<...>>`，引用计数的初始化、增加、查询和减少直接在对应 frame-table 分片锁内完成。页面清零、页面复制、文件 I/O 和普通共享页的页表操作均不在分片锁内。

测试结果是“内核目标热点明确下降，但端到端收益不一致”：

| 指标 | folio-batch | frame-ref-direct | 变化 |
| --- | ---: | ---: | ---: |
| tg-xtask 预构建 | 20 分 59 秒 | **20 分 38 秒** | **-1.67%** |
| 正式 Cargo 编译 | 529.39 秒 | 534.76 秒 | +1.01% |
| BuildStorm | 539.38 秒 | 544.64 秒 | +0.98% |
| profile 预构建阶段 | 1260.83 秒 | **1239.52 秒** | **-1.69%** |
| profile 正式阶段（含 shell 退出） | 599.93 秒 | 621.33 秒 | +3.57% |
| profile 三阶段总时间 | 1865.18 秒 | 1865.40 秒 | +0.01% |
| 宿主全流程 | 1876.16 秒 | 1876.27 秒 | +0.01% |

因此不能声称本轮让完整 BuildStorm 稳定变快。它把预构建缩短 21 秒，且目标 CPU/锁路径明显下降；正式 Cargo 单次运行却慢 5.37 秒，并在编译结束后的 shell 退出区间多消耗约 16 秒，最终总时间持平。这需要在后续重复运行中验证方差。

## 为什么原结构昂贵

原先一个新匿名页的生命周期包含两层同步：

1. 取得 64 路 frame-table 分片锁；
2. 在 BTreeMap 中插入一个新条目；
3. 为 `Arc<SpinNoIrq<FrameRefCnt>>` 进行一次内核堆分配；
4. 修改引用时先取得分片锁并克隆 `Arc`；
5. 再取得该页自己的 `SpinNoIrq`；
6. 最后一个引用释放时再回到分片表删除条目。

火焰图无法仅凭 `CowFaultPlan::execute` 调用点区分 PCP 锁和 frame-table 锁，但该调用点、clone 和 unmap 都落在 `NoPreemptIrqSave::release`。去掉每页对象和第二层锁，能够同时减少锁次数、Arc 原子操作和小对象堆分配。

## 新的引用规则

### 新页

`alloc_referenced_frame()` 先从物理页分配器取得 frame，再在分片表中插入计数 1。这个计数由随后安装的 PTE 所有。

### 并发 COW prepare

prepare 在短分片锁内增加一个临时引用：

- 新计数为 2：prepare 时只有 PTE 和当前 plan 两个引用，标记为 `Exclusive`；
- 新计数大于 2：frame 已共享，标记为 `Anonymous` 并在锁外复制。

exclusive commit 再次在分片锁内验证计数仍为 2，并执行现有单 PTE CAS。若 fork 在 prepare 与 commit 之间增加了引用，commit 返回 `Stale`，不会把共享页错误升级为可写。

### fork

fork 在发布子 PTE 前短暂增加一次计数，然后释放 frame-table 锁：

1. 预留子 PTE 的引用；
2. 将父 PTE 改为只读；
3. 安装子 PTE；
4. 任一步失败都回滚预留引用。

因此页表操作不再持有 per-frame 锁，也不会持有 frame-table 分片锁。

### unmap

PTE 被清除后，其原有引用不会立即减少，而是以物理地址保存在 deferred 队列：

1. 清除一批 PTE；
2. 完成 TLB shootdown；
3. 对 deferred 物理地址逐一减少引用；
4. 计数到零时先从 frame 表删除，再在分片锁外归还物理页。

被清除 PTE 的引用本身已经能在 TLB flush 前固定 frame，因此 deferred 队列不需要额外 `Arc` pin。

## 构建前的三遍静态检查

### 第一遍：引用所有权

- 新页初始计数恰好为 1；
- COW plan 的临时引用在成功、stale 和错误路径均由 `Drop` 释放；
- fork 两个失败出口都回滚预留计数；
- unmap 在 TLB flush 前不减少 PTE 引用；
- 最后一个引用先从表中删除，再在分片锁外归还物理页；
- 旧的 `FrameRefCnt`、`get_frame_ref` 和 `Arc<SpinNoIrq<...>>` 已全部消失。

### 第二遍：锁顺序和竞争

- `drop_referenced_frame` 的临时 guard 在调用物理页回收前已经析构；
- fork 不持有 frame-table 锁执行父/子页表更新；
- 共享页复制前显式释放 frame-table guard；
- exclusive 权限升级与 fork 引用预留由同一分片锁串行化；
- concurrent commit 只在短分片锁下执行单 PTE CAS，不分配页表、不读取文件；
- 没有 frame-table → 物理页分配器 → frame-table 的递归路径。

第二遍还发现并消除了 exclusive 旧路径中“两次读取同一分片”的写法，改成一次加锁完成计数检查和权限升级。

### 第三遍：性能边界和一致性

- `alloc_referenced_frame`、`copy_nonoverlapping` 和 `file.read_at` 均在显式 frame-table guard 外；
- frame 表仍有 64 个分片，不引入全局引用锁；
- `cargo fmt --all -- --check` 通过；
- `git diff --check` 通过；
- axfs vendor 校验和匹配；
- 三遍完成后才执行 profiling 内核构建，构建通过。

## 测试方法

本轮再次从以下官方压缩镜像独立解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
```

测试镜像：

```text
tmp/profile-rv-frame-ref-direct-20260725/sdcard-rv.img
15032385536 bytes
```

运行时没有其他 QEMU，不使用 Docker，使用 8 GiB、8 vCPU 和 `-snapshot`。采样完整覆盖 cagent、空 target 预构建、正式 BuildStorm 和进程退出。

采样率与上一轮完全相同，且：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

## CPU 火焰图

占比以阶段内非 idle CPU 样本为分母。

| 路径 | 预构建上一轮 | 预构建本轮 | 正式上一轮 | 正式本轮 |
| --- | ---: | ---: | ---: | ---: |
| `handle_page_fault_concurrent` | 28.46% | **27.89%** | 27.67% | **25.29%** |
| `CowFaultPlan::execute` | 11.47% | **9.81%** | 11.57% | **9.55%** |
| `CowBackend::commit_fault` | 6.11% | 6.86% | 6.19% | 6.73% |
| COW unmap | 10.79% | 10.51% | 10.32% | 10.02% |
| resident reclaim worker | 14.70% | 13.97% | 14.15% | 13.40% |
| clone | 3.87% | **3.27%** | 4.79% | **3.07%** |
| `NoPreemptIrqSave::release` | 25.52% | **23.10%** | 24.90% | **19.45%** |

按墙钟时间归一化后：

| 路径 | 预构建样本/秒 | 正式样本/秒 |
| --- | ---: | ---: |
| `CowFaultPlan::execute` | 1.148 → **0.919**（-19.9%） | 0.990 → **0.795**（-19.7%） |
| `NoPreemptIrqSave::release` | 2.555 → **2.165**（-15.3%） | 2.132 → **1.619**（-24.1%） |
| clone | 0.388 → **0.307**（-20.9%） | 0.410 → **0.256**（-37.6%） |

这证明目标同步成本确实下降，没有只是因为本轮时间更短而少采到样本。

火焰图：

- [预构建 CPU](./guest-cpu-prebuild.svg)
- [正式阶段 CPU](./guest-cpu-timed.svg)
- [全流程 CPU](./guest-cpu-all.svg)

## 地址空间持锁结果

| 写锁持有事件 | 预构建变化 | 正式变化 |
| --- | ---: | ---: |
| clone parent | **-15.86%** | **-23.55%** |
| clone child | **-26.01%** | **-29.95%** |
| exit clear | **-16.89%** | **-11.54%** |
| munmap | **-13.25%** | -2.08% |
| exec clear | -2.97% | +2.56% |

clone 的 sampled count 基本相同，而总持锁时间和最大单次时间同时下降。例如正式阶段 child clone 从 50.69 秒降到 35.51 秒，最大样本从 203.56 ms 降到 80.25 ms。这与 fork 不再持有 per-frame 锁执行页表操作一致。

相关火焰图：

- [clone parent](./guest-aspace-clone-parent-write-hold-timed.svg)
- [clone child](./guest-aspace-clone-child-write-hold-timed.svg)
- [exit clear](./guest-aspace-exit-clear-write-hold-timed.svg)

## 为什么正式端到端没有变快

正式阶段的目标 CPU 路径和地址空间持锁都下降，但这次运行同时出现：

- 正式 off-CPU 估算总量 +31.04%；
- ext4 inclusive 估算总量 39.65 秒 → 51.87 秒；
- 同步块写等待 0.092 秒 → 0.401 秒；
- folio load wait 0.405 秒 → 0.594 秒。

其中块写和 folio 等待的绝对量仍小，ext4/off-CPU 是跨任务累计值，不能直接相加到墙钟时间；但它们说明本轮正式阶段的文件系统等待环境比上一轮更差。另一方面，正式 Cargo 只慢 5.37 秒，而 `timed` phase 多 21.40 秒，剩余差异主要出现在 BuildStorm 输出结束后的 shell/进程退出区间。

因此目前最严谨的判断是：

- frame 引用表优化对其目标路径有效；
- 预构建有 1.67% 的端到端改善；
- 正式阶段单次结果落在相反方向，尚不能证明稳定收益；
- 全流程总时间持平，下一轮不能把它描述成整体提速。

等待火焰图：

- [正式 off-CPU](./guest-offcpu-timed.svg)
- [正式锁等待](./guest-offcpu-lock-timed.svg)
- [正式 ext4](./guest-ext4-lock-wait-timed.svg)
- [正式 I/O](./guest-fs-io-latency-timed.svg)

## 下一轮依据

去掉 per-frame 锁后，`NoPreemptIrqSave::release` 仍是正式阶段 19.45% 的非 idle CPU 样本。其最大的明确调用来源为：

- 匿名页 `CowFaultPlan::execute`：133 个 leaf 样本；
- 全局 Rust heap 分配：124；
- folio frame 分配：85；
- COW unmap：82；
-普通 frame 分配：52；
- unmap 批量释放：43；
- clone：40。

匿名页和 folio 分配都会经过 `axalloc` 的 per-CPU page cache。该 PCP 只有两个生产调用方：

```text
kernel COW/匿名页
axfs folio/page cache
```

两者都在普通任务上下文，当前却使用 `SpinNoIrq`，每次命中本地 PCP 仍要保存/关闭/恢复本地中断。下一轮应在完整调用点审查后，把 PCP 锁改为 `SpinNoPreempt`：仍禁止持锁任务被调度或迁移，但不为不会从中断上下文进入的缓存路径支付 IRQ save/restore 成本。

frame table 同样只从页故障、fork、unmap 和回收任务访问，也可评估改成 `SpinNoPreempt`。两类锁必须分别审核，不能仅因为 CPU 火焰图宽就批量替换。

## 产物

- [summary.json](./summary.json)
- [完整 CPU 火焰图](./guest-cpu-all.svg)
- [完整 off-CPU 火焰图](./guest-offcpu-all.svg)
- [完整锁等待火焰图](./guest-offcpu-lock-all.svg)
- [完整 I/O 火焰图](./guest-fs-io-latency-all.svg)
