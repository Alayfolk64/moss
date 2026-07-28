# RISC-V BuildStorm 优化时间线与收益归因（2026-07-21～2026-07-28）

## 先给结论

“最有用的是 folio 和 `mprotect`”这个判断大体正确，但需要把两件事说得更精确：

1. **7 月 24 日凌晨约 2.11 倍的提升不能只归功于 folio。** 那一轮同时包含两阶段缺页、稳定页引用、folio/page-cache、ext4 和块设备等一组修改，只能把它归因于整条文件缺页与 I/O 链路。
2. **7 月 24 日下午的 per-VMA `mprotect` 是证据最强、幅度最大的单项优化。** BuildStorm 从详细测量基线的 1243.89 秒降到 567.41 秒，下降 54.38%；`mprotect` 累计任务时间下降约 99.3%，而调用次数没有减少，局部证据与端点结果相互吻合。
3. **7 月 25 日的 folio 批量提交是 `mprotect` 之后证据最强的单项优化。** 在加入地址空间回收和远程 SBI RFENCE 的正确性基线后，BuildStorm 从 582.76 秒降到 539.38 秒，下降 7.44%。
4. **`mprotect` 之后并非“所有修改都没有变化”。** 从 567.41 秒到目前最后一次测得的 504.26 秒，表面上又下降约 11.1%；若以功能更完整的 582.76 秒正确性基线计算，则约下降 13.5%。但这些数据来自多轮单次运行，期间还混有正确性修改和 I/O 波动，不能把每轮小百分比机械相加成确定的因果收益。
5. **进入约 508 秒平台后，多数端点变化只有 ±1%。** 这部分工作的主要价值经常是消除局部热点、缩短关中断区、修复远程 TLB 一致性或改善资源回收，而不是可稳定复现的大幅墙钟提升。

因此，按“对 BuildStorm 墙钟的确定贡献”排序：

1. 7 月 24 日凌晨的整组文件缺页/folio/page-cache/I/O 链路修改；
2. 7 月 24 日下午的 per-VMA `mprotect`；
3. 7 月 25 日的 folio 批量提交；
4. unmap gather、抢占快速路径和后续 allocator/COW 微优化。

如果只比较**能较清楚单独归因的修改**，则 `mprotect` 第一，folio 批量提交第二。

## 口径说明

- `BuildStorm` 表中的时间优先取各报告记录的 `BUILDSTORM_COMPILE elapsed_s`，不是整次宿主运行时间。
- 每轮通常只完成一次全流程运行。小于约 1% 的差异应视为观察值，不是稳定置信区间。
- 火焰图中某个目标调用栈消失，可以证明局部路径被优化；它不能自动证明端到端时间会同比例下降。
- 7 月 24 日下午之后加入了远程 SBI RFENCE 等正确性修改，所以 567.41 秒可作性能参照，但不是后续轮次完全等价的正确性基线。
- 7 月 28 日的缺页复现是正确性追踪，不是新一轮 BuildStorm 性能测量。

## 主时间线

| 时间 | 版本/轮次 | BuildStorm | 相对上一可比轮次 | 可以下的结论 |
| --- | --- | ---: | ---: | --- |
| 07-21 | [最初完整基线](./riscv-buildstorm-guest-20260721/optimization-report.md) | Cargo 自报约 2485 s | — | 文件缺页大锁、page-cache 和 I/O 链路是主要瓶颈；冷预构建约 76:28，正式阶段约 42:05。 |
| 07-24 04:59 | `ea78c094`，[整组缺页优化复测](./riscv-buildstorm-guest-20260724/optimization-report.md) | 1178.57 s | 相对 2485 s：-52.57% | 两阶段缺页、稳定引用、folio/page-cache、ext4/块设备等整组修改有效；不能只归因于 folio。 |
| 07-24 上午 | [`mprotect` 详细测量](./riscv-buildstorm-mprotect-20260724/mprotect-report.md) | 1243.89 s | 相对 1178.57 s：+5.54% | 这是补充探针后的定位基线，不是修复；确认全表扫描和记录锁是决定性热点。 |
| 07-24 15:45 | `97196f41`，[per-VMA `mprotect`](./riscv-buildstorm-mprotect-per-vma-20260724/mprotect-per-vma-report.md) | **567.41 s** | 相对 1243.89 s：**-54.38%** | 最大、最清晰的单项优化；这就是 `mprotect-fix-20260724` 分支的落点。 |
| 07-24 晚间 | [驻留页回收/远程 RFENCE](./riscv-buildstorm-resident-reclaim-20260724/resident-reclaim-report.md) | 582.76 s | 相对 567.41 s：+2.71% | 增加地址空间回收和远程 SBI RFENCE 正确性；不能按纯性能回退理解，后续应以它作为更完整基线。 |
| 07-25 | [folio 批量提交](./riscv-buildstorm-folio-batch-20260725/folio-batch-report.md) | **539.38 s** | 相对 582.76 s：**-7.44%** | `mprotect` 之后最强的单项端点收益，同时保留需求页优先和竞争语义。 |
| 07-25 | [直接 frame 引用](./riscv-buildstorm-frame-ref-direct-20260725/frame-ref-direct-report.md) | 544.64 s | +0.98% | 目标 CPU/锁路径明显下降，但端点没有变快。 |
| 07-25 | [frame-table 锁缩短关中断](./riscv-buildstorm-frame-lock-preempt-20260725/frame-lock-preempt-report.md) | 542.03 s | -0.48% | 局部改动成立；539～545 秒范围只支持端点基本持平。 |
| 07-25 | [frame 引用直接索引](./riscv-buildstorm-frame-ref-index-symbol-safe-20260725/frame-ref-index-report.md) | 543.10 s | +0.20% | 局部热点下降 81%～86%，端点仍约 543 秒。 |
| 07-25 | [跨 VMA unmap gather](./riscv-buildstorm-unmap-gather-20260725/unmap-gather-report.md) | 531.20 s | -2.19% | 合并回收与 RFENCE，reclaim RFENCE 下降约 98%；单轮端点也改善，属于可信的结构性修改。 |
| 07-25 | [抢占释放快速路径](./riscv-buildstorm-preempt-release-fastpath-20260725/preempt-release-fastpath-report.md) | 508.49 s | -4.27% | 目标路径变短且端点改善；CPU 归因弱于 `mprotect`/folio，不能把 22.71 秒全部认作稳定收益。 |
| 07-25 | PTE CAS、acquire/current、guard、PTE inline、水位统计等轮次 | 约 508～518 s | 多数在 ±1% | 主要是局部热点和临界区优化；BuildStorm 已进入平台区。 |
| 07-25 | [PCP fast/slow path](./riscv-buildstorm-pcp-fastpath-20260725/pcp-fastpath-report.md) | 518.11 s | 小幅回退 | 目标叶样本下降 94%～95%，但端点变慢；局部优化成立，墙钟收益不成立。 |
| 07-25 | [COW HashMap](./riscv-buildstorm-cow-hash-20260725/cow-hash-report.md) | 514.67 s | 端点无可靠收益 | 新增分配、扩容和析构成本，报告结论是回退该方案。 |
| 07-25 | [COW 内联槽](./riscv-buildstorm-cow-inline-20260725/cow-inline-report.md) | 515.89 s | 近似持平 | 撤销 HashMap，匿名映射避免创建文件页所有权表；局部证据支持保留。 |
| 07-25 | [bitmap next-fit](./riscv-buildstorm-bitmap-nextfit-20260725/bitmap-nextfit-report.md) | 511.76 s | -0.80% | 高阶 folio 扫描热点被消除；4.13 秒不能全部视为稳定收益。 |
| 07-25 | [原子 usage 快照](./riscv-buildstorm-atomic-usages-20260725/atomic-usages-report.md) | 509.12 s | -0.52% | 目标调用栈消失，局部成立；端点差异可能仍是单轮波动。 |
| 07-25 | [Zicboz 清零](./riscv-buildstorm-zicboz-zero-20260725/zicboz-zero-report.md) | 507.87 s | -0.25% | COW 新页旧 `memset` 路径消失，局部因果证据支持保留。 |
| 07-25 | [PCP IRQ 快速路径](./riscv-buildstorm-pcp-irqfast-20260725/pcp-irqfast-report.md) | 506.28 s | -0.31% | BuildStorm 快 1.59 秒，但 prebuild 慢约 6 秒、宿主全流程基本相同；属于混合结果。 |
| 07-25 最后一轮 | [用户返回快速路径数据](./riscv-buildstorm-user-return-fast-20260725/summary.json) | **504.26 s** | -0.40% | 当前最低单次 BuildStorm；prebuild 比上一轮慢约 6.2 秒，timed phase 快约 3.4 秒，宿主全流程几乎相同，整体应判为性能中性。 |
| 07-28 | [缺页中断完整讲解](../current-page-fault-walkthrough.md)与复现追踪 | 未重跑 BuildStorm | — | 验证私有匿名页写缺页可沿 prepare/execute/commit 链路处理，返回同一用户指令后正常退出；这是正确性证据。 |

## 分阶段看发生了什么

### 1. 7 月 21 日：找到真正的上限

初始测量显示，问题不是某一条普通指令慢，而是高并发冷构建触发了文件缺页、folio 加载、page-cache 失效和文件系统锁等待的放大效应。Cargo 自报正式时间约 2485 秒，预构建约 76 分钟；这给后续工作确立了“先拆缺页大锁和文件 I/O 链路”的方向。

### 2. 7 月 24 日凌晨：整条文件缺页链路第一次跃迁

提交 `ea78c094fbd5767a9b1578994feac4a3a75d3c6f` 的标题是“优化缺页并发并完善全流程性能分析”。BuildStorm 降到 1178.57 秒，约为旧值的 47.43%。

这一提交包含的是组合改动，报告也没有把全部收益归给单个补丁。准确说法应当是：

- folio/page-cache 是核心部分；
- 两阶段缺页把准备工作移出关键写锁；
- 稳定页引用和 invalidate/加载并发规则解决了锁环与重复工作；
- ext4 和块设备路径的调整减少了 I/O 链路放大。

所以“folio 带来了第一次大提升”可以作为概括，但不能写成“folio 单项提升了 52.57%”。

### 3. 7 月 24 日下午：`mprotect` 是最清楚的单项修复

详细探针先测得 1243.89 秒，并把成本拆成全表扫描、seal 记录锁、权限更新和地址空间锁等待。随后提交 `97196f4195e6e25a3d0f9846e9ae807f4f71cb1e` 将处理改为按受影响 VMA 工作。

结果同时满足三项强证据：

- 调用次数没有靠“少做 syscall”下降；
- `mprotect` 累计任务时间下降约 99.27%～99.34%；
- BuildStorm 端点下降 54.38%。

这比只看一次墙钟更可靠，因此它是整个时间线里最确定的单项优化，也是单独切分支保存的合理节点。

### 4. 7 月 24 日晚至 7 月 25 日：先补正确性，再挤压局部热点

驻留页回收轮次补上了远程 SBI RFENCE。它比 567.41 秒慢 2.71%，但旧轮次不具备完全等价的远程 TLB 一致性，不能为了追求数字回退正确性。

紧接着 folio 批量提交把正确性基线的 582.76 秒降到 539.38 秒。这一轮既有清楚的实现边界，也有 7.44% 的端点变化，是 `mprotect` 之后最值得单独强调的一轮。

此后 frame 引用、frame-table 锁和直接索引连续把局部热点削掉，但三轮端点都在约 543 秒。这说明优化对象真实存在，同时也说明它们已经不再控制整个关键路径。

unmap gather 将跨 VMA 的释放和 RFENCE 合并，BuildStorm 到 531.20 秒；抢占释放快速路径又测到 508.49 秒。两项都值得保留，但由于只有单轮数据且其它 CPU/I/O 长尾同时变化，收益置信度低于前两个大节点。

### 5. 7 月 25 日后半段：进入约 508 秒平台

PTE CAS、抢占获取/current 读取、guard 内联、PTE 内联、水位总页数、PCP、COW、bitmap 和清零路径等修改继续改变热点分布。此时应把结果分成两层：

- **局部成立：** 目标调用栈或关中断热点显著下降，语义审查和完整构建通过；
- **全局未证实：** BuildStorm 墙钟只变化零点几个百分点，或一个阶段变快而另一个阶段变慢。

最后一次 `user-return-fast` 数据为 504.26 秒，但与上一轮相比：

- prebuild：1124.22 秒 → 1130.44 秒，慢约 6.2 秒；
- timed phase：572.93 秒 → 569.53 秒，快约 3.4 秒；
- 宿主全流程：1711.306 秒 → 1711.301 秒，基本完全相同。

因此，不能因为 504.26 是最低单次值，就把用户返回快速路径认定为新的大幅优化。

## 明确失败或应回退的尝试

| 尝试 | 证据 | 处理 |
| --- | --- | --- |
| [COW HashMap](./riscv-buildstorm-cow-hash-20260725/cow-hash-report.md) | BTree 查找下降，但引入新的表分配、扩容和析构成本 | 回退 HashMap，后续改为内联槽加按需 BTree overflow |
| [`memset` 字宽改写](./riscv-buildstorm-memset-word-20260725/memset-word-failure-report.md) | 预构建出现文件系统内容异常，正式 BuildStorm `rc=101` | 数据不能用于性能比较，修改回退 |
| [抢占 state token](./riscv-buildstorm-preempt-state-token-20260725/preempt-state-token-report.md) | BuildStorm +0.13%，总流程近零；成本从 release 转移到 acquire | 性能中性且增加状态搬运，不值得保留 |
| [pending-self](./riscv-buildstorm-preempt-pending-self-20260725/pending-self-failure-report.md) | 预构建 8/446 时页表销毁路径 panic，没有完整火焰图 | 不保留语义修改，回到已完整通过的基线 |

失败记录仍然有价值：它们明确了哪些方向已经试过、为何不能把不完整运行当成性能结果，也避免以后重复踩同一条路径。

## 提交与工作区边界

关键 Git 节点如下：

| 提交 | 时间 | 含义 |
| --- | --- | --- |
| `ea78c094fbd5767a9b1578994feac4a3a75d3c6f` | 07-24 04:59 | 整组缺页并发与文件 I/O 链路优化 |
| `97196f4195e6e25a3d0f9846e9ae807f4f71cb1e` | 07-24 15:45 | per-VMA `mprotect` 修复；`mprotect-fix-20260724` 指向这里 |
| `7ac73c46c64b3ede08577d1050c28c694229f611` | 07-25 13:54 | 后续缺页与地址空间并发修改 |
| `3ab5d401322bc12a97d987a8498a84f3d5dec33c` | 07-25 14:45 | 页缓存水位重复统计及当时已有性能材料 |
| `865518e9e4596af3371a329fc73e0a6f5f57702a` | 07-28 17:02 | 提交此前未提交的后续优化、profiling 归档和缺页复现材料 |

也就是说，7 月 24 日下午的修复确实有独立提交。7 月 25 日的部分工作已有提交，而更多后续结果目录和 7 月 28 日缺页追踪在 `865518e9` 之前一直留在工作区，不能笼统称为“7 月 24 日之后全部都没提交”。

当前最后一份性能端点来自 `user-return-fast` 的 504.26 秒；`865518e9` 还包含之后的缺页正确性追踪修改，但没有对应的新一轮 BuildStorm 全流程重测。因此后续比较性能时，应先从明确版本重新跑一次完整基线，不能直接把 504.26 秒标到所有当前源码上。

## 最终判断

- 说“`mprotect` 修复后已经没有数量级变化”是对的。
- 说“`mprotect` 修复后所有修改都没效果”是不对的；folio 批量提交有 7.44% 的清楚收益，unmap gather 和若干局部路径也有直接证据。
- 说“最有用的是 folio 和 `mprotect`”基本正确；其中 7 月 24 日凌晨的大收益必须表述为**以 folio/page-cache 为核心的整条文件缺页与 I/O 链路组合优化**。
- 若以后要继续追墙钟，应把 504～508 秒视作当前平台，使用同一提交做至少多轮 A/B，而不是继续根据单次 ±1% 结果排序。
