# RISC-V `NoPreemptIrqSave` 获取快路径报告

## 结论

本轮针对上一轮 CPU 火焰图中的
`NoPreemptIrqSave::acquire -> disable_preempt -> current_task_ptr ->
IrqSave` 重复关中断路径进行了修改：

1. `NoPreemptIrqSave::acquire` 先保存并关闭本地中断；
2. 在中断已关闭、任务不可能迁移的条件下直接读取 per-CPU 当前任务；
3. 增加当前任务的 preempt 计数；
4. release 仍保持“恢复原中断状态，再递减 preempt 计数”。

三遍构建前静态审查、profiling 内核构建、全新官方镜像、cagent 10/10、
完整预构建和正式 BuildStorm 均通过。共生成 112 张同口径 SVG，采样没有
丢失：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

局部目标已经达到。与紧邻的 PTE CAS 基准相比，普通
`KernelGuardIfImpl::disable_preempt` 的非 idle CPU 样本为：

| 阶段 | 修改前 | 本轮 | 按阶段秒数归一化后的变化 |
| --- | ---: | ---: | ---: |
| 预构建 | 538 | **198** | **-63.40%** |
| 正式 | 251 | **64** | **-73.33%** |

这部分剩余样本来自仍需独立工作的普通 `NoPreempt` guard；新加的
`disable_preempt_irq_disabled` 已被编译器内联，不再出现一条重复
`IrqSave` 调用链。

端到端结果没有证明正式编译变快：

| 指标 | PTE CAS 基准 | 本轮 | 变化 |
| --- | ---: | ---: | ---: |
| setup | 4.23 s | 4.29 s | +0.05 s |
| tg-xtask 预构建 phase | 1137.81 s | 1144.14 s | +6.33 s / +0.56% |
| Cargo 预构建自报 | 18m 56s | 19m 03s | +7 s |
| 正式 profile phase | 581.99 s | 556.49 s | -25.50 s / -4.38% |
| BuildStorm 自报 | 511.92 s | 512.13 s | +0.21 s / +0.04% |
| Cargo 正式构建自报 | 8m 15s | 8m 16s | +1 s |
| 三个 profile phase 合计 | 1724.03 s | 1704.91 s | -19.12 s / -1.11% |
| 宿主全流程 | 1741.24 s | 1711.17 s | -30.08 s / -1.73% |

`BuildStorm 自报`最接近实际正式编译工作，变化只有 0.04%。较短的 timed
phase 和宿主全流程还包含阶段切换、测试收尾及 profile 导出等待，不能把其
25～30 秒变化全部归因于本轮修改。因此准确结论是：

- 重复的 acquire 关中断路径已经消除；
- BuildStorm 正式编译耗时在本轮测量精度内不变；
- 这一修改可以保留，因为它删除的是可证明冗余的 guard 嵌套，但不能宣称
  已带来端到端提速。

## 原路径为什么重复

修改前，RISC-V 上的获取顺序是：

```text
NoPreemptIrqSave::acquire
    disable_preempt
        current_task_ptr
            IrqSave::acquire
                local_irq_save_and_disable
            读取 CURRENT_TASK_PTR
            IrqSave::release
                local_irq_restore
        preempt_disable_count += 1
    local_irq_save_and_disable
```

`current_task_ptr` 必须防止“先取到本 CPU 的 per-CPU 基址，随后任务迁移，
再从旧 CPU 读取指针”。但外层 `NoPreemptIrqSave` 本来马上就要关闭中断，
所以这里形成一次短暂的嵌套关中断、恢复中断，再由外层重新关中断。

修改后的顺序为：

```text
NoPreemptIrqSave::acquire
    保存并关闭本地中断
    直接读取 CURRENT_TASK_PTR
    preempt_disable_count += 1
```

中断关闭后，这段直线路径没有阻塞、缺页、显式 yield 或调度点，因此读取
per-CPU 指针期间不会迁移。嵌套 guard 也保持正确：内层保存到的原状态为
“中断已关闭”，内层 release 不会错误开启中断。

## 修改内容

### `kernel_guard`

- 为 `KernelGuardIf` 增加 `disable_preempt_irq_disabled`；
- `NoPreemptIrqSave::acquire` 改成先关中断，再调用窄接口；
- 普通 `NoPreempt` 继续使用原来的 `disable_preempt`；
- release 顺序没有改变。

### `axhal`

增加 `current_task_ptr_migration_disabled`。它只允许在调用者已经用
preempt 计数或本地关中断阻止迁移时使用，不自行创建第二个 guard。

### `axtask`

- 增加 `CurrentTask::try_get_migration_disabled`；
- 新 acquire 接口在中断已关闭时使用该 helper；
- release 使用既有 preempt 保护下的同类 helper；
- 空 current-task 的启动阶段仍返回 `None`，没有强制解引用空指针。

本轮相关源码和 checksum 合计为 64 行增加、13 行删除。

## 三遍静态审查

### 第一遍：获取和释放时序

- 关中断到增加 preempt 计数之间只有 per-CPU 指针读取和原子加一；
- 该区间不存在锁等待、内存分配、缺页、yield 或调度；
- release 恢复中断时 preempt 计数仍大于零，即使中断立即到达也不能迁移
  当前任务；
- 随后原子减一，只有最外层 guard 才检查 pending reschedule；
- 嵌套 guard 的中断状态和 preempt 计数逐层配对。

### 第二遍：接口、架构和生命周期

- 仓库中 `KernelGuardIf` 只有 `axtask` 一个真实实现，新增方法没有漏实现；
- RISC-V、LoongArch、AArch64、x86 都先通过各自架构指令原子关闭本地中断；
- current-task 尚未初始化时保持 `None` 路径；
- `CurrentTask` 的 `ManuallyDrop<Arc>` 仍只借用 per-CPU current 引用，不改变
  强引用计数；
- 调度器上下文切换仍要求 IRQ 已关闭，边界没有被放宽。

### 第三遍：机械检查

- `cargo fmt --all --check` 通过；
- `git diff --check` 通过；
- `cargo metadata --offline --no-deps` 通过；
- 10 个已修改 vendor 源文件的 SHA-256 与各自
  `.cargo-checksum.json` 全部一致；
- 三遍检查完成后才清理 `kernel_guard` RISC-V release 缓存并构建，完整依赖
  链重新展开，构建通过。

## 完整测试口径

镜像从官方压缩包重新独立解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
tmp/profile-rv-preempt-acquire-fastpath-20260725/sdcard-rv.img
15032385536 bytes
sha256=c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0
```

运行条件：

- Ubuntu 宿主直接运行，不套 Docker；
- QEMU RISC-V `virt`；
- 8 GiB、8 vCPU；
- `-snapshot`；
- 全新空 target；
- CPU 10 Hz；
- off-CPU 1/8 抽样并按采样率还原；
- ext4 1/16；
- page cache 1/32；
- AddrSpace 写锁持有 1/16；
- mprotect 1/32。

结果：

```text
cagent: 10/10 pass
tg-xtask prebuild: 19m 03s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=512.13
Finished release target(s) in 8m 16s
profile host elapsed: 1711.165 s
```

`BUILDSTORM_MINIBUILD fail` 与此前各轮相同，是单独的 minibuild 检查结果；
正式 BuildStorm 完整成功，本轮没有修改该脚本。

## CPU 火焰图变化

### acquire 目标

| 非 idle 叶/路径 | 基准预构建 | 本轮预构建 | 基准正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| `disable_preempt` | 538 | **198** | 251 | **64** |
| `local_irq_save_and_disable` | 112 | **83** | 45 | 41 |
| `CurrentTask::from_raw_ptr` | 796 | 796 | 329 | **276** |

按阶段时间归一化，`local_irq_save_and_disable` 在预构建下降 26.31%，正式
下降 4.71%。正式阶段样本只有几十个，不能将其 4.71% 当成稳定收益。

`CurrentTask::from_raw_ptr` 没有像 `disable_preempt` 一样消失，是因为 acquire
和 release 仍会把 per-CPU 裸指针临时包装成 `CurrentTask`，以调用任务上的
preempt 计数方法。本轮只删除了重复 IRQ guard，没有删除该包装。

### 当前最大的叶热点

| 热点 | 预构建 | 正式 |
| --- | ---: | ---: |
| `NoPreemptIrqSave::release` | 1727 / **18.13%** | 656 / **16.08%** |
| `CurrentTask::from_raw_ptr` | 796 / 8.36% | 276 / 6.76% |
| `UserContext::run` | 561 / 5.89% | 186 / 4.56% |
| ext4 `memset` | 536 / 5.63% | 234 / 5.74% |

release 的 86.28% 预构建样本、77.74% 正式样本来自 spin-lock guard drop。
它仍然很宽，而且相对基准没有下降。此前“把 task token 保存进每一个 guard”
的实验已经证明会把成本搬到 acquire、扩大 guard 状态并增加裸指针风险，
不能再次采用。

下一轮更窄的方向是：保留“每次 release 重新读取当前任务”，但不要构造
`ManuallyDrop<Arc>` 包装。新增封装好的
`disable/enable_current_preempt_migration_disabled` helper，在迁移已被禁止
时读取裸指针并立即调用 `AxTask` 上的计数方法。这样：

- 不在 guard 中保存跨调度点的 task token；
- 不改变 guard 大小；
- 不改变 preempt 计数仍然属于任务这一语义；
- 只删除 `CurrentTask::from_raw_ptr` 和临时包装；
- 主要风险集中在裸引用的有效期，必须将其封装在 helper 内，不能向调用者
  返回带任意生命周期的引用。

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
- [mprotect 地址空间锁等待](guest-mprotect-aspace-lock-wait-all.svg)
- [exit clear 写锁](guest-aspace-exit-clear-write-hold-all.svg)
- [exec clear 写锁](guest-aspace-exec-clear-write-hold-all.svg)
- [clone child 写锁](guest-aspace-clone-child-write-hold-all.svg)
- [clone parent 写锁](guest-aspace-clone-parent-write-hold-all.svg)
