# RISC-V PTE 只读访问器内联迭代报告

## 结论

本轮给 RISC-V `Rv64PTE` 的六个短小只读访问器增加普通
`#[inline]`：

- `from_bits`
- `paddr`
- `bits`
- `is_unused`
- `is_present`
- `is_huge`

没有使用 `#[inline(always)]`，没有修改页表位定义、原子顺序、锁、PTE
安装方式或页面生命周期。

三遍静态审查、profiling 内核构建、全新官方镜像、cagent 10/10、完整
tg-xtask 预构建和正式 BuildStorm 均通过。生成 112 张同口径 SVG，采样
没有丢失：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

本轮目标热点已经消失：`Rv64PTE::is_huge` 的非 idle 叶样本由 CRC 基线的
预构建 278 个、正式阶段 99 个降为两个阶段均为 0；最终 ELF 中也没有六个
访问器的独立机器码符号。这证明跨 crate 调用边界确实被消除。

但端到端没有出现可证明的提速。正式 BuildStorm 从 guard-inline 接受基线
的 508.98 秒变为 515.86 秒，慢 6.88 秒，即 1.35%；预构建从 19m09s
变为 19m22s。这个幅度与历次运行漂移同量级，不能归因为六个访问器内联，
也不能用热点消失反向声称总体变快。

因此本轮的准确结论是：

1. 内联在机器码和目标叶样本上生效；
2. 修改语义风险低，可以保留；
3. 它只消除很短的调用边界，端到端收益小于当前单轮噪声；
4. 下一轮不继续扩大 PTE 内联范围，而处理有明确重复工作的内存余量查询。

## 修改和静态审查

### 第一遍：差异与校验和

- 将六行 `#[inline]` 从当前源码中剥离后，文件与修改前逐字节一致；
- 六个函数的函数体完全未变；
- `vendor/page_table_entry/.cargo-checksum.json` 中源码哈希更新为
  `1de70534ca442f8129b6c2f3cdf1f0e51c09ba9e20b462d71211e59c901729ed`；
- checksum JSON 自身 SHA-256 为
  `e4d7c74b50caacb1181f1cd54f9fcea9fcd041245fa6824934fa9718da398f06`。

### 第二遍：语义、并发和架构

- 六个方法只构造值、读取 `u64` 或测试固定位；
- 不写 PTE，不改变 A/D/V/R/W/X/U 位；
- 不引入 `unsafe`、锁、原子变量或共享可变状态；
- 普通 `#[inline]` 只给优化器提供跨 crate MIR，优化器仍可拒绝；
- LoongArch 和 AArch64 实现未修改；
- `new_page`、`new_table`、`set_paddr`、`set_flags` 和 `clear` 未修改。

### 第三遍：仓库与构建输入

- `git diff --check` 通过；
- `cargo fmt --all --check` 通过；
- `cargo metadata --offline --no-deps` 通过；
- 当时工作树内 12 个已修改 vendor 源文件与各自
  `.cargo-checksum.json` 全部一致；
- 三遍检查完成后才清理 `page_table_entry` 的 RISC-V release 产物并构建。

## 构建和机器码验证

构建命令：

```text
cargo clean -p page_table_entry --target riscv64gc-unknown-none-elf --release
make PROFILE=y kernel-rv
```

构建成功，用时 12.08 秒。产物：

```text
kernel-rv
sha256=ef9cfa126a239a9b5ef640b20db1e622e047fa214af62f3616623424facac404

kernel-rv.profile.elf
sha256=b46937fa432ebeaeb7862a860c145de3e7f668511106d3eb229ce843a8edd0c5
```

`rust-nm -C` 和完整反汇编中均找不到六个访问器的独立函数符号。火焰图中
仍可能出现 `paddr` 等源码名字，因为 DWARF 会把调用者中的一段内联指令
标记为其原始函数和行号；这不表示运行时又发生了一次同名函数调用。

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
tg-xtask prebuild: 19m 22s
Cargo release build: 8m 20s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=515.86 cores=8
artifact bytes=1681000
profile host elapsed=1756.289 s
SVG=112
```

`BUILDSTORM_MINIBUILD fail` 仍是测试脚本中独立的 minibuild 项；正式
tg-xtask 预构建和 BuildStorm 都成功。

## 时间对比

| 指标 | guard-inline 接受基线 | CRC 八次展开 | PTE 内联 | PTE 相对接受基线 |
| --- | ---: | ---: | ---: | ---: |
| setup phase | 4.25 s | 4.31 s | 3.75 s | -0.50 s |
| 预构建 phase | 1150.13 s | 1336.95 s | 1163.51 s | +13.39 s / +1.16% |
| Cargo 预构建自报 | 19m09s | 22m15s | 19m22s | +13 s / +1.14% |
| timed phase | 584.82 s | 667.02 s | 585.02 s | +0.20 s / +0.03% |
| BuildStorm 自报 | 508.98 s | 594.28 s | 515.86 s | +6.88 s / +1.35% |
| Cargo 正式构建自报 | 8m13s | 9m37s | 8m20s | +7 s / +1.42% |
| 宿主全流程 | 1756.23 s | 2026.30 s | 1756.29 s | +0.06 s |

PTE 本轮相对 CRC 轮全面恢复，说明 CRC 轮约 15% 的全程减速确实主要是运行
环境漂移；但与更接近的 guard-inline 基线相比，本轮没有端点改善。宿主
全流程还包含 guest 停止 profiler、GDB 导出和收尾，不用于替代 BuildStorm
自身计时。

## CPU 火焰图验证

以下统计均排除 `task:idle`。

| 阶段 | CRC 基线非 idle 样本 | CRC `is_huge` | PTE 本轮非 idle 样本 | PTE `is_huge` |
| --- | ---: | ---: | ---: | ---: |
| 预构建 | 11,323 | 278 / 2.46% | 9,222 | **0 / 0%** |
| 正式 | 4,931 | 99 / 2.01% | 3,952 | **0 / 0%** |

本轮 `paddr` 仍出现预构建 27 个、正式 16 个叶样本，但 ELF 中没有独立
`paddr` 符号。这些是 DWARF 标出的内联指令区间，不能与修改前的独立
`is_huge` 函数调用混为一谈。

本轮主要非 idle 叶路径：

| 叶路径 | 预构建 | 正式 |
| --- | ---: | ---: |
| `NoPreemptIrqSave::release` 内联区间 | 1164 / 12.62% | 347 / 8.78% |
| `CurrentTask::from_raw_ptr` | 711 / 7.71% | 279 / 7.06% |
| lwext4 `memset` | 637 / 6.91% | 289 / 7.31% |
| lwext4 `crc32` | 87 / 0.94% | 237 / 6.00% |
| `UserContext::run` | 549 / 5.95% | 204 / 5.16% |
| BTree `find_key_index` | 240 / 2.60% | 118 / 2.99% |
| `handle_page_fault_concurrent_inner` | 298 / 3.23% | 113 / 2.86% |
| `alloc_page_cached` | 226 / 2.45% | 64 / 1.62% |

`release` 和 `UserContext::run` 仍受定时器进入位置影响；全局 `memset` 的
宽存储版本已经在两个全新镜像中触发文件损坏和页表回收 fault；CRC 已经
完成一轮局部优化。因此不能只按表中数值从第一行机械向下修改。

## 同步与 I/O 侧证

以下时间是多个任务累计时间，不是可以直接相加的墙钟时间：

| 事件 | guard 基线预构建 | PTE 预构建 | guard 基线正式 | PTE 正式 |
| --- | ---: | ---: | ---: | ---: |
| clone child 写锁持有 | 70.80 s | 50.92 s | 30.37 s | 24.67 s |
| exit clear 写锁持有 | 59.35 s | 51.61 s | 25.25 s | 21.91 s |
| exec clear 写锁持有 | 35.54 s | 30.42 s | 12.10 s | 10.35 s |
| mprotect 写锁持有 | 48.30 s | 50.83 s | 21.58 s | 21.85 s |
| munmap 写锁持有 | 28.38 s | 26.86 s | 11.03 s | 9.74 s |
| ext4 锁等待 | 4.85 s | 3.60 s | 1.40 s | 0.74 s |
| folio 加载等待 | 3.59 s | 3.23 s | 0.40 s | 0.33 s |
| 同步块读取 | 5.81 s | 4.93 s | 2.30 s | 1.69 s |

这些事件大多下降而 BuildStorm 仍慢 1.35%，再次说明单轮墙钟受宿主调度、
QEMU 翻译和依赖长尾影响，不能把相关性直接当作 PTE 内联收益。

## 下一轮选择：水位线直接读取托管页总数

本轮预构建中，`CachedFile::memory_watermarks` 下的
`usize::saturating_sub` 有 223 个叶样本，占非 idle 样本 2.42%。静态
检查发现每次页缓存插入或 readahead 都会：

1. 扫描 64 个 per-CPU cache count 原子变量；
2. 获取全局 `BitmapPageAllocator` 锁读取 `used_pages()`；
3. 再扫描 64 个 count；
4. 再次获取同一全局锁读取 `available_pages()`。

其中 `used_pages() + available_pages()` 的结果恒等于分配器托管的总页数；
PCP refill/drain 只是在本地缓存与全局 bitmap 间搬运空闲页，不改变总量。
`GlobalAllocator` 已经维护初始化后只读的 `managed_pages`，并通过
`total_pages()` 无锁读取。

曾考虑增加一个全局 `allocated_pages`，让 `used_pages()` 和
`available_pages()` 都变成原子读取；但这要求每次 PCP 命中和释放都写同一
原子缓存行，会把所有 CPU 的 order-0 快路径重新连接到一个全局争用点。
在没有计数探针证明收益前不采用。

下一轮把水位线总量计算替换为 `allocator.total_pages()`。它直接移除一次
64 槽扫描和一次全局 bitmap 锁，不改变 `available_pages()`、分配/释放、
PCP 或任何页面所有权。当前评测使用 level-2 页分配器；静态审查将证明
其中 `used_pages() + available_pages() == total_pages()` 在 PCP 搬运前后
成立。`managed_pages` 在 level-1 初始化和 `add_memory` 时也同步更新，但
本轮性能结论只针对实际构建的 level-2 配置。

三遍静态审查完成前不构建。

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
