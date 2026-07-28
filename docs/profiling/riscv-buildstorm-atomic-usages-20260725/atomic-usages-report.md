# RISC-V 原子 usage 统计迭代报告

## 结论

本轮修改有效消除了 `GlobalAlloc::alloc` 中第二把全局 IRQ-off 统计锁。

在相同官方镜像、8 GiB、8 vCPU、`-snapshot` 和完整 profiler 口径下：

| 指标 | bitmap next-fit 基线 | 原子 usage 统计 | 变化 |
| --- | ---: | ---: | ---: |
| Cargo 预构建 | 19m02s | 18m56s | -6s |
| prebuild phase | 1143.762 s | 1137.340 s | -6.422 s（-0.56%） |
| Cargo release | 8m16s | 8m14s | -2s |
| BuildStorm | 511.76 s | **509.12 s** | -2.64 s（-0.52%） |
| timed phase | 579.563 s | 574.776 s | -4.787 s（-0.83%） |
| 宿主全流程 | 1741.349 s | **1726.348 s** | -15.001 s（-0.86%） |

目标 CPU 路径的变化比端点时间更明确：

| CPU delayed leaf | 基线预构建 | 原子统计预构建 | 基线正式 | 原子统计正式 |
| --- | ---: | ---: | ---: | ---: |
| `GlobalAlloc::alloc -> NoPreemptIrqSave::release` | **272** | **0** | **139** | **0** |
| 全部非 idle `NoPreemptIrqSave::release` | 775 | 475 | 360 | 223 |

`272/139 -> 0/0` 直接证明每次堆分配不再取得 `usages: SpinNoIrq<Usages>`。全部 allocator release 叶样本也分别下降 38.7% 和 38.1%。

BuildStorm 的 2.64 秒改善只有 0.52%，仍可能包含单轮 QEMU 波动；但目标调用栈彻底消失，说明局部优化成立。本轮 `509.12 s` 也比此前水位总页数的单次最佳 `510.37 s` 快 1.25 秒。

## 修改动机

next-fit 迭代消除高阶 folio 位图扫描后，`NoPreemptIrqSave::release` 下最大的直接父路径变成：

```text
GlobalAlloc::alloc -> NoPreemptIrqSave::release
```

样本为：

- 预构建 272；
- 正式 139。

原实现的成功分配路径为：

```text
取得 balloc: SpinNoIrq<TLSF>
    TLSF allocate
    取得 usages: SpinNoIrq<Usages>
        RustHeap += size
    释放 usages
释放 balloc
```

usage 计数只用于 `/proc/meminfo`、`/proc/meminfo2` 和 `sys_sysinfo`，不参与内存所有权、OOM 决策、页回收或同步。为诊断计数再取得一把全局 IRQ-off 锁，会让所有小对象分配多一次锁操作，并延长 TLSF 锁的持有时间。

## 实现

### 1. 分类别原子计数

保留公开快照类型：

```rust
pub struct Usages([usize; UsageKind::VARIANTS.len()]);
```

内部存储改为：

```rust
struct AtomicUsages([AtomicUsize; UsageKind::VARIANTS.len()]);
```

每种 usage 独立进行：

```text
alloc   -> fetch_add
dealloc -> fetch_sub
```

### 2. 使用 Relaxed 内存序

这些原子只记录诊断数值，不发布：

- TLSF block 所有权；
- 物理页所有权；
- 页表内容；
- page cache 状态；
- 其他受保护数据。

因此不需要 Acquire/Release 建立跨 CPU happens-before，`Ordering::Relaxed` 足以保证单个计数不撕裂且加减不丢失。

`usages()` 逐项 load 后返回快照。不同类别可能来自相差很短的时刻，这与诊断统计用途一致；现有消费者只读取 PageCache 或打印全部值，不依赖跨类别事务一致性。

### 3. TLSF 锁外计数

成功分配后明确：

```rust
drop(balloc);
self.usages.alloc(...);
```

分配结果仍在计数完成后才返回调用者，但全局 TLSF 锁不再覆盖统计更新。

释放顺序改为：

```text
先实际释放 TLSF block/物理页
再原子减少 usage
```

快照在并发期间本来就是瞬时值；该顺序保证计数不会先宣布资源已经释放、实际 allocator 却仍未完成释放。

### 4. 所有计数路径统一

以下路径全部改为相同原子接口：

- level-1 堆分配；
- level-2 TLSF 堆分配；
- 普通连续页；
- 指定地址页分配；
- PCP 批量补充；
- PCP 批量回收；
- `dealloc_pages`；
- `usages()` 快照。

RustHeap 用于 TLSF 扩容的物理页仍不重复计数，原有 accounting 语义未改变。

## 三遍静态审查

### 第一遍：语义与成对计数

检查了：

- 只有分配成功才加计数；
- 只有实际释放完成才减计数；
- RustHeap TLSF 扩容页不重复计入；
- PCP 每个 UsageKind 的补充和回收按页数成对；
- level-1 与 level-2 均覆盖；
- 指定地址分配与普通分配一致；
- `/proc` 和 `sysinfo` 消费者不要求跨类别一致快照；
- `fetch_sub` 保留 debug 下的 underflow 检查。

### 第二遍：并发、内存序和锁序

检查结果：

- 单类别并发加减不会丢失；
- usage 原子不承担所有权发布，Relaxed 足够；
- `balloc`、`palloc` 和 PCP 锁序没有改变；
- 删除了成功堆分配中的嵌套 usage 锁；
- TLSF 锁在原子计数前明确释放；
- 多类别位于同一 cache line，但旧实现本来就让所有类别竞争同一锁；新实现没有扩大共享范围；
- 原子操作在 RISC-V `A` 扩展和 LoongArch 目标上均受当前工具链支持。

### 第三遍：机械完整性

以下全部通过：

- `cargo fmt --all -- --check`
- `cargo metadata --offline --no-deps`
- `git diff --check`
- 全仓确认没有残留 `usages.lock()`
- `README.md` 未修改
- `axalloc`、`axallocator`、`bitmap-allocator` vendor 校验和一致

## 构建与镜像

显式清理 `axalloc` 和 `starry-kernel`，构建日志确认重新编译 `axalloc`。

| 项目 | 值 |
| --- | --- |
| 构建命令 | `make PROFILE=y kernel-rv` |
| 构建耗时 | 12.81 s |
| `kernel-rv` SHA256 | `f010f00cfaa9af45d0ea44fab206ad35ff2e702460af097119cb69e3b8ff62b2` |
| `kernel-rv.profile.elf` SHA256 | `06b77a06da10062e6a84bed65c5ce02dfe1f85fa201eb21f3d0c65132e12be6b` |

每轮都从 `/home/wuxun/Downloads/sdcard-rv-pub.img.gz` 重新解压：

| 项目 | 值 |
| --- | --- |
| 解压大小 | 15,032,385,536 字节 |
| SHA256 | `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0` |

上一轮在 19:45:42 启动，本轮严格等到 20:45:58 才启动，满足一小时只运行一次完整测试。启动前没有其他 QEMU。

## 完整测试口径

QEMU：

```text
RISC-V virt
8 GiB
8 vCPU
-snapshot
```

profiler 口径与基线完全一致：

| 类型 | 口径 |
| --- | --- |
| CPU | 10 Hz |
| off-CPU | 1/8 抽样，权重乘 8 |
| ext4 | 1/16 抽样，权重乘 16 |
| page cache | 1/32 抽样，权重乘 32 |
| mprotect | 1/32 抽样，权重乘 32 |
| 地址空间写锁持有 | 1/16 抽样，权重乘 16 |

完整性：

| 项目 | 值 |
| --- | ---: |
| setup phase | 3.794 s |
| prebuild phase | 1137.340 s |
| timed phase | 574.776 s |
| 宿主全流程 | 1726.348 s |
| CPU 样本 | 54,604 |
| `dropped_cpu` | 0 |
| `dropped_wait` | 0 |
| `dropped_pending` | 0 |
| `skipped_cpu` | 0 |
| GDB 导出 | 第 1 次成功 |
| SVG | 112 张 |

cagent 10/10 通过，BuildStorm 正式编译成功。`BUILDSTORM_MINIBUILD fail` 与前轮相同，后续预构建和正式编译不受影响。

## 时间总表

| 指标 | 水位总页数 | Cow 内联槽 | bitmap next-fit | 原子 usage |
| --- | ---: | ---: | ---: | ---: |
| setup phase | 3.962 s | 3.746 s | 3.908 s | **3.794 s** |
| prebuild phase | 1161.134 s | 1166.185 s | 1143.762 s | **1137.340 s** |
| Cargo 预构建 | 19m20s | 19m25s | 19m02s | **18m56s** |
| timed phase | 580.015 s | 583.185 s | 579.563 s | **574.776 s** |
| Cargo release | 8m15s | 8m20s | 8m16s | **8m14s** |
| BuildStorm | 510.37 s | 515.89 s | 511.76 s | **509.12 s** |
| 宿主全流程 | 1756.302 s | 1771.332 s | 1741.349 s | **1726.348 s** |

原子 usage 是当前表中 prebuild、timed、BuildStorm 和宿主全流程的单次最好结果。差距仍小于 1%，后续运行必须继续观察，不能将其当成稳定置信区间。

## CPU 火焰图分析

### 非 idle 样本

| 阶段 | bitmap next-fit | 原子 usage | 变化 |
| --- | ---: | ---: | ---: |
| 预构建 | 8382 | 8255 | -127（-1.52%） |
| 正式 | 3840 | 3777 | -63（-1.64%） |

### 目标路径

| 路径 | 基线预构建 | 原子统计预构建 | 基线正式 | 原子统计正式 |
| --- | ---: | ---: | ---: | ---: |
| `GlobalAlloc::alloc -> release` | **272** | **0** | **139** | **0** |
| `GlobalAlloc::dealloc -> release` | 23 | 0 | 14 | 0 |
| 全部 `release` 叶样本 | 775 | 475 | 360 | 223 |

`GlobalAlloc::alloc` 路径彻底消失，不是采样比例的小幅波动。`GlobalAlloc::dealloc` 的旧包装路径也消失；TLSF 自身仍有锁，因此外层 `alloc::dealloc/realloc` 仍能出现在 release 父路径中。

### 剩余 release 直接父路径

原子统计版本：

| 直接父路径 | 预构建 | 正式 |
| --- | ---: | ---: |
| 地址空间 `alloc_frame` | 131 | 51 |
| `UnmapGather::finish` | 71 | 25 |
| task 创建/退出两个闭包 | 118 | 50 |
| `alloc::dealloc` | 35 | 23 |
| `alloc::realloc` | 33 | 16 |
| Arc drop | 23 | 13 |
| folio 高阶分配 | 2 | 2 |

next-fit 已使高阶 folio 分配保持在噪声级别。当前 allocator release 不再由一个单独的全局统计锁主导。

### 当前正式阶段主要叶节点

| 叶节点 | 样本 | 说明 |
| --- | ---: | --- |
| `CurrentTask::from_raw_ptr` | 287 | 既有 direct-current 迭代未获得端到端收益 |
| ext4 `crc32` | 255 | 已有 unroll8 优化，不宜继续盲目展开 |
| ext4 `memset` | 254 | 宽存储版本在全新镜像测试中造成破坏，不能重试 |
| `UserContext::run` | 230 | 用户/内核切换采样落点，不等于函数自身耗时 |
| `NoPreemptIrqSave::release` | 223 | 已从基线 360 下降 |
| BTree `find_key_index` | 119 | 主要来自 Cow FilePageShard 与文件页缓存 |

本轮已经移除了最大且可安全隔离的 allocator 子热点。剩余前几个候选均已有失败或收益不足的历史证据，不能只按火焰宽度重复修改。

## 锁等待与 I/O

### 文件系统

| 事件 | 基线预构建 | 原子预构建 | 变化 | 基线正式 | 原子正式 | 变化 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 同步文件系统 I/O | 136.341 s | 128.876 s | -5.48% | 76.110 s | 76.967 s | +1.13% |
| ext4 锁等待 | 4.246 s | 3.599 s | -15.24% | 1.208 s | 1.213 s | +0.41% |
| folio 加载等待 | 3.142 s | 2.889 s | -8.05% | 0.375 s | 0.379 s | +1.07% |

正式阶段三项基本持平，说明本轮 2.64 秒改善不是由 I/O 算法变化造成。预构建值下降与 phase 缩短方向一致，但不能证明 usage 原子直接优化了 ext4。

### 地址空间写锁累计持有

| 事件 | 基线预构建 | 原子预构建 | 基线正式 | 原子正式 |
| --- | ---: | ---: | ---: | ---: |
| clone child | 48.801 s | 48.658 s | 20.220 s | 21.866 s |
| clone parent | 24.342 s | 24.922 s | 8.948 s | 11.476 s |
| exit clear | 52.694 s | 50.583 s | 22.342 s | 21.762 s |
| exec clear | 31.272 s | 31.356 s | 10.448 s | 10.404 s |
| mprotect | 49.673 s | 48.490 s | 21.579 s | 21.215 s |
| munmap | 25.889 s | 26.829 s | 8.264 s | 9.965 s |

这些变化有升有降。usage 统计不修改地址空间锁，表格用于排除“地址空间工作量突然整体下降”这一错误归因。

## 火焰图

- [完整 CPU](guest-cpu-all.svg)
- [预构建 CPU](guest-cpu-prebuild.svg)
- [正式 CPU](guest-cpu-timed.svg)
- [完整 off-CPU](guest-offcpu-all.svg)
- [正式 off-CPU](guest-offcpu-timed.svg)
- [正式锁等待](guest-offcpu-lock-timed.svg)
- [正式同步文件系统 I/O](guest-fs-io-latency-timed.svg)
- [正式 ext4 锁等待](guest-ext4-lock-wait-timed.svg)
- [正式 folio 加载等待](guest-folio-load-wait-timed.svg)
- [正式 clone child 写锁](guest-aspace-clone-child-write-hold-timed.svg)
- [正式 clone parent 写锁](guest-aspace-clone-parent-write-hold-timed.svg)
- [正式 mprotect 写锁](guest-aspace-mprotect-write-hold-timed.svg)
- [正式 munmap 写锁](guest-aspace-munmap-write-hold-timed.svg)

## 下一步判断

不能选择以下已被证伪或已优化过的路径：

- 全局宽存储 `memset`：两次全新镜像出现文件系统破坏；
- 单独去除 `CurrentTask::from_raw_ptr` 包装：已有完整迭代，无端到端收益；
- 继续展开 CRC：unroll8 已降低目标样本，继续展开缺乏证据；
- `UserContext::run`：只是用户态采样边界。

剩余可以继续论证的两个方向：

1. 地址空间 order-0 `alloc_frame` 的 PCP 锁路径：正式 51 个 release 样本。要先区分本地 PCP pop、refill 和全局 drain，不能直接移除 IRQ 保护。
2. BTree `find_key_index`：正式 119 个样本，其中 52 个来自 `FilePageShard::insert`，25 个来自 `FileBackend::read_at`，23 个来自 BTree range search。

下一轮更合理的是先静态审查 `FilePageShard` 的实际条目生命周期和访问顺序，判断 overflow 是否适合有序 `Vec`。如果不能证明每 shard 条目小且插入大多单调，就不应仅凭 52 个样本替换 BTree。
