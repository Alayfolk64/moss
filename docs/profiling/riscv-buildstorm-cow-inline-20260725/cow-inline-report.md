# RISC-V COW 文件页内联槽迭代报告

## 结论

本轮撤销上一轮 `FilePageRefs` 的 64 路 `HashMap`，改成：

```text
每个分片：
    一个内联 (VirtAddr, FolioPageRef)
    一个按需分配的 BTreeMap overflow
```

同时将 `CowBackend::file_pages` 改为
`Option<Arc<FilePageRefs>>`。匿名映射不使用文件 folio，因而不再创建
64 分片文件页所有权表；文件映射仍保留原来的 64 路并发粒度。

本轮严格完成三遍静态检查后才构建。随后使用全新解压且 SHA-256
一致的官方 RISC-V 根文件系统，直接在宿主机运行 8 vCPU、8 GiB、
`-snapshot` QEMU，并完成整个 cagent、tg-xtask 预构建和正式
BuildStorm。结果为：

```text
cagent: 10/10 pass
tg-xtask prebuild: 19m25s
Cargo release build: 8m20s
BUILDSTORM_COMPILE: 515.89s
host full profile: 1771.332s
SVG: 112
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

局部目标确实改善。只统计调用栈中属于
`CowBackend` 的 BTree 查找叶样本：

| 阶段 | PCP/BTree | 内联槽+BTree | 变化 |
| --- | ---: | ---: | ---: |
| 预构建 | 131 | 102 | -22.1% |
| 正式 | 101 | 46 | -54.5% |

新增 `FilePageShard::insert/remove` 自身的叶样本很少：

| 阶段 | `insert` | `remove` |
| --- | ---: | ---: |
| 预构建 | 4 | 1 |
| 正式 | 1 | 0 |

上一轮 HashMap 的连续表扩容与大表析构回归也没有回来。直接位于
`alloc::dealloc -> NoPreemptIrqSave::release` 的延迟样本为：

| 阶段 | PCP/BTree | HashMap | 内联槽+BTree |
| --- | ---: | ---: | ---: |
| 预构建 | 79 | 301 | 62 |
| 正式 | 24 | 122 | 33 |

但本轮不能宣称端到端已经确定提速：

- BuildStorm 比 PCP 轮快 2.22 秒；
- 比 HashMap 轮慢 1.22 秒；
- 比此前水位总页数轮慢 5.52 秒；
- 完整运行时间比 PCP/HashMap 轮约快 15 秒，但主要指标没有形成足以
  排除运行波动的一致领先；
- 本轮同步文件系统 I/O 的任务累计时间偏高，预构建达到
  204.632 秒，会掩盖或放大其他微小变化。

准确结论是：

1. 内联槽有效降低了 Cow 文件页所有权表的 BTree 查找；
2. 它避免了 HashMap 的大块连续表扩容和析构成本；
3. 匿名 VMA 不再携带无用的文件页所有权表；
4. 未发现正确性或并发回归；
5. 当前数据只支持保留这项局部结构改进，不支持声称 BuildStorm
   已稳定变快；
6. 当前最大的可归因分配器热点已转为高阶 folio frame 分配。

## 修改内容

### 旧 BTree 结构

```rust
struct FilePageRefs {
    shards: [Mutex<BTreeMap<VirtAddr, FolioPageRef>>; FILE_PAGE_SHARDS],
}
```

每个分片第一次插入都需要创建 BTree 节点。由于相邻虚拟页按页号对
64 取模，常见的前 64 个页会分别进入不同分片，但旧实现仍会为这些
单元素分片各分配一个树节点。

### 被拒绝的 HashMap 结构

上一轮改用 64 个 `HashMap`。精确查找变短，但活跃分片需要分配
RawTable，增长时需要 reserve/rehash，VMA 销毁时还要释放每张连续表。
完整火焰图证明该成本大于节省的部分 BTree 查找，因此本轮已撤销。

### 当前内联槽结构

```rust
struct FilePageShard {
    inline: Option<(VirtAddr, FolioPageRef)>,
    overflow: BTreeMap<VirtAddr, FolioPageRef>,
}
```

状态规则为：

```text
空分片：
    inline = None
    overflow = empty

插入第一项：
    放入 inline
    不分配 BTree 节点

插入相同地址：
    替换 inline 中的引用

插入其他地址：
    放入 overflow BTree

删除 inline：
    取出 inline
    用 overflow.pop_first() 提升一项
```

删除时提升一项是必要的不变量。否则可能出现
`inline=None, overflow!=empty`，下一次插入就会把新键放进 inline，
而旧的相同键仍留在 overflow，形成重复键。当前实现始终保持：

```text
inline == None  =>  overflow.is_empty()
```

查找先比较内联地址，未命中才访问 BTree。大映射仍可扩展，不需要
线性 Vec 查找，也没有 HashMap 的连续大表。

### 匿名映射不创建 FilePageRefs

字段从：

```rust
file_pages: Arc<FilePageRefs>
```

改成：

```rust
file_pages: Option<Arc<FilePageRefs>>
```

构造规则为：

```text
Backend::new_cow(file): Some(Arc::new(FilePageRefs::new()))
Backend::new_alloc():   None
```

fork 子地址空间按父 backend 是否有文件页表决定是否创建空表。VMA
拆分仍通过 `Arc` 共享原表，因此不会改变同一映射拆分后的所有权语义。
`Option<Arc<T>>` 利用非空指针 niche，字段本身不比 `Arc<T>` 更大。

## 三遍静态检查

### 第一遍：状态与所有权

逐个检查了全部构造点和调用点：

- `new_cow` 始终创建文件页表；
- `new_alloc` 始终为 `None`；
- `clone` 保持同一个 `Arc`，适用于 VMA 拆分；
- `clone_for_fork` 创建独立空表，不复制父 PTE 所有权记录；
- 缺页 CAS 成功后才插入 `FolioPageRef`；
- `AlreadyMapped` 不插入重复引用；
- 文件页 COW 仍先校验记录物理地址与 PTE 一致；
- PTE 条件替换成功后才移除文件页引用；
- unmap 的错误路径会恢复刚取出的引用；
- fork 只把非文件直映页作为普通 COW 页共享。

内联槽、overflow 与 `Option<Arc<_>>` 的状态不变量闭合，没有残留
`HashMap` 或旧 `shard_and_key` 调用。

### 第二遍：并发、锁序与析构

- 仍按虚拟页号对 64 取模；
- 不会同时持有两个 `FilePageShard` 锁；
- `get()` 在分片锁内克隆稳定的 `FolioPageRef`，返回前释放锁；
- 文件 I/O、物理页分配和 4 KiB 复制均不持有分片锁；
- 缺页提交的 PTE CAS 与所有权记录插入仍处于同一分片锁边界；
- 文件页写 COW 的条件 PTE 替换与记录移除仍处于同一分片锁边界；
- 没有新增 AddrSpace 写锁；
- 没有改变 TLB 失效后才能释放 frame 的顺序；
- `FolioPageRef` 析构不会递归获取同一个文件页分片锁。

因此本轮缩短的是单元素分片的数据结构路径，没有把不同页或不同分片
重新串行化。

### 第三遍：机械检查

在第一次 `cargo fmt --all --check` 指出两处纯换行问题后，只执行了
机械格式化，然后完整重跑第三遍：

```text
cargo fmt --all --check: pass
cargo metadata --offline --no-deps: pass
git diff --check: pass
旧 HashMap/shard_and_key 搜索: empty
vendor checksum: pass
```

已修改 axalloc 文件的 SHA-256 仍与
`vendor/axalloc/.cargo-checksum.json` 一致：

```text
vendor/axalloc/src/lib.rs
f785a95315ded4ea643147811d936cc8428856209c7881905ee5dfcd99680b8b

vendor/axalloc/src/pcp.rs
0dc13cdad32f2d026ad16aaa7d74da2d596d3040404cc9e1842bde9e47c2b333
```

## 构建与测试口径

三遍静态检查结束后才执行：

```text
cargo clean -p starry-kernel \
  --target riscv64gc-unknown-none-elf --release
make PROFILE=y kernel-rv
```

构建成功，用时 12.01 秒：

```text
kernel-rv
e2dfa3bfa0791ff9b6e822a277570dbc01f1f03d3cf976a800d99622d1fce3f6

kernel-rv.profile.elf
a17d60c195fc41660fd4d2d4598b9cc66213e0219a464220b112093c2f906725
```

测试环境：

- Ubuntu 宿主直接运行 QEMU，不使用 Docker；
- RISC-V `virt`；
- 8 GiB；
- 8 vCPU；
- `-snapshot`；
- 官方根文件系统重新解压；
- 镜像大小 15,032,385,536 字节；
- 镜像 SHA-256：
  `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0`；
- CPU 10 Hz；
- off-CPU 1/8；
- ext4 1/16；
- page cache 1/32；
- AddrSpace 写锁持有 1/16；
- mprotect 1/32。

`BUILDSTORM_MINIBUILD fail` 是已有的独立 minibuild 项。本轮完整
tg-xtask 预构建、正式 release 构建和最终产物均成功。

## 时间对比

| 指标 | 水位总页数 | PCP fast path | Cow HashMap | Cow 内联槽 |
| --- | ---: | ---: | ---: | ---: |
| setup phase | 3.962 s | 4.018 s | 3.932 s | 3.746 s |
| prebuild phase | 1161.134 s | 1180.369 s | 1173.802 s | 1166.185 s |
| Cargo 预构建 | 19m20s | 19m39s | 19m32s | 19m25s |
| timed phase | 580.015 s | 596.314 s | 597.552 s | 583.185 s |
| Cargo release | 8m15s | 8m22s | 8m19s | 8m20s |
| BuildStorm | 510.37 s | 518.11 s | 514.67 s | 515.89 s |
| 宿主全流程 | 1756.302 s | 1786.279 s | 1786.292 s | 1771.332 s |

端点没有形成一致排序。例如本轮 timed phase 比 HashMap 少 14.37 秒，
但 BuildStorm 自报反而多 1.22 秒。因此不能把完整阶段差值全部归因于
内联槽。

工作量接近：

```text
PCP prebuild mprotect calls:       1,467,455
HashMap prebuild mprotect calls:   1,466,102
Inline prebuild mprotect calls:    1,461,591

PCP timed mprotect calls:            664,851
HashMap timed mprotect calls:        662,189
Inline timed mprotect calls:         663,635
```

## CPU 火焰图分析

以下样本排除 `task:idle`：

| 阶段 | PCP/BTree | HashMap | 内联槽+BTree |
| --- | ---: | ---: | ---: |
| 预构建非 idle | 9,301 | 9,330 | 9,231 |
| 正式非 idle | 3,966 | 3,943 | 3,929 |

### Cow 文件页索引

| 指标 | PCP 预构建 | 内联预构建 | PCP 正式 | 内联正式 |
| --- | ---: | ---: | ---: | ---: |
| Cow 栈内 BTree 叶样本 | 131 | 102 | 101 | 46 |
| `commit_missing_fault` inclusive | 583 | 622 | 274 | 270 |
| 全部 Cow inclusive | 1,522 | 1,525 | 702 | 656 |

解释：

- 正式阶段 Cow BTree 叶样本下降 55 个，是明确的局部收益；
- `FilePageShard` 新函数本身只有 1 个正式叶样本，内联比较并未成为
  新热点；
- `commit_missing_fault` 总路径没有同步下降，说明其余页表 CAS、
  folio 引用、锁和调度成本仍占主要部分；
- 全部 Cow 正式样本下降 6.6%，但预构建基本不变。

### IRQ-off 分配器延迟样本

`NoPreemptIrqSave::release` 是中断恢复后的补采样位置，宽度代表此前
IRQ-off 分配器临界区的累计成本，不能机械解释为 `release` 指令本身
很慢。

| 直接父路径 | PCP 预构建 | HashMap 预构建 | 内联预构建 | PCP 正式 | HashMap 正式 | 内联正式 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `alloc_folio_frame` | 425 | 422 | 446 | 83 | 99 | 82 |
| `GlobalAlloc::alloc` | 267 | 313 | 218 | 100 | 123 | 139 |
| anonymous `alloc_frame` | 135 | 150 | 150 | 41 | 58 | 48 |
| page-table `alloc_frame` | 72 | 72 | 139 | 15 | 12 | 14 |
| `UnmapGather::finish` | 59 | 43 | 78 | 21 | 30 | 28 |
| `alloc::dealloc` | 79 | 301 | 62 | 24 | 122 | 33 |

HashMap 最明显的析构回归已经消失。当前最大的稳定、可定位父路径是
`axfs::highlevel::file::alloc_folio_frame`：

```text
prebuild: 446 / 9231 = 4.83%
timed:      82 / 3929 = 2.09%
```

该函数的 order-0 页已经走 PCP；大 folio 虽有本地缓存，但缓存未命中时
会在全局 bitmap page allocator 锁内申请一批连续页。下一轮应先检查：

1. 本地高阶 folio cache 的实际命中、补充和回收；
2. 批量补充是否过度申请或频繁失败后退化为单个高阶分配；
3. bitmap 连续区搜索在 IRQ-off 临界区内扫描了多少位；
4. 是否能在不改变连续物理页要求的前提下，将高阶块缓存和全局搜索
   做成更接近 Linux per-CPU page-list 的批量路径。

在取得这些证据前，不应直接扩大固定缓存容量。

## 锁等待与 I/O

以下为所有任务累计时间，不是墙钟时间：

| 事件 | PCP 预构建 | HashMap 预构建 | 内联预构建 | PCP 正式 | HashMap 正式 | 内联正式 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| clone child 写锁 | 53.336 s | 60.990 s | 61.393 s | 23.897 s | 23.174 s | 21.236 s |
| clone parent 写锁 | 28.698 s | 36.184 s | 23.908 s | 11.090 s | 11.770 s | 11.736 s |
| exit clear 写锁 | 52.932 s | 44.406 s | 51.899 s | 22.131 s | 17.584 s | 22.249 s |
| exec clear 写锁 | 32.847 s | 41.723 s | 32.092 s | 10.413 s | 13.186 s | 10.245 s |
| mprotect 写锁 | 51.969 s | 53.147 s | 49.072 s | 22.697 s | 22.270 s | 21.127 s |
| munmap 写锁 | 27.922 s | 31.133 s | 26.278 s | 11.030 s | 9.439 s | 10.439 s |
| ext4 锁等待 | 4.461 s | 4.470 s | 5.042 s | 1.092 s | 1.060 s | 1.396 s |
| folio 加载等待 | 3.419 s | 3.168 s | 2.994 s | 0.328 s | 0.448 s | 0.409 s |
| 同步文件系统 I/O | 167.652 s | 186.105 s | 204.632 s | 80.268 s | 90.142 s | 83.900 s |

AddrSpace 写锁指标没有整体恶化。同步 I/O 的预构建累计时间却比 PCP
高 36.980 秒。它是并发任务累计等待，不能直接加到墙钟上，也不能由
本轮内联槽改造直接解释；但它足以说明几秒级端点差值不可靠。

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
- [exit clear 写锁](guest-aspace-exit-clear-write-hold-all.svg)
- [exec clear 写锁](guest-aspace-exec-clear-write-hold-all.svg)
- [clone child 写锁](guest-aspace-clone-child-write-hold-all.svg)
- [clone parent 写锁](guest-aspace-clone-parent-write-hold-all.svg)
