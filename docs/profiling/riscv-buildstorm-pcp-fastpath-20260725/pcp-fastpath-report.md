# RISC-V PCP 分配 fast/slow path 迭代报告

## 结论

本轮针对上一轮火焰图中的 `GlobalAllocator::alloc_page_cached` 做了
fast/slow path 分离，没有调整 PCP 容量、批量大小、回收阈值、锁和页面
记账。

修改前的原始 PC 分布表明，预构建 244 个目标样本中有 221 个落在
`SpinNoIrq` 重新开启中断后的第一条指令，正式阶段 83 个中有 76 个落在
同一位置。这不是一条 `mv` 指令本身耗时，而是定时器在 IRQ 关闭期间无法
采样，待临界区结束后集中触发。因此该宽度代表 PCP 临界区的累计成本，但
不能继续按源码叶行机械归因。

反汇编发现旧 fast path 每次命中仍会：

1. 调用一次 `PerCpuPageCache::update_limits()`，而该函数通常只比较两个
   不变字段后返回；
2. 为极少发生的“refill 失败、全局 drain、第二次重试、最终 fallback”
   冷路径保留大栈帧和寄存器；
3. 将 refill 逻辑与命中链表的短路径放在同一个函数体内。

本轮将配置相等检查内联到命中路径，把 refill 和全局恢复路径拆成
`#[cold] #[inline(never)]` 函数。三遍静态审查、profiling 内核构建、
全新官方镜像、cagent 10/10、完整 tg-xtask 预构建和正式 BuildStorm
均通过。生成 112 张同口径 SVG，采样零丢失：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

目标局部指标显著改善：

| 阶段 | 修改前目标叶样本 | 本轮目标叶样本 | 降幅 |
| --- | ---: | ---: | ---: |
| 预构建 | 244 / 2.67% | 15 / 0.16% | **-93.9%** |
| 正式 | 83 / 2.08% | 4 / 0.10% | **-95.2%** |

但是端到端没有提速：正式 BuildStorm 从 510.37 秒变为 518.11 秒，慢
7.74 秒；预构建从 19m20s 变为 19m39s。目标路径的机器码和样本下降是真实
且可归因的，墙钟回归也是真实测量结果，两者并不矛盾：旧目标只占约 2%
非 idle CPU 样本，减少它不足以支配 `aws-lc-sys`、axbuild、文件写入和
宿主 QEMU 调度造成的长尾。

准确结论是：

1. fast/slow 分离确实缩短了 PCP 命中和 IRQ 关闭路径；
2. 修改没有使端到端变快，不能声称获得 7 秒或其他墙钟收益；
3. 本轮预构建同步文件 I/O 累计时间比上一轮多 14.40 秒，其中
   `CachedFile::write_at` 多 15.73 秒，与 19.24 秒的预构建 phase 增量
   同量级；
4. 正式阶段新的最大可操作热点是分散的 BTree 查询，其中 COW 缺页提交
   的 `FilePageRefs` 插入占最大子项，下一轮先审查该数据结构，不能整体
   替换所有 BTree。

## 修改内容

### GlobalAllocator

普通命中路径现在只做一次 PCP 尝试。第一次 refill 仍由 PCP 自己完成；
只有 refill 失败时才进入独立冷函数：

```rust
let page = {
    let mut cache = self.page_caches[slot].lock();
    let page = cache.allocate(self, total_pages, cpu_count, kind);
    self.page_cache_counts[slot]
        .0
        .store(cache.count(), Ordering::Relaxed);
    page
};
if let Some(page) = page {
    return Ok(page);
}

self.alloc_page_cached_slow(slot, total_pages, cpu_count, kind)
```

`alloc_page_cached_slow()` 保留旧代码的顺序：

```text
drain 所有 PCP
    ↓
在原 slot 上第二次 allocate
    ↓
成功则返回
    ↓
失败则调用 alloc_pages(1)
```

### PerCpuPageCache

旧 `update_limits()` 自身已有相等快速返回，但函数体包含重新计算和 drain，
优化器没有把它内联。现在调用前先做相同谓词：

```rust
if self.configured_total_pages != total_pages
    || self.configured_cpu_count != cpu_count.max(1)
{
    self.update_limits(allocator, total_pages, cpu_count);
}
```

命中时直接衰减 `free_count` 并 `pop()`；空链表才调用独立的
`refill_and_pop()`。

## 三遍静态审查

### 第一遍：控制流等价

逐项对照旧 `for attempt in 0..2`：

| 旧分支 | 新分支 |
| --- | --- |
| 第一次 `cache.allocate()` 成功后返回 | fast path 成功后返回 |
| 第一次失败后 `drain_page_caches()` | 进入 slow path 后首先 drain |
| 第二次在同一 slot 上调用 `cache.allocate()` | slow path 在同一 slot 上重试 |
| 第二次失败后 `alloc_pages(1)` | `None` 分支调用同一 fallback |

`total_pages` 和 `slot` 仍只在入口计算一次。`cpu_count=0` 时，旧
`update_limits()` 使用 `max(1)`；新谓词也和 `max(1)` 比较，行为一致。

### 第二遍：并发、锁和所有权

- 两次 PCP 操作仍各自在同一把 `page_caches[slot]` 的 `SpinNoIrq` 锁内；
- 第一次锁释放后才执行全局 drain，与旧块作用域一致；
- refill 仍在持有对应 PCP 锁时发生，没有产生并发双 refill；
- `page_cache_counts[slot]` 仍在链表修改后、释放锁前发布；
- `free_count` 每次 allocate 仍只右移一次；
- refill 后仍只弹出一个与 `UsageKind` 匹配的页面；
- 失败恢复仍使用入口时选择的 slot，迁移窗口没有扩大；
- 没有新增 `unsafe`、原子变量、共享写、阻塞等待或调度点；
- `#[inline]`、`#[cold]` 和 `#[inline(never)]` 只影响代码布局。

### 第三遍：格式、feature 和 vendor 校验

- `cargo fmt --all --check` 通过；
- `cargo metadata --offline --no-deps` 通过；
- `git diff --check` 通过；
- 最终源码差异只有 `vendor/axalloc/src/lib.rs` 和
  `vendor/axalloc/src/pcp.rs`；
- `src/lib.rs` SHA-256：
  `f785a95315ded4ea643147811d936cc8428856209c7881905ee5dfcd99680b8b`；
- `src/pcp.rs` SHA-256：
  `0dc13cdad32f2d026ad16aaa7d74da2d596d3040404cc9e1842bde9e47c2b333`；
- 两个值与 `vendor/axalloc/.cargo-checksum.json` 完全一致；
- 三遍审查完成后才清理 `axalloc` 的 RISC-V release 产物并构建。

## 构建与反汇编

构建命令：

```text
cargo clean -p axalloc --target riscv64gc-unknown-none-elf --release
make PROFILE=y kernel-rv
```

构建成功，用时 12.92 秒。产物：

```text
kernel-rv
sha256=c7f6fa46667da36f3ac61500b6b8f76d31b18de366238f666b8f355df9f1e6d9

kernel-rv.profile.elf
sha256=c863036e21d67d7bfdef3a71065fc05f6c85b2ff3f5129c50005e277feec8f8f
```

机器码变化：

- `alloc_page_cached` 从 `0x220` 字节降为 `0x158` 字节，即从 544 字节
  降为 344 字节，减少 36.8%；
- 普通路径的栈帧从 `0x90` 降为 `0x60`；
- ELF 出现独立 `alloc_page_cached_slow`；
- ELF 出现独立 `PerCpuPageCache::refill_and_pop`；
- `PerCpuPageCache::allocate` 不再有独立符号，命中逻辑已进入调用者；
- fast path 仅在配置字段不一致时调用 `update_limits`。

这证明源码意图在最终 RISC-V 机器码中生效，不是只看 Rust 源码作出的
推测。

## 完整测试口径

- Ubuntu 宿主直接运行 QEMU，不使用 Docker；
- RISC-V `virt`，8 GiB，8 vCPU；
- QEMU 使用 `-snapshot`；
- 根文件系统重新从官方压缩包解压，不复用上一轮 target；
- 镜像大小 15,032,385,536 字节；
- 镜像 SHA-256：
  `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0`；
- CPU 10 Hz；
- off-CPU 1/8；
- ext4 1/16；
- page cache 1/32；
- AddrSpace 写锁持有 1/16；
- mprotect 1/32。

运行结果：

```text
cagent: 10/10 pass
tg-xtask prebuild: 19m 39s
Cargo release build: 8m 22s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=518.11 cores=8
artifact bytes=1681000
profile host elapsed=1786.279 s
SVG=112
```

`BUILDSTORM_MINIBUILD fail` 仍是独立 minibuild 项；完整 tg-xtask 预构建和
正式 BuildStorm 成功。

## 时间对比

| 指标 | guard-inline 接受基线 | 水位总页数 | PCP fast path | 相对上一轮 |
| --- | ---: | ---: | ---: | ---: |
| setup phase | 4.251 s | 3.962 s | 4.018 s | +0.057 s |
| 预构建 phase | 1150.129 s | 1161.134 s | 1180.369 s | +19.236 s |
| Cargo 预构建自报 | 19m09s | 19m20s | 19m39s | +19 s |
| timed phase | 584.821 s | 580.015 s | 596.314 s | +16.299 s |
| BuildStorm 自报 | 508.98 s | 510.37 s | 518.11 s | +7.74 s |
| Cargo 正式构建自报 | 8m13s | 8m15s | 8m22s | +7 s |
| 宿主全流程 | 1756.230 s | 1756.302 s | 1786.279 s | +29.977 s |

本轮不能通过端到端验收。由于修改只缩短命中机器码，且目标样本下降
94%–95%，没有证据表明它造成额外 I/O 或锁；暂时保留这一低风险改动，
但不把墙钟回归解释成优化收益。

## CPU 火焰图

以下统计排除 `task:idle`。

| 阶段 | 水位轮非 idle | 水位轮目标叶 | 本轮非 idle | 本轮目标叶 |
| --- | ---: | ---: | ---: | ---: |
| 预构建 | 9,149 | 244 / 2.67% | 9,301 | **15 / 0.16%** |
| 正式 | 3,996 | 83 / 2.08% | 3,966 | **4 / 0.10%** |

本轮主要非 idle 叶路径：

| 叶路径 | 预构建 | 正式 | 判断 |
| --- | ---: | ---: | --- |
| `NoPreemptIrqSave::release` 内联区间 | 1317 / 14.16% | 414 / 10.44% | 仍含其他 IRQ 关闭区间的延迟采样 |
| `CurrentTask::from_raw_ptr` | 741 / 7.97% | 277 / 6.98% | 高频上下文入口 |
| lwext4 `memset` | 619 / 6.66% | 233 / 5.87% | 全局宽存储版本已有损坏证据 |
| `UserContext::run` | 565 / 6.07% | 170 / 4.29% | trap 返回附近落点 |
| BTree `find_key_index` | 250 / 2.69% | **160 / 4.03%** | 当前最大可拆分数据结构热点 |
| `handle_page_fault_concurrent_inner` | 228 / 2.45% | 49 / 1.24% | 缺页主路径 |
| `disable_preempt` | 221 / 2.38% | 61 / 1.54% | guard 高频成本 |
| SBI call | 168 / 1.81% | 77 / 1.94% | TLB/IPI 等固件调用 |
| bitmap `available_pages` | 165 / 1.77% | 较少 | 页缓存实时余量检查 |

`alloc_page_cached_slow`、`refill_and_pop` 和 `update_limits` 没有成为新的
CPU 叶热点。

## 锁与 I/O 侧证

以下为任务累计时间，不是墙钟时间：

| 事件 | 水位轮预构建 | 本轮预构建 | 水位轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| clone child 写锁持有 | 54.891 s | 53.336 s | 22.691 s | 23.897 s |
| clone parent 写锁持有 | 26.766 s | 28.698 s | 12.533 s | 11.090 s |
| exit clear 写锁持有 | 52.443 s | 52.932 s | 22.840 s | 22.131 s |
| exec clear 写锁持有 | 32.485 s | 32.847 s | 10.527 s | 10.413 s |
| mprotect 写锁持有 | 51.401 s | 51.969 s | 21.532 s | 22.697 s |
| munmap 写锁持有 | 26.831 s | 27.922 s | 9.939 s | 11.030 s |
| ext4 锁等待 | 4.481 s | 4.461 s | 1.172 s | 1.092 s |
| folio 加载等待 | 3.129 s | 3.419 s | 0.298 s | 0.328 s |
| 同步文件系统 I/O | 153.248 s | **167.652 s** | 81.916 s | 80.268 s |

预构建 I/O 增量主要来自：

```text
CachedFile::write_at
49.308 s -> 65.042 s
```

其余主要 AddrSpace 写锁事件没有一致同向变化。正式 mprotect 调用数从
661,485 变为 664,851，增加约 0.51%；预构建调用数减少约 0.33%。这说明
两轮工作量非常接近但并非逐指令相同。

## 下一轮热点选择

正式阶段 BTree `find_key_index` 从 109 个叶样本变为 160 个。它不是一套
可以整体替换的单一数据结构：

- 约 78 个样本位于 COW 缺页提交和 `FilePageRefs` 插入；
- 约 20 个样本位于回收/解除映射时的 `FilePageRefs::remove`；
- 其余分散于 VMA `range_search`、文件页查找、mprotect、clone 和 ext4
  extent 查询。

因此下一轮先读 `FilePageRefs` 的生命周期和访问模式，重点判断是否能为
“按虚拟页号插入、查询、删除”提供更合适的分片或 radix/xarray 风格结构。
不能直接把所有 BTree 换成哈希表：

1. VMA 查询需要有序范围语义；
2. clear/unmap 需要有序遍历和批量移除；
3. 哈希表会增加随机访存和分配；
4. 改动必须保留 `FolioPageRef` 的 drop 时机与 TLB 失效顺序。

完成该静态分析前不修改、更不构建。

## 火焰图

- [完整 CPU](guest-cpu-all.svg)
- [预构建 CPU](guest-cpu-prebuild.svg)
- [正式 CPU](guest-cpu-timed.svg)
- [完整 off-CPU](guest-offcpu-all.svg)
- [锁等待 off-CPU](guest-offcpu-lock-all.svg)
- [工作负载 off-CPU](guest-offcpu-workload-all.svg)
- [ext4 全局锁等待](guest-ext4-lock-wait-all.svg)
- [folio 加载等待](guest-folio-load-wait-all.svg)
- [同步文件系统 I/O](guest-fs-io-latency-all.svg)
- [mprotect 地址空间锁等待](guest-mprotect-aspace-lock-wait-all.svg)
- [exit clear 写锁](guest-aspace-exit-clear-write-hold-all.svg)
- [exec clear 写锁](guest-aspace-exec-clear-write-hold-all.svg)
- [clone child 写锁](guest-aspace-clone-child-write-hold-all.svg)
- [clone parent 写锁](guest-aspace-clone-parent-write-hold-all.svg)
