# RISC-V 连续页 next-fit 迭代报告

## 结论

本轮修改有效消除了上轮火焰图中最明确的局部热点：高阶 folio 分配每次都从位图 0 开始扫描。

在相同的官方 RISC-V 镜像、8 GiB、8 vCPU、`-snapshot` 和完整 guest profiler 口径下：

| 指标 | Cow 内联槽基线 | 连续页 next-fit | 变化 |
| --- | ---: | ---: | ---: |
| Cargo 预构建 | 19m25s | 19m02s | -23s |
| prebuild phase | 1166.185 s | 1143.762 s | -22.423 s（-1.92%） |
| Cargo release | 8m20s | 8m16s | -4s |
| BuildStorm | 515.89 s | 511.76 s | -4.13 s（-0.80%） |
| timed phase | 583.185 s | 579.563 s | -3.622 s（-0.62%） |
| 宿主全流程 | 1771.332 s | 1741.349 s | -29.983 s（-1.69%） |

更重要的是，目标路径的 CPU 样本发生了结构性变化：

| CPU 叶样本 | 基线预构建 | next-fit 预构建 | 基线正式 | next-fit 正式 |
| --- | ---: | ---: | ---: | ---: |
| `NoPreemptIrqSave::release` | 1367 | 775 | 491 | 360 |
| `alloc_folio_frame -> release` | **446** | **0** | **82** | **0** |
| `alloc_page_frame -> release` | 3 | 2 | 1 | 2 |

`alloc_folio_frame` 分配的是高阶连续页；它的 delayed leaf 样本从 `446/82` 降到 `0/0`。这说明 next-fit 确实不再让每次高阶分配在关中断的全局页分配器锁内重复扫描低地址占用前缀。

但端到端 BuildStorm 只改善 0.80%，而且仍比此前单次最佳的水位总页数结果 `510.37 s` 慢 `1.39 s`。因此本轮可以确认的是“局部热点被消除”，不能把 4.13 秒全部认定为稳定收益。完整流程仍受 Cargo 依赖图尾部、全局堆分配器、用户态 `memset`、ext4 CRC、同步 I/O 和地址空间管理共同影响。

## 修改动机

上轮 Cow 内联槽报告中，`NoPreemptIrqSave::release` 是最大的内核 delayed leaf：

- 预构建：1367 / 9231 个非 idle 样本；
- 正式：491 / 3929 个非 idle 样本。

其中最清晰的直接父路径是：

- `alloc_folio_frame`：预构建 446，正式 82；
- `GlobalAlloc::alloc`：预构建 218，正式 139；
- 地址空间 `alloc_frame`：预构建 150，正式 48；
- 页表 `alloc_frame`：预构建 139，正式 14。

`alloc_folio_frame` 申请一组物理连续页。页缓存 folio 常用 16 个 4 KiB 基页，即 64 KiB。旧的连续页搜索每次都从位图索引 0 开始：

```text
第一次：从 0 扫描，找到空闲连续区 A
第二次：又从 0 扫过已经占用的前缀，找到 B
第三次：再从 0 扫过 A、B，找到 C
……
```

位图由全局 `palloc: SpinNoIrq<BitmapPageAllocator>` 保护，搜索期间中断和抢占都关闭。低地址前缀越长，持锁时间越长；其他 CPU 的页分配和 PCP refill 也会被延迟。

## 实现

### 1. 位图分配器支持指定搜索起点

在 `BitAlloc` 中增加：

```rust
fn alloc_contiguous_from(
    &mut self,
    start: usize,
    size: usize,
    align_log2: usize,
) -> Option<usize>
```

底层 `find_contiguous` 新增 `start` 参数。原有 `alloc_contiguous(None, ...)` 仍传入 0，因此旧调用者的语义没有改变。

搜索仍严格满足：

- 返回区间全部空闲；
- 起点满足 `2^align_log2` 对齐；
- 区间不越过位图容量；
- 搜索失败不修改位图。

### 2. 页分配器保存连续页游标

`BitmapPageAllocator` 增加：

```rust
next_contiguous_search: usize
```

成功分配 `num_pages > 1` 后，游标移动到本次区间末尾：

```text
next = allocated_index + num_pages
```

下一次连续页分配从该位置继续搜索。

### 3. 尾部失败后只进行一次回绕

执行顺序为：

```text
[cursor, bitmap_end) 搜索
        |
        +-- 成功：更新 cursor
        |
        +-- 失败：从 0 回绕搜索
```

因此游标不会永久跳过低地址已经释放的空洞，也不会制造“明明有连续内存却报告 OOM”的错误。

当前回绕调用原有从 0 开始的完整搜索。它在失败场景可能再次查看游标之后的一小段区域，但正常连续分配不再反复扫描整个低地址前缀。本轮先采用这个较小、容易验证的修改，没有进一步扩展位图 API 的结束边界。

### 4. 完全空闲时恢复最低地址行为

当 `used_pages` 降到 0 时，游标重置到第一个可用页。这保持了既有测试和初始化后的最低地址分配语义。

单页分配、指定地址分配和释放格式均未改变。

## 三遍静态审查

### 第一遍：语义与边界

检查了：

- `start >= capacity` 立即失败；
- 起点先向上对齐，再检查连续区间；
- 尾部失败后回绕；
- OOM 不修改位图和游标；
- 成功后才增加 `used_pages`；
- 单页路径不经过 next-fit；
- `alloc_pages_at` 语义不变；
- 完全释放后游标恢复到第一个可用页；
- 所有 `find_contiguous` 调用点均补充了明确的起点。

本遍发现并修复了两个内部单元测试仍使用旧函数签名的问题。

### 第二遍：并发、锁序和性能

检查结果：

- 游标和位图仍由同一个 `palloc: SpinNoIrq` 保护；
- 没有增加新锁或原子变量；
- 没有改变任何锁顺序；
- 搜索仍在原锁内完成，但稳态搜索范围缩短；
- PCP 只处理 order-0 页，不依赖该游标；
- 低地址释放区可能等到回绕才被复用，但不会丢失；
- 失败后保留旧游标，PCP drain 后的重试仍会检查尾部和低地址区域。

### 第三遍：机械完整性

以下检查全部通过：

- `cargo fmt --all -- --check`
- `cargo metadata --offline --no-deps`
- `git diff --check`
- 全仓所有 `find_contiguous` 调用点检查
- `README.md` 未修改
- `axalloc`、`axallocator`、`bitmap-allocator` vendor 文件 SHA256 与 `.cargo-checksum.json` 一致

两个 vendor 包不是 workspace member，Cargo 拒绝直接从根工作区运行它们的独立测试；没有为测试去修改 workspace。实际 PROFILE 内核构建随后完整编译了三个受影响包并通过。

## 构建校验

第一次只清理 `starry-kernel` 时，Cargo 复用了 `bitmap-allocator`、`axallocator` 和 `axalloc` 的旧依赖产物，生成的内核哈希与上一轮完全相同。该二进制被判定为无效，没有进入 QEMU 测试。

随后显式清理：

```text
bitmap-allocator
axallocator
axalloc
starry-kernel
```

重建日志明确包含：

```text
Compiling bitmap-allocator
Compiling axallocator
Compiling axalloc
Compiling starry-kernel
```

最终构建：

| 项目 | 结果 |
| --- | --- |
| 命令 | `make PROFILE=y kernel-rv` |
| 构建耗时 | 13.09 s |
| `kernel-rv` SHA256 | `dae23915357d13e3912c8b886e353a105d3503fde7d15faf035e9dc05b3f577b` |
| `kernel-rv.profile.elf` SHA256 | `a3bd1cbfe9187dfb87aec48277f82d845d88831d54cf6087b745500d379e1ce4` |

哈希已经与上一轮不同，确认 next-fit 进入最终内核。

## 完整测试口径

### 镜像

每轮都从以下压缩镜像重新解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
```

解压后：

| 项目 | 值 |
| --- | --- |
| 大小 | 15,032,385,536 字节 |
| SHA256 | `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0` |

QEMU 使用：

```text
-m 8G -smp 8 -snapshot
```

运行前确认宿主机没有其他 RISC-V 或 LoongArch QEMU 进程。

### profiler

采样口径保持不变：

| 类型 | 口径 |
| --- | --- |
| CPU | 10 Hz |
| off-CPU | 1/8 抽样，权重乘 8 |
| ext4 | 1/16 抽样，权重乘 16 |
| page cache | 1/32 抽样，权重乘 32 |
| mprotect | 1/32 抽样，权重乘 32 |
| 地址空间写锁持有 | 1/16 抽样，权重乘 16 |

完整运行结果：

| 项目 | 值 |
| --- | ---: |
| setup phase | 3.908 s |
| prebuild phase | 1143.762 s |
| timed phase | 579.563 s |
| 宿主全流程 | 1741.349 s |
| CPU 样本 | 54,468 |
| `dropped_cpu` | 0 |
| `dropped_wait` | 0 |
| `dropped_pending` | 0 |
| `skipped_cpu` | 1 |
| GDB 导出 | 第 1 次成功 |
| SVG | 112 张 |

一次 `skipped_cpu` 是采样中断未取得本 CPU profiler 的 `try_lock`，约占全部 CPU 样本的 0.002%，不会改变结论。

cagent 10/10 通过，BuildStorm 正式编译成功。`BUILDSTORM_MINIBUILD fail` 仍存在；当前脚本只输出 fail 而没有错误正文，但后续预构建与正式编译均成功，因此它不影响本轮性能数据，也不能归因于 next-fit。

## 时间对比

| 指标 | 水位总页数 | PCP fast path | Cow HashMap | Cow 内联槽 | bitmap next-fit |
| --- | ---: | ---: | ---: | ---: | ---: |
| setup phase | 3.962 s | 4.018 s | 3.932 s | 3.746 s | 3.908 s |
| prebuild phase | 1161.134 s | 1180.369 s | 1173.802 s | 1166.185 s | **1143.762 s** |
| Cargo 预构建 | 19m20s | 19m39s | 19m32s | 19m25s | **19m02s** |
| timed phase | 580.015 s | 596.314 s | 597.552 s | 583.185 s | **579.563 s** |
| Cargo release | 8m15s | 8m22s | 8m19s | 8m20s | 8m16s |
| BuildStorm | **510.37 s** | 518.11 s | 514.67 s | 515.89 s | 511.76 s |
| 宿主全流程 | 1756.302 s | 1786.279 s | 1786.292 s | 1771.332 s | **1741.349 s** |

本轮宿主全流程和两个 profiler phase 都是表中最低值，但 BuildStorm 单项仍不是最低值。一次运行的 1% 左右差异可能包含 QEMU 调度与 guest 工作量波动；局部火焰图变化比 4.13 秒端点变化更可信。

## CPU 火焰图

### 非 idle 总样本

| 阶段 | Cow 内联槽 | bitmap next-fit | 变化 |
| --- | ---: | ---: | ---: |
| 预构建 | 9231 | 8382 | -849（-9.20%） |
| 正式 | 3929 | 3840 | -89（-2.27%） |

phase 时间也同时下降，因此非 idle 样本下降与 CPU 工作量减少方向一致。

### IRQ-off 页分配路径

仅统计以 `NoPreemptIrqSave::release` 为叶节点的 delayed CPU 样本：

| 直接父路径 | 基线预构建 | next-fit 预构建 | 基线正式 | next-fit 正式 |
| --- | ---: | ---: | ---: | ---: |
| `alloc_folio_frame` | **446** | **0** | **82** | **0** |
| `GlobalAlloc::alloc` | 218 | 272 | 139 | 139 |
| 地址空间 `alloc_frame` | 150 | 133 | 48 | 44 |
| 页表 `alloc_frame` | 139 | 6 | 14 | 2 |
| `UnmapGather::finish` | 78 | 73 | 28 | 20 |
| `alloc::dealloc` | 62 | 62 | 33 | 33 |
| 全部 `release` 叶样本 | 1367 | 775 | 491 | 360 |

高阶 folio 路径的 `446/82 -> 0/0` 是本轮最强证据。页表分配路径也明显下降，合理解释是 PCP refill 不再长时间等待被连续页线性扫描占据的全局 `palloc` 锁；但页表样本不是本轮直接修改对象，因此只记录相关性，不单独宣称因果。

`GlobalAlloc::alloc` 没有改善，预构建还从 218 增加到 272。它已经取代高阶 folio 搜索，成为 `release` 下最大的直接父路径。

### 现在的主要 CPU 叶节点

正式阶段非 idle 聚合叶样本前列：

| 叶节点 | 样本 |
| --- | ---: |
| `NoPreemptIrqSave::release` | 360 |
| `CurrentTask::from_raw_ptr` | 326 |
| ext4 `memset` | 272 |
| ext4 `crc32` | 225 |
| `UserContext::run` | 200 |
| Cow 文件页 BTree 搜索 | 104 |
| 地址空间缺页处理 | 78 |

连续页扫描已不再出现在主要叶节点中。

## 锁等待与 I/O

### 地址空间写锁持有时间

这些值是整个工作负载的累计持锁时间，不是 wall-clock 时间：

| 事件 | 基线预构建 | next-fit 预构建 | 基线正式 | next-fit 正式 |
| --- | ---: | ---: | ---: | ---: |
| clone child | 61.393 s | 48.801 s | 21.236 s | 20.220 s |
| clone parent | 23.908 s | 24.342 s | 11.736 s | 8.948 s |
| exit clear | 51.899 s | 52.694 s | 22.249 s | 22.342 s |
| exec clear | 32.092 s | 31.272 s | 10.245 s | 10.448 s |
| mprotect | 49.072 s | 49.673 s | 21.127 s | 21.579 s |
| munmap | 26.278 s | 25.889 s | 10.439 s | 8.264 s |

clone child、clone parent 和 munmap 有下降，但 exit、exec、mprotect 基本持平。next-fit 没有直接修改地址空间锁，因此不把这些差异作为主要收益证据。

### 文件系统与 folio

| 事件 | 基线预构建 | next-fit 预构建 | 变化 | 基线正式 | next-fit 正式 | 变化 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 同步文件系统 I/O | 204.632 s | 136.341 s | -33.4% | 83.900 s | 76.110 s | -9.3% |
| ext4 锁等待 | 5.042 s | 4.246 s | -15.8% | 1.396 s | 1.208 s | -13.4% |
| folio 加载等待 | 2.994 s | 3.142 s | +4.9% | 0.409 s | 0.375 s | -8.3% |

同步 I/O 和 ext4 锁等待同时下降，但 next-fit 不改变块 I/O 算法。可能的间接关系是分配锁缩短后，提交和完成任务更及时；也可能是单轮工作负载波动。需要后续迭代继续观察，不能仅凭本轮确认因果。

## 火焰图

主要图：

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

## 下一步

本轮不应继续修改连续页搜索：目标路径已经降到 0 个 delayed leaf 样本。

下一项有直接 CPU 证据的内核热点是 `GlobalAlloc::alloc -> NoPreemptIrqSave::release`：

- 预构建：272；
- 正式：139；
- 正式阶段没有比基线下降。

当前 `alloc_level2` 在持有全局 TLSF `balloc` 锁时，又取得全局 `usages` 锁更新统计。所有小对象分配都会经过这两个 IRQ-off 临界区。一个低风险、可独立验证的下一迭代是把 `UsageKind` 统计改为逐项 `AtomicUsize`：

1. 不改变 TLSF 分配算法；
2. 不改变内存所有权；
3. 移除每次分配/释放的第二把全局 `SpinNoIrq`；
4. 缩短 `balloc` 锁的嵌套持有时间；
5. 用下一轮相同火焰图检查 `GlobalAlloc::alloc -> release` 是否下降。

如果原子统计修改后该路径仍不下降，再考虑按分配大小增加 profiler 计数，判断是否值得实现 per-CPU 小对象缓存。不能只依据当前样本直接引入复杂的 per-CPU 堆分配器。
