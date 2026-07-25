# RISC-V BuildStorm frame-table 锁优化报告

## 结论

本轮只修改 COW 物理页引用表的 64 个分片锁：

```text
SpinNoIrq<FrameTableRefCount>
              ↓
SpinNoPreempt<FrameTableRefCount>
```

frame table 只由同步的用户内存操作、fork、unmap 和后台回收任务访问，不从中断处理器进入。新锁仍禁止持锁任务被抢占和迁移，但不再为每次引用计数操作保存、关闭和恢复本地中断。

完整测试表明：

- 修改通过三遍静态检查和完整 BuildStorm，没有出现中断重入、锁递归、引用计数错误或丢失 profiling 样本；
- 正式阶段 `NoPreemptIrqSave::release` 从 19.45% 降至 11.89%；
- 将锁释放与现在能够被时钟中断采到的 BTreeMap 内部操作合并计算，目标路径样本下降约 15%～18%；
- BuildStorm 为 542.03 秒，位于前两轮 539.38 秒和 544.64 秒之间；
- 三阶段总时间为 1864.56 秒，三轮几乎相同，不能声称获得稳定的端到端提速。

这轮修改对目标 CPU 路径有效，但当前端到端结果属于持平。

## 修改内容

文件：

```text
kernel/src/mm/aspace/backend/cow.rs
```

frame table 保持 64 路分片，数据结构仍是：

```text
BTreeMap<PhysAddr, usize>
```

只把分片锁从关闭中断的自旋锁换成禁止抢占的自旋锁。以下内容均未改变：

- frame 的引用所有权规则；
- 计数从 0 到 1 的初始化；
- fork 发布子 PTE 前预留引用；
- COW exclusive commit 在同一分片锁内验证计数并更新 PTE；
- 最后一个引用归零后才释放物理页；
- 页复制、文件读取、物理页分配和释放均在显式 frame-table guard 外执行；
- 分片数量仍为 64，没有退化成全局大锁。

## 为什么可以不关闭中断

`SpinNoPreempt` 与 `SpinNoIrq` 都保证当前持锁任务不会被调度到别的 CPU。区别是：

- `SpinNoIrq` 还会保存并关闭当前 CPU 的中断，释放时恢复中断；
- `SpinNoPreempt` 保持中断开启。

若中断处理器会再次访问同一把锁，保持中断开启会在本 CPU 上形成递归死锁。因此本轮在构建前先审计了 `frame_table()` 的全部调用点，而不是仅凭火焰图替换锁。

审计结果是：

- `frame_table()` 是 `cow.rs` 内部私有函数；
- 调用方只位于缺页、fork、unmap、页回收和引用释放路径；
- 没有 trap/IRQ handler、软中断回调或异步回调调用这些函数；
- 持锁区内不会分配物理页，也不会执行可能回到 frame table 的回收；
- TLB 批处理使用固定容量容器，不会在 frame-table guard 内触发动态内存分配。

所以这里保留“不可抢占”，去掉“不必要的关中断”是成立的。这个结论只适用于 frame table，不能推广到物理页分配器的 PCP 锁。

## 三遍静态检查

### 第一遍：中断上下文与调用图

逐一检查 `frame_table()`、`reference_frame()`、`drop_referenced_frame()`、fault prepare/commit、fork 和 unmap 的调用关系：

- 没有中断注册项；
- 没有 trap handler 直接调用；
- 没有 `.await` 或可能跨调度点长期保存 guard 的代码；
- 后台 reclaim worker 是普通内核任务，不是中断上下文。

### 第二遍：锁顺序、重入和生命周期

重点检查 frame-table guard 内是否可能进入以下路径：

- frame allocator；
- frame deallocator；
- 页复制；
- 文件 I/O；
- TLB shootdown；
- 其他会重新取得 frame-table 分片锁的函数。

结果是实际分配、释放、复制和 I/O 都发生在 guard 之外。exclusive COW 只在短 guard 内读取计数并条件修改已有 PTE，不引入反向锁顺序。

### 第三遍：并行粒度与代码一致性

- 仍为 64 个独立分片，不引入进程级或全局锁；
- 不同分片的 frame 引用操作仍可并行；
- `cargo fmt --all -- --check` 通过；
- `git diff --check` 通过；
- 三遍检查完成后才执行 `make PROFILE=y kernel-rv`，构建通过。

## 测试口径

本轮从官方压缩镜像重新独立解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
```

使用的测试镜像：

```text
tmp/profile-rv-frame-lock-preempt-20260725/sdcard-rv.img
```

运行条件：

- 不使用 Docker；
- QEMU RISC-V `virt`；
- 8 GiB 内存；
- 8 vCPU；
- `-snapshot`；
- 完整运行 cagent、空 target 的 tg-xtask 预构建、正式 BuildStorm 和退出阶段；
- CPU、off-CPU、ext4、页缓存、同步块 I/O 和地址空间锁持有全程采样。

采样状态：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

## 端到端时间

| 指标 | folio-batch | direct frame ref | frame lock preempt |
| --- | ---: | ---: | ---: |
| tg-xtask 预构建 | 20 分 59 秒 | **20 分 38 秒** | 20 分 48 秒 |
| 正式 Cargo 编译 | **529.39 秒** | 534.76 秒 | 532.20 秒 |
| BuildStorm | **539.38 秒** | 544.64 秒 | 542.03 秒 |
| profile 预构建阶段 | 1260.83 秒 | **1239.52 秒** | 1249.37 秒 |
| profile 正式阶段 | **599.93 秒** | 621.33 秒 | 610.57 秒 |
| profile 三阶段总时间 | 1865.18 秒 | 1865.40 秒 | **1864.56 秒** |
| 宿主全流程 | **1876.16 秒** | 1876.27 秒 | 1876.28 秒 |

宿主全流程脚本以 15 秒间隔轮询，所以三轮宿主时间都落在相同量化区间。guest 内部纳秒时间更精确，但三轮总差异仍不足 1 秒。

因此，这些数据支持“目标开销下降”，不支持“完整流程已经显著变快”。

## CPU 火焰图

以下占比以阶段内非 idle CPU 样本为分母。

### 主要路径

| 路径 | 预构建 folio | 预构建 direct | 预构建本轮 | 正式 folio | 正式 direct | 正式本轮 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| page fault 总路径 | 28.46% | 27.89% | **26.60%** | 27.67% | **25.29%** | 27.10% |
| `CowFaultPlan::execute` | 11.47% | 9.81% | **9.47%** | 11.57% | 9.55% | **9.50%** |
| `CowBackend::commit_fault` | **6.11%** | 6.86% | 6.34% | **6.19%** | 6.73% | 7.00% |
| COW unmap | 10.79% | **10.51%** | 10.83% | 10.32% | 10.02% | **9.80%** |
| resident reclaim | 14.70% | **13.97%** | 14.11% | 14.15% | 13.40% | **13.33%** |
| clone | 3.87% | 3.27% | **3.23%** | 4.79% | **3.07%** | 3.39% |

### 锁和 frame-table 索引

| 路径 | 预构建 direct | 预构建本轮 | 正式 direct | 正式本轮 |
| --- | ---: | ---: | ---: | ---: |
| `NoPreemptIrqSave::release` | 23.10% | **14.07%** | 19.45% | **11.89%** |
| `NoPreempt::release` | 未单列 | 0.31% | 未单列 | 0.40% |
| BTreeMap 查找 | 2.31% | 5.71% | 2.46% | 6.94% |
| 三项合计 | 25.41% | **20.09%** | 21.91% | **19.23%** |

不能把 BTreeMap 占比上升直接解释成 BTreeMap 变慢。上一轮 frame-table 持有 `SpinNoIrq` 时，本地时钟中断不能在临界区内部采样，样本更容易落在恢复中断的 release 附近。本轮保持中断开启后，profiler 可以采到临界区内的 `BTreeMap::get_mut` 和查找代码。

用原始样本合并计算更直观：

```text
预构建：2683 + 268 = 2951
本轮：  1686 + 37 + 684 = 2407，下降约 18.4%

正式：  1006 + 127 = 1133
本轮：  596 + 20 + 348 = 964，下降约 14.9%
```

这说明修改消除了实际 CPU 成本，同时也让 profiler 更准确地暴露出下一层的 BTreeMap 索引成本。

火焰图：

- [预构建 CPU](./guest-cpu-prebuild.svg)
- [正式阶段 CPU](./guest-cpu-timed.svg)
- [完整 CPU](./guest-cpu-all.svg)

## 等待和地址空间持锁

| 指标 | 预构建 direct | 预构建本轮 | 正式 direct | 正式本轮 |
| --- | ---: | ---: | ---: | ---: |
| off-CPU 累计估算 | 57264.27 秒 | **49043.94 秒** | 38313.23 秒 | **23326.35 秒** |
| ext4 inclusive | **39.34 秒** | 43.55 秒 | 51.87 秒 | **42.99 秒** |
| ext4 全局锁等待 | **6.33 秒** | 10.30 秒 | 2.19 秒 | **1.71 秒** |
| page-cache inclusive | **150.49 秒** | 192.01 秒 | 43.78 秒 | **38.66 秒** |
| 同步块读取 | 6.42 秒 | 6.72 秒 | 2.54 秒 | **2.38 秒** |
| folio load wait | **3.91 秒** | 4.07 秒 | **0.59 秒** | 0.63 秒 |

off-CPU 是所有任务等待时间的累计估算，多个任务会同时等待，不能直接加到墙钟时间。ext4 和 page-cache 也是 inclusive 指标。它们适合判断负载结构和数量级，不应单独用来宣称端到端收益。

地址空间写锁：

| 事件 | 预构建 direct | 预构建本轮 | 正式 direct | 正式本轮 |
| --- | ---: | ---: | ---: | ---: |
| clone parent | 43.34 秒 | **42.03 秒** | **17.13 秒** | 21.06 秒 |
| clone child | **80.03 秒** | 85.11 秒 | **35.51 秒** | 38.58 秒 |
| exit clear | **103.55 秒** | 108.08 秒 | 40.26 秒 | **39.90 秒** |
| exec clear | **85.21 秒** | 90.34 秒 | 38.37 秒 | **37.40 秒** |
| munmap | 34.81 秒 | **33.38 秒** | 13.88 秒 | **12.64 秒** |

这些值小幅双向波动，没有证据表明更换 frame-table 锁扩大了地址空间锁临界区。

等待图：

- [正式 off-CPU](./guest-offcpu-timed.svg)
- [正式锁等待](./guest-offcpu-lock-timed.svg)
- [正式 ext4 锁等待](./guest-ext4-lock-wait-timed.svg)
- [正式 I/O](./guest-fs-io-latency-timed.svg)
- [正式 clone child 写锁](./guest-aspace-clone-child-write-hold-timed.svg)
- [正式 exit clear 写锁](./guest-aspace-exit-clear-write-hold-timed.svg)

## 下一轮依据

本轮让 profiler 可以在 frame-table 临界区内部采样，BTreeMap 查找由隐藏在 IRQ-disabled 区间中的成本变成了可见热点：

```text
预构建：684 个样本，非 idle CPU 的 5.71%
正式：  348 个样本，非 idle CPU 的 6.94%
```

当前引用表按物理页地址维护引用计数，物理页号本身可以直接作为数组索引；用平衡树做每次 `O(log n)` 查找并不理想。下一轮最有证据支持的方向是：

1. 在内存初始化完成后，根据实际 RAM 物理页范围建立稠密或分段的引用计数表；
2. 保留 64 路分片锁和现有引用生命周期；
3. 用物理页号直接定位分片及槽位，将查找从 `O(log n)` 改为 `O(1)`；
4. 不改变页分配器、页回收或 COW 语义；
5. 对 RISC-V 连续内存和 LoongArch 带低端区/空洞的物理布局分别做边界审查。

该改动比换锁大，必须先完成数据布局、初始化顺序、物理地址边界、计数溢出和锁顺序三遍静态检查，再允许构建测试。

## 产物

- [summary.json](./summary.json)
- [console.log](./console.log)
- [完整 CPU 火焰图](./guest-cpu-all.svg)
- [完整 off-CPU 火焰图](./guest-offcpu-all.svg)
- [完整锁等待火焰图](./guest-offcpu-lock-all.svg)
- [完整 I/O 火焰图](./guest-fs-io-latency-all.svg)
