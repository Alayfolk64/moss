# RISC-V COW FilePageRefs HashMap 迭代报告

## 结论

本轮针对上一轮正式 CPU 火焰图中的
`CowBackend::commit_missing_fault -> FilePageRefs -> BTree::find_key_index`
热点，将 COW 私有文件页的 64 路所有权索引从 `BTreeMap` 改为
`hashbrown::HashMap`。

修改保留了原有的：

- 64 路分片；
- 每个虚拟页所属的分片；
- 每个分片的独立 `Mutex`；
- PTE CAS 与所有权记录更新之间的锁边界；
- `FolioPageRef` 的获取、移除和析构时机；
- TLB 失效及物理页释放顺序。

三遍静态审查、profiling 内核构建、全新官方镜像、cagent 10/10、
完整 tg-xtask 预构建和正式 BuildStorm 均通过。生成 112 张同口径
SVG，采样零丢失：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

局部查找目标有明显下降：

| 阶段 | PCP 轮 BTree 叶样本 | HashMap 轮 BTree 叶样本 | 下降 |
| --- | ---: | ---: | ---: |
| 预构建 | 250 / 2.69% | 110 / 1.18% | 56.0% |
| 正式 | 160 / 4.03% | 50 / 1.27% | 68.8% |

但它不是成功的端到端优化。HashMap 引入了新的表分配、扩容和销毁成本：

| 阶段 | PCP 轮 `NoPreemptIrqSave::release` | HashMap 轮 | 增量 |
| --- | ---: | ---: | ---: |
| 预构建 | 1317 / 14.16% | 1612 / 17.28% | +22.4% |
| 正式 | 414 / 10.44% | 605 / 15.34% | +46.1% |

这些宽样本不能机械归因于 `release` 那一行。RISC-V 定时器在 IRQ
关闭期间不能进入 profiler，常在恢复中断后的第一条指令补采样，因此
宽度代表此前 IRQ-off 分配器临界区的累计成本。调用栈进一步给出直接
证据：

- 预构建有 32 个延迟样本位于
  `CowBackend::commit_missing_fault -> RawTable::reserve -> dealloc`；
- 正式阶段同一路径有 7 个；
- 预构建 reclaim worker 销毁 `MemoryArea` 的 dealloc 延迟样本从
  8 增至 173，`munmap` 销毁 `MemoryArea` 从 2 增至 42；
- 正式阶段对应值从 1 增至 32、从 1 增至 22。

HashMap 的大块连续表虽然使精确查找更短，却让扩容和销毁更重。同步文件
系统 I/O 累计时间也没有改善。端到端结果仍在前三轮噪声区间：

```text
tg-xtask prebuild: 19m32s
Cargo release build: 8m19s
BUILDSTORM_COMPILE: 514.67s
host full profile: 1786.292s
```

准确结论是：

1. 旧 BTree 的精确查找确实可以优化；
2. 64 路独立 HashMap 不是合适替代，它把 CPU 成本转移到内存管理；
3. BuildStorm 比上一轮快 3.44 秒、比水位线轮慢 4.30 秒，不能从中
   声称确定收益；
4. 本轮 HashMap 修改应撤销；
5. 下一轮使用“不新增大表”的小型内联槽，未命中再回退到原 BTree，
   保留大映射的可扩展性。

## 修改内容

旧结构：

```rust
struct FilePageRefs {
    shards: [Mutex<BTreeMap<VirtAddr, FolioPageRef>>; FILE_PAGE_SHARDS],
}
```

本轮结构：

```rust
struct FilePageRefs {
    shards: [Mutex<HashMap<usize, FolioPageRef>>; FILE_PAGE_SHARDS],
}
```

哈希键使用完整虚拟地址，分片仍使用虚拟页号：

```rust
let address = vaddr.as_usize();
let page = address / PAGE_SIZE_4K;
(&self.shards[page % FILE_PAGE_SHARDS], address)
```

没有使用 `page / FILE_PAGE_SHARDS` 作为键。这样即使错误调用者传入同一
4 KiB 页内的不同非对齐地址，也不会被静默合并，保持旧 BTree 的精确键
语义。

64 个 HashMap 共享一次生成的随机 `DefaultHashBuilder`：

```rust
let hash_builder = DefaultHashBuilder::default();
core::array::from_fn(|_| {
    Mutex::new(HashMap::with_hasher(hash_builder.clone()))
})
```

空表不分配，但每个活跃分片第一次插入会分配，容量增长会重新分配，
`FilePageRefs` 销毁时每个活跃分片都要释放连续 RawTable。

## 三遍静态审查

### 第一遍：所有权状态机

逐个核对了全部 `get/insert/remove` 调用点：

- 缺页提交仍在 `map_4k_concurrent()` 成功后才插入 `FolioPageRef`；
- `AlreadyMapped` 不插入重复引用；
- 文件页写时复制仍先验证已记录页与 PTE 的物理地址一致；
- `remap_4k_concurrent_if()` 成功后才移除文件页引用；
- unmap 仍先取出所有权，PTE 不一致时恢复同一引用；
- fork 仍只用精确地址查询干净文件页。

分片公式没有变化，完整虚拟地址键保持旧语义。

### 第二遍：并发、锁序和析构

- 仍只有一把目标分片锁，不会同时锁两个分片；
- 缺页提交中的 PTE CAS 仍在同一分片锁内；
- 文件页 COW 的 PTE 条件替换和记录移除仍在同一分片锁内；
- 没有新增 AddrSpace 写锁；
- 没有新增跨分片锁序；
- HashMap 首次插入可能在分片锁内分配，但旧 BTree 首节点也在该锁内
  分配；
- 删除不主动 shrink，容量供后续插入复用；
- HashMap 析构仍由 `Arc<FilePageRefs>` 最后一个引用负责。

静态审查能证明并发正确性，但不能提前证明连续表扩容和销毁足够便宜；
这正是本轮完整采样发现的性能反例。

### 第三遍：格式、依赖和 vendor 校验

- `cargo fmt --all --check` 通过；
- `cargo metadata --offline --no-deps` 通过；
- `git diff --check` 通过；
- kernel 已直接依赖 `hashbrown 0.16.1`，默认 feature 包含
  `default-hasher`、`inline-more` 和 `allocator-api2`；
- `DefaultHashBuilder` 使用 foldhash 随机状态；
- axalloc 两个已修改 vendor 文件 SHA-256 与
  `.cargo-checksum.json` 一致：

```text
vendor/axalloc/src/lib.rs
f785a95315ded4ea643147811d936cc8428856209c7881905ee5dfcd99680b8b

vendor/axalloc/src/pcp.rs
0dc13cdad32f2d026ad16aaa7d74da2d596d3040404cc9e1842bde9e47c2b333
```

## 构建和测试口径

三遍审查结束后才执行：

```text
cargo clean -p starry-kernel \
  --target riscv64gc-unknown-none-elf --release
make PROFILE=y kernel-rv
```

构建成功，用时 13.04 秒：

```text
kernel-rv
sha256=42d41ba3b974686d74c9387878a897d247e26e4dbf450ebdcda603d5b7d6ee9c

kernel-rv.profile.elf
sha256=2977082992038137dd5fb1ecb9899b8ffec0bdb6f52775665edf8adf38db55a9
```

完整测试：

- Ubuntu 宿主直接运行 QEMU，不使用 Docker；
- RISC-V `virt`；
- 8 GiB；
- 8 vCPU；
- `-snapshot`；
- 根文件系统从官方压缩包重新解压；
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
BUILDSTORM_MINIBUILD: fail
tg-xtask prebuild: 19m32s
Cargo release build: 8m19s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=514.67 cores=8
artifact bytes=1681000
profile host elapsed=1786.292s
SVG=112
```

`BUILDSTORM_MINIBUILD fail` 是已有的独立 minibuild 项；完整 tg-xtask
预构建和正式 BuildStorm 成功。

## 时间对比

| 指标 | 水位总页数 | PCP fast path | Cow HashMap | 相对 PCP |
| --- | ---: | ---: | ---: | ---: |
| setup phase | 3.962 s | 4.018 s | 3.932 s | -0.087 s |
| 预构建 phase | 1161.134 s | 1180.369 s | 1173.802 s | -6.568 s |
| Cargo 预构建自报 | 19m20s | 19m39s | 19m32s | -7 s |
| timed phase | 580.015 s | 596.314 s | 597.552 s | +1.238 s |
| BuildStorm 自报 | 510.37 s | 518.11 s | 514.67 s | -3.44 s |
| Cargo 正式构建自报 | 8m15s | 8m22s | 8m19s | -3 s |
| 宿主全流程 | 1756.302 s | 1786.279 s | 1786.292 s | +0.013 s |

三个关键端点没有一致方向：预构建略快、timed phase 略慢、宿主全流程
几乎完全相同。工作量也有小幅变化：

```text
mprotect prebuild calls: 1,467,455 -> 1,466,102 (-0.09%)
mprotect timed calls:      664,851 ->   662,189 (-0.40%)
```

因此不能把 3.44 秒 BuildStorm 差值归因于 HashMap。

## CPU 火焰图

以下统计排除 `task:idle`。

| 阶段 | PCP 非 idle | HashMap 非 idle |
| --- | ---: | ---: |
| 预构建 | 9,301 | 9,330 |
| 正式 | 3,966 | 3,943 |

### 目标结构

| 叶路径 | PCP 预构建 | HashMap 预构建 | PCP 正式 | HashMap 正式 |
| --- | ---: | ---: | ---: | ---: |
| BTree `find_key_index` | 250 / 2.69% | 110 / 1.18% | 160 / 4.03% | 50 / 1.27% |
| HashMap 函数 | 13 / 0.14% | 87 / 0.93% | 5 / 0.13% | 33 / 0.84% |
| 两者合计 | 263 / 2.83% | 197 / 2.11% | 165 / 4.16% | 83 / 2.11% |

仅看查找/插入函数，正式阶段减少约一半。但完整 Cow 相关栈从：

```text
prebuild: 931 -> 743 samples
timed:    455 -> 263 samples
```

下降没有等比例转化为总 CPU 或墙钟收益，因为分配器成本增加。

### 当前主要非 idle 叶路径

| 叶路径 | 预构建 | 正式 |
| --- | ---: | ---: |
| `NoPreemptIrqSave::release` 延迟采样 | 1612 / 17.28% | 605 / 15.34% |
| `CurrentTask::from_raw_ptr` | 727 / 7.79% | 265 / 6.72% |
| `UserContext::run` | 616 / 6.60% | 172 / 4.36% |
| lwext4 `memset` | 523 / 5.61% | 219 / 5.55% |
| `handle_page_fault_concurrent_inner` | 207 / 2.22% | 85 / 2.16% |
| `u64::bitor` | 213 / 2.28% | 81 / 2.05% |
| SBI call | 164 / 1.76% | 75 / 1.90% |
| BTree `find_key_index` | 110 / 1.18% | 50 / 1.27% |

`NoPreemptIrqSave::release` 的增加比省下的 BTree 叶样本更大，是拒绝当前
实现的直接 CPU 依据。

## 分配与销毁证据

`NoPreemptIrqSave::release` 的直接父帧显示：

| 父路径 | PCP 预构建 | HashMap 预构建 | PCP 正式 | HashMap 正式 |
| --- | ---: | ---: | ---: | ---: |
| `GlobalAlloc::alloc` | 267 | 313 | 100 | 123 |
| `alloc::dealloc` | 79 | 301 | 24 | 122 |
| `alloc_folio_frame` | 425 | 422 | 83 | 99 |
| anonymous `alloc_frame` | 135 | 150 | 41 | 58 |

HashMap 特有或明显放大的调用栈：

| 调用栈 | 预构建 | 正式 |
| --- | ---: | ---: |
| `commit_missing_fault -> RawTable::reserve -> dealloc` | 32 | 7 |
| reclaim worker 销毁 `MemoryArea` | 173 | 32 |
| `munmap` 销毁 `MemoryArea` | 42 | 22 |

PCP 轮后两项分别只有：

```text
reclaim worker: prebuild 8, timed 1
munmap:         prebuild 2, timed 1
```

这不是 HashMap 查找函数本身变慢，而是大块连续表的生命周期成本。继续
优化 HashMap 的探测指令无法解决该问题。

## 锁与 I/O

以下为任务累计时间，不是墙钟时间。

| 事件 | PCP 预构建 | HashMap 预构建 | PCP 正式 | HashMap 正式 |
| --- | ---: | ---: | ---: | ---: |
| clone child 写锁持有 | 53.336 s | 60.990 s | 23.897 s | 23.174 s |
| clone parent 写锁持有 | 28.698 s | 36.184 s | 11.090 s | 11.770 s |
| exit clear 写锁持有 | 52.932 s | 44.406 s | 22.131 s | 17.584 s |
| exec clear 写锁持有 | 32.847 s | 41.723 s | 10.413 s | 13.186 s |
| mprotect 写锁持有 | 51.969 s | 53.148 s | 22.697 s | 22.270 s |
| munmap 写锁持有 | 27.922 s | 31.133 s | 11.030 s | 9.439 s |
| ext4 锁等待 | 4.462 s | 4.470 s | 1.092 s | 1.060 s |
| folio 加载等待 | 3.419 s | 3.169 s | 0.328 s | 0.448 s |
| 同步文件系统 I/O | 167.652 s | 186.107 s | 80.268 s | 90.142 s |

同步文件系统 I/O 是：

```text
page-cache-inclusive
+ ext4-inclusive
+ block-read-sync
+ block-write-sync
```

预构建增加 18.455 秒，正式增加 9.874 秒。它们是并发任务累计时间，
不能直接加到墙钟上，但同样没有支持“HashMap 使完整流程更快”的证据。

## 下一轮方案

下一轮撤销 HashMap，不继续给大表做微调。采用每分片一个内联小槽：

```text
精确键命中内联槽
    -> O(1)，不分配

内联槽为空
    -> 直接安装，不分配

键不相同
    -> 回退原 BTreeMap
```

该方案的边界：

1. 只给每个分片一个内联项，限制固定内存增量；
2. 小映射或每分片首个页不创建 BTree 节点；
3. 大映射继续使用原 BTree，避免 Vec 的 O(n) 删除；
4. 不创建连续 RawTable，不再有 rehash 和大表销毁；
5. remove 命中内联槽时直接 `take()`，不需要访问 BTree；
6. 不做跨分片操作，不改变现有并发粒度。

该方案仍需三遍静态检查后才能构建和完整测试。若它不能同时降低 BTree
与分配器样本，也应拒绝，而不是只保留局部漂亮的火焰图。

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
