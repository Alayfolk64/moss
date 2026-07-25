# Moss `aws-lc-sys` build-script 定向性能报告

## 结论

本次只测量 `aws-lc-sys` 的 `build-script` 从 `execve` 成功到最后一个线程退出的完整生命周期，不包含此前或此后的 crate。精确测量窗口为 **688.436 秒**。

与未插桩的同镜像 Moss 结果相比，探针带来的时间增量为 21.636 秒，即 **3.24%**。因此，本文用未插桩的 666.8 秒与 Linux 的 420.8 秒比较总时间，用插桩结果分析 Moss 内部的时间分布。

最重要的发现是：

1. 这不是 `aws-lc-sys` 被限制成单核。采样期间宿主 QEMU 进程平均占用 **782.1% CPU**，相当于约 7.82 个宿主逻辑核；guest CPU 样本估算也有约 7.14 个活动 vCPU。
2. Moss 与 Linux 相差的 246.0 秒不能全部归因于 ext4 或同步磁盘 I/O。ext4 全局锁等待为 7.32 个 task-second，同步块读写等待合计约 5.21 个 task-second，它们是真实开销，但量级不足以单独解释差距。
3. 当前最大热点是 CPU 上的内存生命周期处理。`handle_page_fault_concurrent` 出现在 **29.31%** 的 CPU 样本中，后台页回收出现在 **12.92%** 的样本中；两类互不重叠的顶层工作合计达到 **42.23%**。
4. 缺页路径内部还反复出现 COW fault 的 execute/commit、PTE 遍历、BTree 查询、解除映射和 TLB/SBI 刷新。这说明机器虽然跑满了 8 核，但相当一部分计算资源消耗在 Moss 的虚拟内存管理，而不是编译器本身。
5. 旧的 `mprotect` 全局元数据锁瓶颈已经基本消除：约 100.8 万次 `mprotect` 的 AddrSpace 锁等待合计只有 30.86 毫秒。`mprotect` 仍有约 53.30 个 task-second 的 CPU 工作，但不能单独解释 246 秒的墙钟时间差。
6. `exit clear`、`exec clear` 和 `clone` 的地址空间操作仍然昂贵。尤其 clone 的抽样平均写锁持有时间约 97–100 毫秒，exit/exec 清理也累计占用大量任务时间。下一步应优先合并页表遍历、页回收和 TLB shootdown，而不是继续围绕已经很小的 `mprotect` 锁等待做文章。

## 对比基线

| 环境 | `aws-lc-sys` build-script | 相对 Linux |
| --- | ---: | ---: |
| x86_64 物理机直接编译 | 20.03 秒 | 不作跨架构性能比值 |
| RISC-V Linux/QEMU | 420.8 秒 | 基线 |
| RISC-V Moss/QEMU，未插桩 | 666.8 秒 | +246.0 秒，+58.5% |
| RISC-V Moss/QEMU，本次定向插桩 | 688.436 秒 | 比未插桩 Moss 慢 3.24% |

Linux 与 Moss 的公平差值应使用两个未插桩结果，即 **246.0 秒**。本次 688.436 秒只用于 Moss 内部归因。

## 测量范围与口径

内核只在可执行文件路径精确匹配以下形式时开始采样：

```text
/build/aws-lc-sys-.../build-script-main
/build/aws-lc-sys-.../build-script-build
```

开始点是对应进程完成 `execve` 并安装新地址空间之后；结束点是同一 PID 的最后一个线程退出。这样能够覆盖 build-script 创建编译器子进程、等待子进程和退出清理的完整过程，又不会把相邻 crate 混进来。

本次配置为：

- RISC-V 64，8 vCPU，8 GiB 内存；
- QEMU 直接在宿主机运行，不套 Docker；
- 根文件系统使用 qcow2 overlay，并启用 QEMU snapshot；
- 每次从没有 Cargo target 缓存的同一起点运行；
- CPU：每 vCPU 约 10 Hz；
- off-CPU：1/8 抽样，统计时乘 8；
- ext4：1/16 抽样，统计时乘 16；
- page cache：1/32 抽样，统计时乘 32；
- `mprotect`：1/32 抽样，调用次数另行精确计数；
- 普通 AddrSpace 写锁持有：1/16 抽样；
- exit/exec clear：完整记录，不抽样。

“乘 8/16/32”只是在汇总抽样事件时恢复总体量级。例如只记录约每 32 次 page-cache 事件中的一次，那么被记录事件的耗时在统计时乘 32。它不会凭空增加精度，因此报告同时保留样本数、平均值和最大值。

本轮没有 CPU、等待事件或 pending 事件丢失，采样缓冲区也没有跳过记录。

## CPU 是否真正并行

正式窗口内宿主 QEMU 进程的 CPU 占用如下：

| 指标 | CPU 占用 |
| --- | ---: |
| 平均 | 782.11% |
| 中位数 | 781.85% |
| 最低 | 768.07% |
| 最高 | 797.77% |

因此，Moss 并没有把整个 build-script 卡成单核。这里真正的问题是：8 个核在做什么。

CPU 样本按任务名分布如下：

| 任务 | 样本 | 比例 |
| --- | ---: | ---: |
| `rustc` | 4,732 | 42.72% |
| 页回收相关 worker | 1,450 | 13.09% |
| `riscv64-linux-gnu-*` 编译驱动 | 1,348 | 12.17% |
| `cc1` | 1,246 | 11.25% |
| idle | 1,186 | 10.71% |
| `cargo` | 470 | 4.24% |
| LLVM `opt` | 289 | 2.61% |

若只依据 idle 样本粗略估算，平均活动 vCPU 数为：

```text
8 × (1 - 10.71%) ≈ 7.14
```

这与宿主的 782% QEMU CPU 占用互相印证。

## CPU 热点

下表是调用栈中包含指定路径的 CPU 样本比例。不同路径可能处在同一条调用栈中，因此不能把所有行直接相加；但“缺页处理”和独立的页回收 worker 属于不同顶层任务，可以用于判断总体方向。

| 热点路径 | CPU 样本占比 | 含义 |
| --- | ---: | --- |
| `handle_page_fault_concurrent` | **29.31%** | 前台编译任务发生的文件页、匿名页和 COW 缺页 |
| 后台 reclaim worker | **12.92%** | 回收页、扫描缓存和释放映射 |
| `CowFaultPlan::execute` | 10.88% | COW 缺页锁外分配或复制阶段 |
| `Backend::commit_fault` | 9.33% | 重新验证并安装 PTE |
| `CowBackend::commit_fault` | 8.59% | COW 后端提交路径 |
| `unmap_present` | 9.59% | 清除已存在的 PTE 并收集待释放页 |
| 页表遍历 | 6.42% | 查找或修改多级页表 |
| BTree 操作 | 6.62% | VMA、页引用或映射元数据查询 |
| `flush_tlb` | 4.10% | TLB 失效处理 |
| SBI 调用 | 3.82% | RISC-V 跨核 TLB/IPI 等固件调用 |
| 文件系统相关路径 | 18.93% | page cache、ext4 与文件读取；与缺页路径重叠 |
| `CachedFile` | 11.53% | Moss 文件页缓存操作；与上项重叠 |
| 物理页分配器 | 4.35% | 缺页与复制时分配物理页 |
| `clone_map` | 3.35% | fork/clone 时复制地址空间元数据 |

在 3,247 个包含并发缺页处理的 CPU 样本中：

| 触发任务 | 样本 | 占缺页样本 |
| --- | ---: | ---: |
| `rustc` | 2,225 | 68.53% |
| `cc1` | 489 | 15.06% |
| 编译器驱动 | 302 | 9.30% |

这说明缺页成本确实直接落在编译器关键路径上，而不只是无关后台任务。

对应火焰图：

- [CPU 全路径](guest-cpu-timed.svg)
- [page cache、ext4 与同步块 I/O 组合图](guest-fs-io-latency-timed.svg)

## 文件缺页、ext4 与同步块 I/O

这里的时间是所有任务的等待或执行时间之和，即 task-second。多个任务可以同时等待，所以它不等于墙钟时间；inclusive 事件还可能相互嵌套，例如一次文件缺页可能同时包含 page cache、ext4 和块设备等待，不能把各行直接相加。

| 事件 | 估算总 task-time | 记录样本 | 估算平均值 | 样本最大值 |
| --- | ---: | ---: | ---: | ---: |
| page-cache inclusive | 105.256 秒 | 61,393（1/32） | 53.58 微秒 | 61.91 毫秒 |
| ext4 inclusive | 27.455 秒 | 10,474（1/16） | 163.83 微秒 | 49.71 毫秒 |
| ext4 全局锁等待 | 7.322 秒 | 2,463 | 2.973 毫秒 | 135.78 毫秒 |
| folio 加载等待 | 2.262 秒 | 675 | 3.351 毫秒 | 56.41 毫秒 |
| 同步块读取 | 4.593 秒 | 1,516 | 3.029 毫秒 | 90.69 毫秒 |
| 同步块写入 | 0.613 秒 | 233 | 2.630 毫秒 | 49.70 毫秒 |

通用 mutex 等待合计 7.663 个 task-second。按调用栈分类，其中 **96.26%** 包含 ext4 路径，说明 ext4 全局锁仍是剩余 mutex 睡眠等待的主体；但通用 mutex 和 ext4 锁是嵌套统计，不能相加。

因此：

- ext4 锁仍值得优化；
- 同步块 I/O 仍会造成毫秒级长尾；
- 但两者的累计等待量都不足以解释 246 秒墙钟差；
- 更大的问题是 page-cache/缺页路径自身消耗的 CPU，以及随后的回收、解除映射和 TLB 刷新。

对应火焰图：

- [ext4 锁等待](guest-ext4-lock-wait-timed.svg)
- [文件系统与块 I/O 延迟](guest-fs-io-latency-timed.svg)
- [folio 加载等待](guest-folio-load-wait-timed.svg)

## `mprotect`

本次精确记录到：

| 指标 | 数值 |
| --- | ---: |
| 调用次数 | 1,007,950 |
| 成功 | 1,007,950 |
| 失败 | 0 |
| 请求处理页数 | 5,219,565 |
| 单次最大页数 | 2,873 |
| 总估算 task-time | 53.296 秒 |
| 平均每次 | 52.876 微秒 |
| 样本最大值 | 51.026 毫秒 |
| 本地更新阶段 | 31.463 秒 |
| AddrSpace 锁等待 | **30.860 毫秒** |

请求页数分布：

| 页数 | 调用次数 |
| --- | ---: |
| 1 页 | 745,532 |
| 2–4 页 | 153,967 |
| 5–16 页 | 73,934 |
| 17–64 页 | 27,772 |
| 65–256 页 | 3,840 |
| 257–1024 页 | 2,275 |
| 1025 页以上 | 630 |

其中 992,403 次是设置读写权限。调用极多，说明 C/C++ 编译、链接与分配器确实频繁改变映射权限；不过 AddrSpace 锁等待只占 30.86 毫秒，原先的全局锁争用已不再是主要问题。

53.296 个 task-second 仍然是真实 CPU 开销，但相对于 688.436 秒 × 8 vCPU 的总容量只约占 0.97%，不能单独解释 Moss 与 Linux 的 246 秒差距。后续若优化 `mprotect`，应针对大量 1 页调用的页表更新和 TLB 合并，而不是重新设计已经很小的锁等待。

对应火焰图：

- [`mprotect` 总路径](guest-mprotect-total-timed.svg)
- [`mprotect` 本地更新](guest-mprotect-local-update-timed.svg)

## AddrSpace 写锁持有时间

| 操作 | 估算总 task-time | 记录样本 | 估算平均值 | 样本最大值 |
| --- | ---: | ---: | ---: | ---: |
| `exit clear` | **101.557 秒** | 1,380，完整记录 | 73.592 毫秒 | 893.377 毫秒 |
| `exec clear` | **65.525 秒** | 377，完整记录 | 173.806 毫秒 | 416.268 毫秒 |
| clone parent | 43.501 秒 | 28（1/16） | 97.10 毫秒 | 140.122 毫秒 |
| clone child | 79.844 秒 | 50（1/16） | 99.81 毫秒 | 149.135 毫秒 |
| `mmap` | 3.555 秒 | 5,261（1/16） | 42.24 微秒 | — |
| `munmap` | 29.267 秒 | 2,145（1/16） | 852.8 微秒 | — |
| `mprotect` 写锁持有 | 40.708 秒 | 63,002（1/16） | 40.38 微秒 | — |
| `madvise` | 6.181 秒 | 189（1/16） | 2.044 毫秒 | — |

clone parent/child 使用确定性的 per-CPU 抽样，样本数不完全相同，所以不能据此推导精确 clone 次数。它们的平均持锁时间仍足以说明：clone 会长时间遍历或复制地址空间结构。

exit、exec、clone 的 task-time 也不能直接相加后从 246 秒墙钟差中扣除，因为不同任务可能并行执行，parent/child 阶段也可能相互重叠。它们的意义在于定位优化对象：

- clone 单次约百毫秒；
- exec 清空旧地址空间平均约 174 毫秒；
- exit 清理存在接近 0.9 秒的长尾；
- CPU 火焰图同时出现 `unmap_present`、页表遍历、BTree、TLB 和 SBI 热点。

这些证据共同指向地址空间构造与销毁路径，而不是只有“拿锁等待”这一件事。

对应火焰图：

- [exit clear 写锁持有](guest-aspace-exit-clear-write-hold-timed.svg)
- [exec clear 写锁持有](guest-aspace-exec-clear-write-hold-timed.svg)
- [clone parent 写锁持有](guest-aspace-clone-parent-write-hold-timed.svg)
- [clone child 写锁持有](guest-aspace-clone-child-write-hold-timed.svg)

## off-CPU 的正确解释

工作负载任务的 off-CPU 合计为 27,476.9 个 task-second。这个数远大于墙钟时间，是因为许多进程和线程可以同时睡眠，且 Rust/C/C++ 构建天然包含父进程等待子进程、管道、futex 和事件循环等待。

最容易误判的是 build-script 主进程：

| 任务 | off-CPU | 主要原因 |
| --- | ---: | --- |
| `build-script-main` | 663.392 秒 | 其中 644.292 秒为 `clock_nanosleep` |
| `riscv64-linux-gnu-*` 驱动 | 662.214 秒 | 其中 637.824 秒为 `waitpid` |
| `cc1` | 5.529 秒 | 其中约 4.405 秒为块读取等待 |

`cc` crate 的并行执行器会轮询它启动的编译器子进程：连续数次没有进展后先 yield，再逐步 sleep，最长可到约 1 秒。因此 build-script 的 644 秒 `nanosleep` 不是“内核让主编译器睡死了”，而是协调进程在等待多个 `cc1` 子进程；编译工作实际发生在子进程中。

同理，编译驱动长时间 `waitpid` 是在等待 `cc1`，不是驱动自身的性能瓶颈。真正需要关注的是 `cc1` 和 `rustc` 运行时落入了哪些内核路径。

AddrSpace 读写锁相关 off-CPU 等待约 50.477 个 task-second，只占工作负载 off-CPU 的约 0.184%。这说明此前将缺页慢阶段移出 AddrSpace 大锁是有效的，但不代表地址空间的 CPU 处理和写锁持有成本已经消失。

对应火焰图：

- [工作负载 off-CPU](guest-offcpu-workload-timed.svg)
- [锁等待 off-CPU](guest-offcpu-lock-timed.svg)

## 为什么当前瓶颈会让“8 核跑满但仍然慢”

QEMU 的 8 个宿主线程接近满载，只能说明 guest 正在执行大量指令，不代表这些指令都属于 `rustc`、`cc1` 或 LLVM。

当前 CPU 火焰图显示，Moss 会把大量时间花在：

1. 编译器访问新的文件页或匿名页，触发缺页；
2. fault plan 查询 VMA、文件页引用和缓存元数据；
3. 分配或复制物理页；
4. 重新验证映射并安装 PTE；
5. 内存压力下由后台 worker 回收页面；
6. 进程/编译器子进程退出或 exec 时逐个解除映射；
7. 对多个小 VMA 分别完成 TLB 刷新，并通过 SBI/IPI 通知其他 vCPU。

这些工作本身可以在多个 CPU 上并行，所以不会表现为单核；但它们会抢占本应交给编译器的 CPU，并增加 QEMU TCG 要翻译和执行的 guest 内核指令数。这正是本轮“并行度正常、总时间仍比 Linux 慢”的主要解释。

## 优化优先级

### P0：合并地址空间回收与 TLB shootdown

当前 `exit`、`exec` 和 `unmap_present` 已经是明确热点。应将“清 PTE、保存待释放 frame、完成 TLB 刷新、释放 frame”的批处理范围从单个 backend/VMA 提升到整个地址空间操作：

1. 遍历多个 VMA 时只清 PTE并收集 frame；
2. 不在每个 VMA 结束时单独 `finish_unmap_batch`；
3. 整个 clear/exec/exit 完成后统一选择地址级或全地址空间 TLB 刷新；
4. TLB 已失效后再统一释放旧 frame，保证其他 CPU 不会通过旧 TLB 访问已复用物理页。

这样可以同时减少重复页表遍历、重复 SBI 调用和跨核 IPI。由于 CPU 图中 `flush_tlb` 与 SBI 分别占 4.10% 和 3.82%，这条路径有直接证据支撑。

### P1：减少每次文件缺页的元数据和复制成本

应逐项确认读/执行的 `MAP_PRIVATE` 文件页是否能够直接映射稳定的 page-cache frame，并且只在第一次写入时 COW。重点检查：

- 同一 fault 是否重复查询 BTree、`FilePageRefs` 和 page cache；
- prepare 阶段已得到的稳定引用，commit 阶段是否能用版本号验证，而不重新走完整查询；
- refcount 为 1 且权限允许时，是否能直接升级权限；
- 连续只读文件页是否能以 folio/batch 形式查找、读取和安装；
- fault 后立即发生回收，是否说明动态 page-cache/PCP 水位仍过于激进。

这一优先级由缺页 29.31%、reclaim 12.92%、BTree 6.62% 和 CachedFile 11.53% 共同支持。

### P2：clone 使用更懒的地址空间复制

clone parent/child 的抽样平均写锁持有时间都接近 100 毫秒。应避免为 fork 在两个地址空间中完整复制页级 BTree 元数据：

- 共享只读 VMA 与文件页元数据；
- 匿名/私有页只修改必要的 COW 权限；
- 允许页表页或映射描述符延迟复制；
- 将 TLB 刷新与上述地址空间级 gather 合并。

实现时必须保证父子进程修改 VMA 或页表时能够正确分离，不能仅为了减少复制而共享可变结构。

### P3：缩小 ext4 全局锁的剩余范围

ext4 mutex 等待仍有 7.32 个 task-second，且占通用 mutex 等待的 96.26%。应继续把以下工作移出全局锁：

- 数据块实际读取和 VirtIO 提交；
- 连续 extent 的数据读取；
- page-cache folio 的填充和等待；
- 不改变 ext4 元数据的只读 fast path。

全局锁只保护 extent/目录/元数据结构的必要查询或更新，并在锁外异步提交合并后的 BIO。不过它是 P3，而不是本轮唯一或最大的解释。

### P4：保留 `mprotect` 现有结构，只做批量化

`mprotect` AddrSpace 锁等待已经下降到 30.86 毫秒，不应重新引入全局记录表或扩大锁范围。若继续优化，优先：

- 连续 PTE 批量修改；
- 权限未变化快速返回；
- 多个相邻 VMA 修改后合并；
- 整个 syscall 只做一次合适粒度的 TLB flush。

## 后续验收建议

完成 P0 后，下一次仍使用同一 `aws-lc-sys` 精确窗口，至少检查：

1. 未插桩 build-script 是否低于 666.8 秒；
2. `unmap_present`、页表遍历、`flush_tlb`、SBI 的 CPU 样本比例是否一起下降；
3. exit/exec clear 的平均值和长尾是否下降；
4. CPU 仍保持约 7–8 核运行，而不是通过减少并发换取表面上的内核样本下降；
5. ext4、块 I/O 和编译器 `cc1` 时间是否没有回退；
6. 使用全新 snapshot，避免 Cargo target 缓存造成虚假提速。

若只看到火焰图比例下降而总时间没有下降，需要检查分母变化；若总时间下降但 CPU 利用率同时明显下降，则要确认是否意外减少了编译任务或破坏了并行度。

## 复现命令

```bash
make PROFILE=y kernel-rv

python3 scripts/run_guest_profile_full.py \
  --kernel kernel-rv \
  --elf kernel-rv.profile.elf \
  --disk tmp/qemu-cargo-timing/moss-timing-overlay-20260724T211300.qcow2 \
  --disk-format qcow2 \
  --out docs/profiling/riscv-awslc-build-script-20260724 \
  --ready-marker '#### MOSS AWS-LC PROFILE READY ####' \
  --poll-seconds 10 \
  --timeout-seconds 2400
```

本次运行：

- 启动时间：2026-07-24 23:57:17 +08:00；
- 采样完成：2026-07-25 00:10:38 +08:00；
- QEMU 进程总存活时间：800.798 秒；
- 精确 build-script 窗口：688.436 秒。

`profiles.bin`、控制台日志和 folded 中间文件体积较大，保留在本地但由本目录 `.gitignore` 排除；报告、SVG 和 `summary.json` 可以提交。
