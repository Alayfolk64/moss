# RISC-V BuildStorm 缺页提交 CAS 去重报告

## 结论

本轮依据上一轮 CPU 火焰图中较宽的
`handle_page_fault_concurrent -> Backend::commit_fault` 路径，消除了
“原先不存在的 4 KiB PTE”在 commit 阶段的重复页表查询。

修改通过了三遍构建前静态检查、内核构建、全新官方镜像冷启动、cagent
10/10 和完整 BuildStorm。112 张同口径 SVG 已生成，采样没有丢失：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

可直接验证的局部结果是：按阶段秒数归一化后，
`PageTable64::query` 的 CPU 样本相对 release-fastpath 基准在预构建下降
11.21%，在正式阶段下降 10.43%。这与“少走一次完整页表”的修改方向一致。

端到端结果并不支持把它称为稳定的大幅提速：

| 指标 | release-fastpath 基准 | 本轮 | 变化 |
| --- | ---: | ---: | ---: |
| setup | 4.35 s | 4.23 s | -0.11 s |
| tg-xtask 预构建阶段 | 1154.52 s | **1137.81 s** | **-16.71 s / -1.45%** |
| 正式 profile 阶段 | 575.20 s | 581.99 s | +6.79 s / +1.18% |
| 三阶段合计 | 1734.07 s | **1724.03 s** | **-10.04 s / -0.58%** |
| BuildStorm 自报 | 508.49 s | 511.92 s | +3.43 s / +0.67% |
| 宿主全流程 | 1741.26 s | 1741.24 s | -0.02 s |

预构建改善、正式阶段变慢、宿主总时间不变，说明本轮局部页表遍历确实减少，
但收益仍小于大型 crate 调度、BuildStorm 尾部和完成后 profile 导出等待的
单轮波动。代码保留的理由是它删除了可证明冗余的页表遍历，且竞争语义更清楚；
不能据此声称获得了 1.45% 的稳定整体收益。

## 为什么原路径有重复工作

普通 missing fault 原路径为：

```text
prepare:
    pt.query(vaddr)                 # 第一次页表遍历，确认缺页

execute:
    锁外分配 frame 或读取 folio

commit:
    pt.query(vaddr)                 # 第二次页表遍历，再确认缺页
    pt.map_4k_concurrent(vaddr, …)  # 第三次页表遍历，叶 PTE CAS
```

私有文件 folio fault-around 在成功安装后还会执行一次 `pt.query(vaddr)`，
确认故障页是否满足权限。因此无竞争的正常路径最多会为故障页走四次页表。

commit 不能相信 prepare 时看到的状态，因为两阶段之间没有持有 AddrSpace
锁，另一个 CPU 可能已安装同一个 PTE；但是 `map_4k_concurrent` 最终执行：

```rust
slot.compare_exchange(0, entry, Ordering::AcqRel, Ordering::Acquire)
```

这个 CAS 本身就是 commit 时的原子“仍为空”验证。CAS 成功表示本 CPU 完成
安装；CAS 失败表示状态已经变化，返回 `FaultCommit::Stale`，由既有的有限
重试重新 prepare 即可。先 `query` 再 CAS 既不能消除竞争，也不能提供额外
正确性。

## 修改内容

### Cow missing fault

新增 `CowBackend::commit_missing_fault`，仅处理 `plan.source.is_none()`：

- 新匿名页直接执行 `map_4k_concurrent`；
- 私有文件 folio 在 invalidate 读锁内直接安装 demanded page 和可选 sibling；
- demanded page 的 CAS 成功直接返回 `Handled`，不再做安装后的 `query`；
- `AlreadyMapped` 返回 `Stale`；
- demanded page 的真实页表错误立即停止可选 sibling 安装并向上传递。

已有 PTE 的 COW 写故障仍走原路径：

```text
query 当前 PTE
验证 source frame / folio
条件 CAS 替换或权限升级
```

因此本轮没有放宽 COW 正确性条件。

### File missing fault

`expected_paddr == None` 表示 prepare 看到 PTE 不存在。此时在
`with_valid_page_ref` 保持 truncate/invalidate 排斥关系后直接 CAS 安装。
有 `expected_paddr` 的已映射写故障继续先查询、验证物理页和权限，再条件
remap。

### Shared missing fault

`expected_paddr == None` 时，在重新验证 `SharedPages` 身份和页索引后直接
CAS 安装；已有 PTE 的权限变更路径保持原查询和条件 remap。

## 三遍静态检查

### 第一遍：所有权和错误回收

- Cow 匿名 frame 只在 CAS 成功后从 `CowFaultPlan` 转移给 PTE；
- CAS 失败时 frame 仍由 plan 的 `Drop` 释放；
- folio page 只在 PTE 成功安装后将克隆引用插入映射引用表；
- 失败、越界和 invalidate 失效时局部 page 引用自然析构；
- File 成功安装后清空 `plan.page`，映射表保留唯一对应 pin；
- Shared 物理页继续由 `SharedPages` 持有，不新增裸所有权。

### 第二遍：并发、锁和 TLB

- commit 仍持有 AddrSpace 读锁，VMA 的 unmap、mprotect 和 fork 写侧不能
  同时改变区域身份；
- 后端 identity、权限和 shared page 身份均在 commit 前重新验证；
- 中间页表和叶 PTE 使用 Acquire/AcqRel CAS，不同页可并行、同一页只有一个
  安装者；
- `AlreadyMapped -> Stale -> 有限重试` 保留竞争收敛；
- 文件映射仍在 invalidate 读锁内发布，truncate 锁序没有变化；
- 成功处理故障后原调用点仍对 faulting address 执行本地 TLB 刷新；
- sibling 的旧负缓存会在对应 CPU 再次 fault 后走 `Handled` 并本地刷新。

### 第三遍：配置和仓库完整性

- `cargo fmt --all --check` 通过；
- `git diff --check` 通过；
- `cargo metadata --offline --no-deps` 通过；
- 所有已修改 vendor crate 的 `.cargo-checksum.json` 与实际文件一致；
- 已证伪的 preempt token guard 状态恢复为 release-fastpath 版本；
- 三遍检查结束后才运行 `make PROFILE=y kernel-rv`，12.65 秒构建通过。

## 完整测试口径

镜像从以下官方压缩包重新独立解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
tmp/profile-rv-pte-commit-cas-20260725/sdcard-rv.img
15032385536 bytes
sha256=c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0
```

运行条件：

- Ubuntu 宿主直接运行，不套 Docker；
- QEMU RISC-V `virt`；
- 8 GiB、8 vCPU；
- `-snapshot`；
- 空 target 冷启动；
- CPU 10 Hz；
- off-CPU 1/8 抽样并按采样率还原；
- ext4 1/16；
- page cache 1/32；
- AddrSpace 写锁持有 1/16；
- mprotect 1/32。

结果：

```text
cagent: 10/10 pass
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=511.92
Finished release target(s) in 8m 15s
profile host elapsed: 1741.242 s
```

`BUILDSTORM_MINIBUILD fail` 是此前各轮相同镜像中已有的独立检查结果；正式
BuildStorm 完整成功，本轮没有修改该测试脚本。

## CPU 采样对比

### 页表查询

| 阶段 | 基准 query 样本 | 本轮 query 样本 | 基准 样本/秒 | 本轮 样本/秒 | 变化 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 预构建 | 72 | 63 | 0.06236 | 0.05537 | **-11.21%** |
| 正式 | 32 | 29 | 0.05563 | 0.04983 | **-10.43%** |

10 Hz 采样对很短的页表函数分辨率有限，所以不能把样本差直接换算成秒数。
但两个阶段方向一致，且修改正好删除 query，足以证明局部工作量下降。

### 整体缺页路径

| 阶段 | 非 idle 样本 | `handle_page_fault_concurrent` inclusive | 占非 idle |
| --- | ---: | ---: | ---: |
| 预构建 | 9607 | 2572 | 26.77% |
| 正式 | 4285 | 1117 | 26.07% |

缺页整体仍约占非 idle CPU 的四分之一。新 helper
`commit_missing_fault` 分别占预构建 523、正式 233 个 inclusive 样本；
这些样本主要包含 folio 验证、多个 sibling PTE CAS、映射引用表插入和锁
guard 成本，不能理解成“重复 query 仍然存在”。

## 本轮之后最大的热点

当前非 idle CPU 的最大明确叶热点仍是通用 guard：

| 热点 | 预构建 | 正式 |
| --- | ---: | ---: |
| `NoPreemptIrqSave::release` | 1492 / 15.53% | 594 / 13.86% |
| `CurrentTask::from_raw_ptr` | 796 / 8.29% | 329 / 7.68% |
| `KernelGuardIfImpl::disable_preempt` | 538 / 5.60% | 251 / 5.86% |

上一轮已经证明保存 task token 只会把开销从 release 搬到 acquire，不能采用。
新的调用栈显示，`disable_preempt` 大量发生在
`NoPreemptIrqSave::acquire` 内：该 guard 随后本来就要关闭本地 IRQ，但当前
顺序先用通用 `current_task_ptr()` 读取当前任务，RISC-V 为保证读取期间不迁移
又临时创建一次嵌套 `IrqSave`，然后外层 guard 再关闭一次 IRQ。

下一轮应验证一个更窄的方案：

1. `NoPreemptIrqSave::acquire` 先保存并关闭 IRQ；
2. 在 IRQ 已关闭的前提下直接读取 per-CPU current task；
3. 增加 preempt 计数；
4. release 顺序仍保持“先恢复 IRQ，再递减 preempt 计数”。

这不会改变普通 `NoPreempt` 的路径，也不保存可移动的裸 task token。风险点是
进入顺序变化，必须静态证明 IRQ 关闭到 preempt 计数增加之间不会发生同步
调度，并检查 scheduler 初始化阶段的空 current-task 情况。

## 火焰图

- [完整 CPU](guest-cpu-all.svg)
- [预构建 CPU](guest-cpu-prebuild.svg)
- [正式 CPU](guest-cpu-timed.svg)
- [完整 off-CPU](guest-offcpu-all.svg)
- [锁等待 off-CPU](guest-offcpu-lock-all.svg)
- [工作负载等待 off-CPU](guest-offcpu-workload-all.svg)
- [ext4 锁等待](guest-ext4-lock-wait-all.svg)
- [folio 加载等待](guest-folio-load-wait-all.svg)
- [同步文件系统 I/O](guest-fs-io-latency-all.svg)
- [缺页 fallback 写锁](guest-aspace-fault-fallback-write-hold-all.svg)
- [exit clear 写锁](guest-aspace-exit-clear-write-hold-all.svg)
- [exec clear 写锁](guest-aspace-exec-clear-write-hold-all.svg)
- [clone child 写锁](guest-aspace-clone-child-write-hold-all.svg)
- [clone parent 写锁](guest-aspace-clone-parent-write-hold-all.svg)
