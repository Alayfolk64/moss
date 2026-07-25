# RISC-V 页缓存水位总页数优化报告

## 结论

本轮将 `CachedFile::memory_watermarks()` 中的总内存页数计算从：

```rust
let allocator = global_allocator();
let total = allocator.used_pages() + allocator.available_pages();
```

改为：

```rust
let total = global_allocator().total_pages();
```

修改没有改变页缓存回收阈值的含义。当前使用的 level-2 页分配器中：

```text
used_pages     = 全局 bitmap 已用页 - PCP 空闲缓存页
available_pages = 全局 bitmap 空闲页 + PCP 空闲缓存页
```

两项相加后 PCP 数量相互抵消，结果恒等于分配器初始化时记录的托管页总数。
因此旧实现为了取得一个不随分配状态变化的常量，每次都重复扫描两遍 64 个
per-CPU 计数并两次取得全局 bitmap 分配器锁。

新实现直接读取初始化后只读的 `managed_pages`，不新增锁、原子写或跨 CPU
共享热缓存行。页缓存是否真正需要回收，仍由后续 `available_pages()` 根据
实时空闲页数量决定，因此 8 GiB 内存下的动态回收行为没有被改成固定
256 MiB 上限。

三遍静态审查、profiling 内核构建、全新官方 RISC-V 镜像、cagent 10/10、
完整 tg-xtask 预构建和正式 BuildStorm 均通过。生成 112 张同口径 SVG，
采样没有丢失：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

目标热点基本消失：

- 预构建阶段 `memory_watermarks` 下的 `saturating_sub` 叶样本从 223 个
  降为 0；
- 正式阶段从 18 个降为 0；
- 预构建阶段包含 `memory_watermarks` 的 CPU 栈从 248 个降为 1；
- 正式阶段从 27 个降为 0。

端到端结果相对上一轮 PTE 内联有所恢复：正式 BuildStorm 从 515.86 秒
降为 510.37 秒，减少 5.49 秒；正式 profiling phase 减少 5.01 秒。
但是相对更早的 guard-inline 接受基线 508.98 秒仍慢 1.39 秒。三轮宿主
全流程都约为 1756.3 秒，说明测试脚本的收尾窗口和宿主调度会掩盖小改动。
所以本轮只能证明重复查询路径被消除，不能仅凭一次运行声称获得稳定的
5.49 秒端到端收益。

## 修改范围

源码只修改一行：

```text
vendor/axfs/src/highlevel/file.rs
```

同步更新 vendor 校验和：

```text
vendor/axfs/.cargo-checksum.json
src/highlevel/file.rs =
d3b322870762ddcd6ced701666352b3d5996b42c1d73770f5bd7f02f432509d4
```

没有修改：

- PCP 分配、批量补充和回收；
- `available_pages()` 的实时空闲页计算；
- 页缓存 low/target 比例；
- frame 所有权和释放顺序；
- ext4、VirtIO、PTE、TLB 或地址空间锁；
- LoongArch 和 RISC-V 的架构相关实现。

## 三遍静态审查

### 第一遍：数学等价与差异审查

核对 level-2 `GlobalAllocator`：

1. `total_pages()` 读取 `managed_pages`；
2. `used_pages()` 从 bitmap 已用页中减去 PCP 缓存的空闲页；
3. `available_pages()` 给 bitmap 空闲页加上同一 PCP 缓存页；
4. bitmap 的 `used_pages + available_pages` 等于其托管页总数；
5. PCP refill/drain 只搬运页面，不增加或删除托管内存。

因此当前构建配置下：

```text
used_pages() + available_pages() == total_pages()
```

`git diff --check` 和 vendor 源码 SHA-256 校验均通过。

### 第二遍：并发与回收语义审查

- `managed_pages` 在页分配器初始化/扩容时更新，正常运行期间无页面分配
  快路径写入；
- 新路径没有全局原子计数器，避免让 8 个 CPU 的 PCP 命中写同一缓存行；
- `memory_watermarks()` 只计算容量阈值，不据此判断当前是否缺内存；
- `reclaim_page_cache_if_needed()` 后续仍读取实时 `available_pages()`；
- 不改变回收锁、文件页引用计数和脏页写回顺序；
- 不把并行缺页重新连接到进程级或系统级大锁。

### 第三遍：构建输入与格式审查

- `cargo fmt --all --check` 通过；
- `cargo metadata --offline --no-deps` 通过；
- `git diff --check` 通过；
- vendor checksum 校验通过；
- 检查当时的最终差异，除一行源码和对应 checksum 外没有本轮额外修改；
- 三遍审查完成后才清理 `axfs` 的 RISC-V release 产物并构建。

## 构建与机器码核对

构建命令：

```text
cargo clean -p axfs --target riscv64gc-unknown-none-elf --release
make PROFILE=y kernel-rv
```

构建成功，用时 12.84 秒。产物：

```text
kernel-rv
sha256=b4ebb3f22c14ae32f02c7b1977c2e1480667a53649c3253f6f42dbf994b13172

kernel-rv.profile.elf
sha256=d4bfd5d771e98d91a799219ee7f5868a7381415889a9958a5bc46378708ac806
```

反汇编确认 `CachedFile::reclaim_page_cache_if_needed` 的水位计算调用
`total_pages()`，不再调用 `used_pages()`；后续实际内存余量检查仍调用
`available_pages()`。`/proc` 等状态展示路径中的 `used_pages()` 未改动，
因为它们需要报告实时用量。

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
tg-xtask prebuild: 19m 20s
Cargo release build: 8m 15s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=510.37 cores=8
artifact bytes=1681000
profile host elapsed=1756.302 s
SVG=112
```

`BUILDSTORM_MINIBUILD fail` 是测试脚本中的独立 minibuild 项；完整 tg-xtask
预构建和正式 BuildStorm 均成功。

## 时间对比

| 指标 | guard-inline 接受基线 | PTE 内联 | 水位总页数 | 相对 PTE |
| --- | ---: | ---: | ---: | ---: |
| setup phase | 4.251 s | 3.751 s | 3.962 s | +0.211 s |
| 预构建 phase | 1150.129 s | 1163.514 s | 1161.134 s | -2.381 s |
| Cargo 预构建自报 | 19m09s | 19m22s | 19m20s | -2 s |
| timed phase | 584.821 s | 585.024 s | 580.015 s | -5.009 s |
| BuildStorm 自报 | 508.98 s | 515.86 s | 510.37 s | -5.49 s |
| Cargo 正式构建自报 | 8m13s | 8m20s | 8m15s | -5 s |
| 宿主全流程 | 1756.230 s | 1756.289 s | 1756.302 s | +0.013 s |

正式 BuildStorm 相对 guard-inline 仍为 `+1.39 s / +0.27%`。这个差值比
相邻两轮漂移小，不能证明回归，也不能证明改动给端点稳定提速。目标路径
的样本归零才是本轮可直接归因的结果。

## CPU 火焰图

以下统计排除 `task:idle`。

| 阶段 | PTE 非 idle 样本 | PTE 水位栈 | PTE 目标叶样本 | 本轮非 idle 样本 | 本轮水位栈 | 本轮目标叶样本 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 预构建 | 9,222 | 248 | 223 | 9,149 | 1 | **0** |
| 正式 | 3,952 | 27 | 18 | 3,996 | 0 | **0** |

本轮主要非 idle 叶路径：

| 叶路径 | 预构建 | 正式 | 判断 |
| --- | ---: | ---: | --- |
| `NoPreemptIrqSave::release` 内联区间 | 1062 / 11.61% | 366 / 9.16% | 受定时器落点偏差影响 |
| `CurrentTask::from_raw_ptr` | 728 / 7.96% | 317 / 7.93% | 高频任务上下文入口 |
| lwext4 `memset` | 611 / 6.68% | 277 / 6.93% | 宽存储版本已有失败证据，不重试 |
| `UserContext::run` | 471 / 5.15% | 208 / 5.21% | trap 返回附近采样落点 |
| BTree `find_key_index` | 271 / 2.96% | 109 / 2.73% | 分散于缺页、VMA 查询和回收 |
| `handle_page_fault_concurrent_inner` | 281 / 3.07% | 107 / 2.68% | 缺页主路径 |
| `alloc_page_cached` | 244 / 2.67% | 83 / 2.08% | 主要来自 COW 缺页分配 |
| `BitmapPageAllocator::available_pages` | 120 / 1.31% | 较少 | 仍存在于实时余量检查 |

`find_key_index` 并非一把单独的大锁：预构建 271 个样本中约 91 个来自
COW 缺页提交时插入文件页引用，72 个来自 VMA 范围查询，其余分布在文件
读取、回收删除和 `page_ref_at`。在没有按用途拆分数据结构前，不应仅凭
总宽度整体替换 BTree。

`alloc_page_cached` 更集中：预构建 244 个样本中 224 个、正式 83 个样本
中 77 个位于 `CowFaultPlan::execute -> alloc_frame`。下一轮应先审查 PCP
本地命中路径，确认耗时来自限额更新、per-CPU 锁还是计数维护，再选择
修改；不能直接增加一个所有 CPU 每次分配都写的全局原子计数器。

## 同步与 I/O 对比

以下是任务累计时间，不是墙钟时间：

| 事件 | PTE 预构建 | 水位预构建 | PTE 正式 | 水位正式 |
| --- | ---: | ---: | ---: | ---: |
| clone child 写锁持有 | 50.916 s | 54.891 s | 24.673 s | 22.691 s |
| clone parent 写锁持有 | 26.903 s | 26.766 s | 13.083 s | 12.533 s |
| exit clear 写锁持有 | 51.612 s | 52.443 s | 21.913 s | 22.840 s |
| exec clear 写锁持有 | 30.416 s | 32.485 s | 10.354 s | 10.527 s |
| mprotect 写锁持有 | 50.826 s | 51.401 s | 21.854 s | 21.532 s |
| munmap 写锁持有 | 26.858 s | 26.831 s | 9.743 s | 9.939 s |
| ext4 锁等待 | 3.598 s | 4.481 s | 0.741 s | 1.172 s |
| folio 加载等待 | 3.229 s | 3.129 s | 0.332 s | 0.298 s |
| 同步块读取 | 4.931 s | 4.541 s | 1.691 s | 1.845 s |
| 同步块写入 | 0.268 s | 0.392 s | 0.534 s | 0.409 s |

这些非目标事件有升有降，符合两轮工作负载和宿主调度的自然波动。mprotect
调用次数也有约 0.5% 以内差异，因此不能把某一项累计时间变化全部归因于
本轮一行修改。

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
