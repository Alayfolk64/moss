# RISC-V PCP fast-hit IRQ-only guard 迭代报告

## 结论

本轮修改有效消除了 order-0 用户页 PCP 命中路径中不必要的 preempt 计数更新和 `enable_preempt` 回调。

在相同官方镜像、8 GiB、8 vCPU、`-snapshot` 和完整 profiler 口径下：

| 指标 | Zicboz 基线 | PCP IRQ-only fast hit | 变化 |
| --- | ---: | ---: | ---: |
| Cargo 预构建 | 18m37s | 18m43s | +6s |
| prebuild phase | 1118.045 s | 1124.224 s | +6.178 s（+0.55%） |
| Cargo release | 8m13s | **8m11s** | -2s |
| BuildStorm | 507.87 s | **506.28 s** | -1.59 s（-0.31%） |
| timed phase | 572.289 s | 572.926 s | +0.637 s（+0.11%） |
| 宿主全流程 | 1711.345 s | 1711.306 s | -0.039 s |

墙钟结果是混合的：

- BuildStorm 自身快 1.59 秒；
- prebuild 慢 6 秒；
- timed phase 基本持平；
- 宿主全流程几乎完全相同。

因此不能仅凭端点时间声称整体加速。目标 CPU 栈的变化更明确：

| CPU 路径 | 基线预构建 | 新版预构建 | 变化 | 基线正式 | 新版正式 | 变化 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `alloc_frame -> NoPreemptIrqSave::release` | **147** | **4** | -97.3% | **56** | **1** | -98.2% |
| `alloc_frame` 全部栈顶工作 | 147 | **112** | -23.8% | 57 | **42** | -26.3% |
| 全内核 `NoPreemptIrqSave::release` 叶节点 | 452 | **308** | -31.9% | 212 | **192** | -9.4% |

新版 112/42 个 `alloc_frame` 栈顶样本中，108/41 个落在内联后的 `GlobalAllocator::alloc_page_cached` 本体。也就是说，目标工作没有被简单换成另一个 `release` 符号：

```text
旧版：
alloc_frame
  -> alloc_page_cached
    -> NoPreemptIrqSave::release

新版：
alloc_frame
  -> alloc_page_cached
```

把新的 fast-path 本体也完整算入后，仍下降 23.8% 和 26.3%。这是保留本轮修改的主要证据。

## 热点来源

上一轮正式 CPU 图中，真正以 `NoPreemptIrqSave::release` 为栈顶的 212 个样本按直接调用者拆分为：

| 直接调用者 | 样本 |
| --- | ---: |
| 地址空间 `alloc_frame` | **56** |
| 用户返回循环 | 49 |
| Rust heap `dealloc` | 29 |
| `WaitQueue::wait_until` | 15 |
| `UnmapGather::finish` | 14 |
| Rust heap `realloc` | 13 |
| `Arc::drop` | 10 |
| 其他 | 26 |

此前报告引用的 268 是包含 `release` 的全部栈，不代表 PC 正在 `release` 内。只有 212 个叶节点样本能用于判断释放函数自身的 CPU 开销。本轮先修最大的单一路径 56。

## 原实现为什么昂贵

每个 4 KiB 匿名页或 COW 私有页通过：

```text
alloc_frame
  -> GlobalAllocator::alloc_page_cached
    -> page_caches[cpu].lock()
```

PCP 槽原来是：

```rust
SpinNoIrq<PerCpuPageCache>
```

它使用 `NoPreemptIrqSave`，一次 cache hit 会执行：

```text
关闭本地中断
读取当前任务
增加 preempt_disable_count
取得 PCP 自旋锁
从链表 pop 一页
释放 PCP 自旋锁
恢复本地中断
再次读取当前任务
减少 preempt_disable_count
检查 need_resched
必要时进入调度器
```

PCP hit 的临界区本身只有：

```text
读取 head
读取空闲页首字中的 next
更新 head/count
```

这条路径不会睡眠、不会主动调度、不会取得全局页分配器。因此只要本地中断保持关闭，当前任务不会在临界区中被 timer interrupt 抢占并迁移。额外维护 preempt 计数会把一个很短的本地 pop 扩大为高频任务状态访问。

## 最终实现

### 1. 为 kspin 增加 SpinIrq

`vendor/kspin/src/lib.rs` 增加：

```rust
pub type SpinIrq<T> = BaseSpinLock<IrqSave, T>;
```

它仍使用同一个原子自旋锁，只把 guard 从：

```text
NoPreemptIrqSave
```

换成：

```text
IrqSave
```

`BaseSpinLockGuard::drop` 的顺序是：

```text
先用 Release store 解开自旋锁
再恢复本地中断
```

因此中断重新打开时，PCP 锁已经不再持有。

### 2. fast hit 与 slow path 分离

`PerCpuPageCache::try_allocate` 只处理：

- PCP limits 已按当前总页数和 CPU 数配置；
- 对应 `UsageKind` 链表非空；
- 从本地链表 pop 一页。

它明确不执行：

- `update_limits`；
- `refill`；
- `drain`；
- 全局页分配；
- 睡眠或调度。

若任一 fast 条件不满足，立即返回 `None`，进入独立 slow 函数。

### 3. slow refill 保留抢占保护

`refill_page_cache` 在取得 IRQ-only PCP 锁之前显式创建：

```rust
NoPreemptIrqSave::new()
```

因此调用链：

```text
PCP lock
  -> refill/update_limits
    -> global palloc lock
```

仍在外层 preempt disable count 大于零的状态下执行。内层 `SpinNoIrq` 释放时只能把嵌套计数减回一，不能在外层 PCP 锁仍持有时调度。

### 4. free 与全局 drain 保留重 guard

`dealloc_page_cached` 可能因为超过 `high` 而进入 batch drain；`drain_page_caches` 本身会逐槽把页面送回全局 allocator。这两条路径都显式保留 `NoPreemptIrqSave`。

本轮只优化最常见的已配置 cache hit，没有把 refill、free、pressure drain 误当成纯本地操作。

### 5. OOM 重试顺序未改变

最终慢路径保持原语义：

```text
本地 refill
  成功 -> 返回
  失败 -> drain 所有 PCP
           -> 本地 refill 重试
                成功 -> 返回
                失败 -> 单页全局 alloc_pages
```

正常 cache miss 不会先 drain 其他 CPU；只有本地 refill 真实失败才执行全局 drain。

## 三遍静态审查

### 第一遍：语义与慢路径

第一遍审查没有直接通过，而是发现并修正了两个问题。

#### 问题一：嵌套锁期间可能调度

最初草案只把整个 PCP 锁直接改成 `SpinIrq`。但是 slow refill 会在 PCP 锁内取得全局 `palloc: SpinNoIrq`：

```text
SpinIrq PCP
  -> SpinNoIrq palloc
```

若没有外层 preempt 计数，内层 `SpinNoIrq` 释放时可能触发 reschedule，而 PCP 锁仍被当前任务持有。该草案没有进入构建，已改成 slow path 外层显式 `NoPreemptIrqSave`。

#### 问题二：正常 miss 误 drain 全部 PCP

最初拆分后曾把所有 fast miss 直接接到旧 slow 函数开头的 `drain_page_caches`。这会让正常本地缓存耗尽也先清空所有 CPU 的 PCP，破坏批量缓存和多核局部性。

最终顺序改为“本地 refill 先行，只有 refill 失败才全局 drain”，与原算法一致。

修正后重新从头检查：

- hit 只 pop 一页；
- miss 不改变 OOM 重试顺序；
- usage accounting 仍由 refill/global allocator 成对维护；
- `page_cache_counts` 在每次锁内变更后更新；
- 不修改 batch/high 动态策略；
- 不修改 `UsageKind` 分链；
- 不修改全局 balloc/palloc。

### 第二遍：中断、抢占、迁移与锁序

逐项确认：

- `SpinIrq::lock` 在尝试原子锁之前关闭本地中断；
- 临界区没有 schedule/yield/wait；
- fast path 没有全局 allocator 回调；
- 自旋锁在中断恢复前解开；
- slow refill、free、drain 都先建立 `NoPreemptIrqSave` 外层 guard；
- 嵌套 palloc 释放不能把 preempt count 降到零；
- PCP -> palloc 的既有锁序没有反转；
- remote drain 仍通过同一原子 PCP 锁互斥；
- IRQ handler 不能在本 CPU fast path 中重入 PCP；
- `cpu_id` 在 guard 前传入的既有行为未扩大；即便极短窗口发生迁移，原子 PCP 锁仍保证安全，只可能暂时失去局部性。

### 第三遍：机械完整性

以下全部通过后才开始构建：

- `rustfmt --check`，分别按 kspin 2021 和 axalloc 2024 edition；
- `cargo metadata --offline --no-deps`；
- `git diff --check`；
- README 未修改；
- `kspin/src/lib.rs` vendor checksum 一致；
- `axalloc/src/lib.rs` vendor checksum 一致；
- `axalloc/src/pcp.rs` vendor checksum 一致；
- 之前修改的 `axallocator` 和 `bitmap-allocator` checksum 仍一致。

## 全量构建与反汇编

包级 `cargo clean -p` 没有命中自定义 vendored target，因此最终使用：

```text
cargo clean --target-dir /home/wuxun/Projects/moss/target
make PROFILE=y kernel-rv
```

这只清理 Moss 宿主构建目录，不触碰 guest 官方镜像和 `/work/tgoskits/target`。

全量日志明确重新编译：

- `kernel_guard`;
- `kspin`;
- `axalloc`;
- `starry-kernel`;
- `starryos`。

| 项目 | 值 |
| --- | --- |
| 全量构建耗时 | 18.11 s |
| `kernel-rv` SHA256 | `ce430276dc9bc2b5af72e30916b9d2ed310d43ab1fbdd965e3cd02826e030f38` |
| `kernel-rv.profile.elf` SHA256 | `19e811b6ce54e8be27a78d75bdac095ef58561f57feddf9e5c04bfce41e82054` |

反汇编中 `GlobalAllocator::alloc_page_cached` 的 fast return 为：

```text
Release store 解开 PCP 自旋锁
csrs sstatus, saved_irq_state
返回
```

fast 分支没有调用 `KernelGuardIf::enable_preempt`。miss 分支跳到独立的 `alloc_page_cached_slow`，后者仍能看到 `NoPreemptIrqSave` guard。

## 新鲜镜像与运行窗口

每轮都从 `/home/wuxun/Downloads/sdcard-rv-pub.img.gz` 重新解压：

| 项目 | 值 |
| --- | --- |
| 解压路径 | `tmp/profile-rv-pcp-irqfast-20260725/sdcard-rv.img` |
| 大小 | 15,032,385,536 字节 |
| SHA256 | `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0` |
| QEMU 写入方式 | `-snapshot` |

哈希也保存于 [disk.sha256](disk.sha256)。

上一轮开始时间是 2026-07-25 21:48:07 +08:00。本轮在 22:48:35 启动，间隔超过一小时，且启动前确认没有其他 QEMU。

| 项目 | 值 |
| --- | --- |
| 开始 | 2026-07-25 22:48:35 +08:00 |
| 结束 | 2026-07-25 23:17:06 +08:00 |
| 宿主全流程 | 1711.306 s |
| profile 导出 | GDB 第 1 次成功 |

## 完整测试口径

QEMU：

```text
RISC-V virt
8 GiB
8 vCPU
-snapshot
```

profiler：

| 类型 | 口径 |
| --- | --- |
| CPU | 10 Hz |
| off-CPU | 1/8 抽样，记录权重乘 8 |
| ext4 | 1/16 抽样，记录权重乘 16 |
| page cache | 1/32 抽样，记录权重乘 32 |
| mprotect | 1/32 抽样，记录权重乘 32 |
| 地址空间写锁持有 | 1/16 抽样，记录权重乘 16 |

完整性：

| 项目 | 值 |
| --- | ---: |
| setup phase | 3.763 s |
| prebuild phase | 1124.224 s |
| timed phase | 572.926 s |
| CPU 样本 | 52,306 |
| `dropped_cpu` | 0 |
| `dropped_wait` | 0 |
| `dropped_pending` | 0 |
| `skipped_cpu` | 1 |
| SVG | 112 张 |

`skipped_cpu=1` 表示一次 10 Hz timer 样本因为该 CPU 的 profiler buffer 正被占用而跳过。相对 52,306 个样本约为 0.0019%，不会改变 147→4 或 56→1 的结论，但本轮完整性不是绝对零跳过，报告如实保留。

cagent 10/10 通过，BuildStorm 正式编译成功，输出 1,681,000 字节。已有 `BUILDSTORM_MINIBUILD fail` 不影响后续 tg-xtask 预构建和正式编译。

## 历次时间总表

| 指标 | 水位总页数 | Cow 内联槽 | bitmap next-fit | 原子 usage | Zicboz 清零 | PCP IRQ fast |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| setup phase | 3.962 s | 3.746 s | 3.908 s | 3.794 s | 3.668 s | 3.763 s |
| prebuild phase | 1161.134 s | 1166.185 s | 1143.762 s | 1137.340 s | **1118.045 s** | 1124.224 s |
| Cargo 预构建 | 19m20s | 19m25s | 19m02s | 18m56s | **18m37s** | 18m43s |
| timed phase | 580.015 s | 583.185 s | 579.563 s | 574.776 s | **572.289 s** | 572.926 s |
| Cargo release | 8m15s | 8m20s | 8m16s | 8m14s | 8m13s | **8m11s** |
| BuildStorm | 510.37 s | 515.89 s | 511.76 s | 509.12 s | 507.87 s | **506.28 s** |
| 宿主全流程 | 1756.302 s | 1771.332 s | 1741.349 s | 1726.348 s | 1711.345 s | **1711.306 s** |

本轮 BuildStorm 是表内单次最好值，但 prebuild 不是。由于只有一次运行，不能把 1.59 秒当作稳定置信区间。

## CPU 火焰图分析

### 目标路径

| 阶段 | 旧 preempt release | 新 preempt release | 新 fast-path 本体 | 旧目标总宽度 | 新目标总宽度 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 预构建 | 147 | 4 | 108 | 147 | **112** |
| 正式 | 56 | 1 | 41 | 57 | **42** |

旧正式目标总宽度为 57 而不是 56，是因为还有 1 个样本落在相邻的 guard hook。新总宽度按 `alloc_frame` 的所有栈顶调用计算，不只挑有利符号。

slow path 仍保留 4/1 个 `NoPreemptIrqSave::release` 样本，符合设计：cache miss/refill 必须保留重 guard。

### 全局 release

| 阶段 | Zicboz 基线 | PCP IRQ fast | 变化 |
| --- | ---: | ---: | ---: |
| 预构建 | 452 | 308 | -144（-31.9%） |
| 正式 | 212 | 192 | -20（-9.4%） |

预构建减少 144，接近目标路径减少的 143；正式减少 20，小于目标减少的 55，说明其他 release 调用者在这次运行中增加了约 35 个样本。不能把全部全局变化只归因于 PCP，但目标直接父路径的消失是确定的。

### 非 idle CPU

| 阶段 | Zicboz 基线 | PCP IRQ fast | 变化 |
| --- | ---: | ---: | ---: |
| 预构建 | 7713 | 7729 | +16（+0.21%） |
| 正式 | 3607 | 3556 | -51（-1.41%） |

预构建非 idle CPU 几乎相同，正式略降。墙钟与 CPU 样本都没有显示优化导致新的 CPU 膨胀。

### 当前正式阶段主要叶节点

| 叶节点 | 样本 | 判断 |
| --- | ---: | --- |
| `CurrentTask::from_raw_ptr` | 262 | 既有 direct-current 迭代没有端点收益 |
| ext4 `crc32` | 252 | 已有 unroll8；继续盲目展开风险高 |
| `NoPreemptIrqSave::release` | 192 | 仍需按直接调用者继续拆分 |
| `UserContext::run` | 185 | 用户态采样边界，不等于该函数自身开销 |
| page-fault inner | 138 | 包含多种 fault plan，必须继续拆分 |
| BTree `find_key_index` | 90 | COW/file page 结构热点，HashMap 替换已有失败记录 |
| `alloc_page_cached` | 46 | 其中 41 来自本轮目标 `alloc_frame` |
| `zero_fresh_frame` | 19 | 上轮优化保持有效 |

PCP 已从主要 `release` 子热点降到 42 个目标样本，不应继续在没有新证据时扩大改动范围。

## I/O 与锁等待

### 文件系统

| 事件 | 基线预构建 | 新版预构建 | 变化 | 基线正式 | 新版正式 | 变化 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 同步文件系统 I/O | 127.311 s | 139.692 s | +12.381 s | 69.646 s | 73.085 s | +3.439 s |
| ext4 锁等待 | 3.385 s | 4.286 s | +0.901 s | 0.892 s | 0.809 s | -0.083 s |
| folio 加载等待 | 3.099 s | 3.037 s | -0.062 s | 0.342 s | 0.325 s | -0.017 s |

本轮不修改文件系统。预构建同步 I/O 增加 12.4 秒，与 prebuild 墙钟慢 6 秒方向一致，说明端点退化不能归因于 PCP CPU 优化。正式同步 I/O 也增加 3.4 秒，而 BuildStorm 仍快 1.59 秒；这与目标 CPU 路径下降相容，但仍不足以量化稳定收益。

### 地址空间写锁累计持有

| 事件 | 基线预构建 | 新版预构建 | 基线正式 | 新版正式 |
| --- | ---: | ---: | ---: | ---: |
| clone child | 52.961 s | 52.909 s | 20.355 s | 22.129 s |
| clone parent | 29.170 s | 25.037 s | 9.742 s | 11.072 s |
| exit clear | 50.667 s | 51.514 s | 21.439 s | 22.352 s |
| exec clear | 27.431 s | 26.934 s | 9.660 s | 9.587 s |
| mprotect | 48.448 s | 48.776 s | 21.001 s | 21.041 s |
| munmap | 24.845 s | 26.925 s | 8.772 s | 9.417 s |

这些值有升有降，mprotect/exec 基本相同。PCP 修改没有改变地址空间写锁粒度。

mprotect 工作量也保持同一量级：

| 阶段 | 基线调用 | 新版调用 | 变化 |
| --- | ---: | ---: | ---: |
| 预构建 | 1,465,614 | 1,460,597 | -0.34% |
| 正式 | 668,249 | 665,875 | -0.36% |

## 火焰图与原始数据

- [完整 CPU](guest-cpu-all.svg)
- [预构建 CPU](guest-cpu-prebuild.svg)
- [正式 CPU](guest-cpu-timed.svg)
- [完整 off-CPU](guest-offcpu-all.svg)
- [预构建 off-CPU](guest-offcpu-prebuild.svg)
- [正式 off-CPU](guest-offcpu-timed.svg)
- [正式锁等待](guest-offcpu-lock-timed.svg)
- [正式同步文件系统 I/O](guest-fs-io-latency-timed.svg)
- [正式 ext4 锁等待](guest-ext4-lock-wait-timed.svg)
- [正式 folio 加载等待](guest-folio-load-wait-timed.svg)
- [正式 clone child 写锁](guest-aspace-clone-child-write-hold-timed.svg)
- [正式 clone parent 写锁](guest-aspace-clone-parent-write-hold-timed.svg)
- [正式 exit clear 写锁](guest-aspace-exit-clear-write-hold-timed.svg)
- [正式 exec clear 写锁](guest-aspace-exec-clear-write-hold-timed.svg)
- [正式 mprotect 写锁](guest-aspace-mprotect-write-hold-timed.svg)
- [正式 munmap 写锁](guest-aspace-munmap-write-hold-timed.svg)
- [运行元数据](run.json)
- [渲染摘要](summary.json)
- [符号化 profile](profile.log)

## 本轮决策

保留本轮修改，理由按证据强弱排序：

1. `alloc_frame -> NoPreemptIrqSave::release` 在两个阶段下降 97% 以上；
2. 把新 fast-path 本体也计入后，整个目标栈仍下降 24%–26%；
3. slow path 保留的 4/1 个重 guard 样本符合实现设计；
4. 全内核 release 叶节点也下降；
5. cagent 和 BuildStorm 完整通过；
6. BuildStorm 单次快 1.59 秒，但只作为辅助证据；
7. prebuild 慢 6 秒且 I/O 增加，报告不宣称预构建加速。

下一轮应重新拆分新版剩余 192 个正式 `NoPreemptIrqSave::release` 叶节点，选择新的最大直接调用者。不能继续把已降到 42 个样本的 PCP 当作最大热点。
