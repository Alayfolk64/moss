# RISC-V BuildStorm 抢占释放快速路径优化报告

## 结论

本轮针对上一轮 CPU 火焰图中很宽的
`kernel_guard::NoPreemptIrqSave::release` 路径，移除了
`enable_preempt()` 获取当前任务时额外创建的 IRQ-save guard。

RISC-V 和 LoongArch 读取 per-CPU 当前任务指针需要多条指令，通用入口
`current_task_ptr()` 为防止读到一半迁移到其他 CPU，会临时关闭本地中断。
但是 `NoPreemptIrqSave::release` 在递减配对的抢占禁用计数前，当前任务本来
就不能迁移。因此本轮新增“调用者已保证禁止迁移”的读取入口，并只在这条
释放路径使用。

新一轮冷镜像全流程成功：

| 指标 | 上轮 unmap gather | 本轮 release fast path | 变化 |
| --- | ---: | ---: | ---: |
| profile 预构建阶段 | 1207.67 s | **1154.52 s** | **-53.15 s / -4.40%** |
| profile 正式阶段 | 599.58 s | **575.20 s** | **-24.38 s / -4.07%** |
| profile 三阶段总和 | 1811.64 s | **1734.07 s** | **-77.57 s / -4.28%** |
| BuildStorm | 531.20 s | **508.49 s** | **-22.71 s / -4.27%** |
| 宿主全流程 | 1816.15 s | **1741.26 s** | **-74.89 s / -4.12%** |

端到端结果确实变快，但 CPU 采样没有证明这 4% 全部来自本轮修改：
`NoPreemptIrqSave::release` 按阶段时间归一化后，预构建基本持平，正式阶段
只下降约 2.9%。这次只有一轮新版本测试，crate 调度、磁盘缓存和宿主负载
也会产生波动。因此准确结论是：

- 新入口通过了完整构建和负载验证，没有发现正确性回归；
- 本轮端到端观测改善约 4.1%～4.3%；
- 旧的 `CurrentTask::try_get` 符号从释放路径消失，但内联会改变采样归属；
- 不能仅凭这一轮把全部端到端改善归因于删除一次 IRQ-save guard；
- `NoPreemptIrqSave::release` 仍是下一轮需要继续拆解的最大通用内核热点。

## 修改内容

涉及文件：

```text
vendor/axhal/src/percpu.rs
vendor/axtask/src/task.rs
vendor/axtask/src/api.rs
```

### 原路径

```text
NoPreemptIrqSave::release
    -> KernelGuardIfImpl::enable_preempt
    -> current_may_uninit
    -> CurrentTask::try_get
    -> axhal::percpu::current_task_ptr
    -> IrqSave::new
    -> 读取 CURRENT_TASK_PTR
    -> 恢复中断
    -> preempt_disable_count.fetch_sub
```

释放一个同时关闭 IRQ 和抢占的自旋锁时，外层 guard 尚未递减配对的抢占
计数。只要计数仍大于零，调度器就不能把当前任务迁移到其他 CPU。此时再次
关闭 IRQ 不是保持 per-CPU 指针稳定所必需的。

### 新路径

`axhal` 新增：

```rust
pub unsafe fn current_task_ptr_preempt_disabled<T>() -> *const T
```

该函数直接读取 `CURRENT_TASK_PTR`，不创建新的中断 guard。它是 `unsafe`
接口，因为类型系统不能证明调用期间禁止迁移。

`axtask::CurrentTask` 新增：

```rust
pub(crate) unsafe fn try_get_preempt_disabled() -> Option<Self>
```

通用的裸指针转 `CurrentTask` 逻辑被提取到 `from_raw_ptr`，避免两个入口
复制引用计数约定。`CurrentTask` 仍使用 `ManuallyDrop<Arc<_>>`，因此从裸
指针构造和析构都不会改变任务引用计数。

`KernelGuardIfImpl::enable_preempt()` 只在递减计数前调用该新入口：

```text
当前计数仍然大于零
    -> 当前任务不能迁移
    -> 直接读取当前 CPU 的任务指针
    -> 递减计数
    -> 若变成零且 need_resched，再进入调度检查
```

`disable_preempt()` 保持原样。它在增加计数之前不能假定抢占已关闭，因此
仍使用通用的迁移安全入口。

## 构建前的三遍静态检查

### 第一遍：配对关系、指针生命周期和迁移

- 新入口只在配对的 `enable_preempt()` 内使用；
- 调用发生在计数递减之前，当前任务仍不能迁移；
- 裸指针只在该函数调用期间使用；
- 计数降为零后，不再通过旧裸指针跨越可能发生的调度；
- `CurrentTask` 的 `ManuallyDrop` 约定未改变。

### 第二遍：嵌套 guard、中断和调度边界

- 嵌套 guard 释放时计数仍大于零；
- 最外层释放将计数减到零后才检查 `need_resched`；
- 中断上下文不会把正在运行且抢占计数非零的任务迁移走；
- scheduler 尚未安装当前任务时，空指针仍返回 `None`；
- run queue 的上下文切换入口未改动。

### 第三遍：配置、格式、vendor 完整性

- `preempt` feature 关闭时不引入未解析符号；
- 新 `unsafe` 的前置条件写入了 API 文档；
- `cargo fmt`、`git diff --check` 和离线 metadata 检查通过；
- `vendor/axhal/.cargo-checksum.json` 与
  `vendor/axtask/.cargo-checksum.json` 已同步；
- 三遍检查完成后才执行 `make PROFILE=y kernel-rv`，构建通过。

## 完整测试口径

根文件系统重新从官方压缩镜像独立解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
tmp/profile-rv-preempt-release-fastpath-20260725/sdcard-rv.img
15032385536 bytes
sha256=c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0
```

运行条件：

- Ubuntu 宿主直接运行，不使用 Docker；
- QEMU RISC-V `virt`；
- 8 GiB、8 vCPU；
- `-snapshot`；
- 从空 target 完整执行 cagent、tg-xtask 预构建、正式 BuildStorm；
- CPU 10 Hz；
- off-CPU 1/8 抽样；
- ext4 1/16 抽样；
- page cache 1/32 抽样；
- 地址空间写锁 1/16 抽样；
- mprotect 1/32 抽样。

cagent 10/10 通过，正式 BuildStorm 成功：

```text
tg-xtask 预构建：19m13s
Finished release target(s) in 8m12s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=508.49
```

采样完整性：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=1
```

`skipped_cpu=1` 表示约 5.8 万次 CPU 样本中有一次采样因该 CPU 的 profile
状态锁正忙而跳过。它不会改变结论，但本轮不能写成“零跳样”。

## CPU 火焰图核验

### 目标路径

| 指标 | 上轮预构建 | 本轮预构建 | 上轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| 非 idle CPU 样本 | 10,286 | 9,691 | 4,509 | 4,223 |
| `NoPreemptIrqSave::release` inclusive | 1,630 | 1,576 | 640 | 596 |
| 每阶段秒的 release 样本 | 1.350 | 1.365 | 1.067 | 1.036 |

预构建单位时间增加约 1.1%，正式阶段下降约 2.9%，统计上应视为基本稳定。
旧图中释放路径内可辨认的 `CurrentTask::try_get` 在预构建和正式阶段分别约
有 696、248 个 inclusive 样本，新图中该符号为零。但是新快速入口被内联，
这些样本可能上移到 `release`，不能解释为对应 CPU 时间全部消失。

`local_irq_save_and_disable` 的全局样本从预构建 134 变为 133，正式阶段从
72 变为 45。正式阶段方向符合预期，预构建没有明显变化，仍不足以单独证明
稳定收益。

### 地址空间路径

本轮没有修改地址空间代码，但下列观测继续下降：

| 事件 | 上轮预构建 | 本轮预构建 | 上轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| `exit clear` 写锁持有 | 65.83 s | 59.35 s | 28.49 s | 25.71 s |
| `exec clear` 写锁持有 | 41.31 s | 36.63 s | 16.50 s | 12.19 s |
| clone child 写锁持有 | 88.88 s | 66.62 s | 31.67 s | 26.57 s |
| clone parent 写锁持有 | 55.09 s | 32.76 s | 14.01 s | 13.97 s |

因为代码未改，这些变化更能说明单轮工作负载存在波动，也进一步要求不要把
整轮的 4% 改善全部归因于抢占释放入口。

## 火焰图

- [完整 CPU](guest-cpu-all.svg)
- [预构建 CPU](guest-cpu-prebuild.svg)
- [正式阶段 CPU](guest-cpu-timed.svg)
- [完整 off-CPU](guest-offcpu-all.svg)
- [预构建 off-CPU](guest-offcpu-prebuild.svg)
- [正式阶段 off-CPU](guest-offcpu-timed.svg)
- [ext4 锁等待](guest-ext4-lock-wait-all.svg)
- [folio 加载等待](guest-folio-load-wait-all.svg)
- [同步文件系统 I/O](guest-fs-io-latency-all.svg)
- [exit clear 写锁](guest-aspace-exit-clear-write-hold-all.svg)
- [exec clear 写锁](guest-aspace-exec-clear-write-hold-all.svg)
- [clone child 写锁](guest-aspace-clone-child-write-hold-all.svg)
- [clone parent 写锁](guest-aspace-clone-parent-write-hold-all.svg)

## 下一轮选择

新的 CPU 图中 `NoPreemptIrqSave::release` 仍占预构建非 idle 样本约 16.3%，
正式阶段约 14.1%。读取当前任务的额外 IRQ guard 已被移除后，源码中每次
guard 获取和释放仍执行：

```text
disable: AtomicUsize::fetch_add
release: AtomicUsize::fetch_sub
```

`preempt_disable_count` 只描述当前任务在当前 CPU 上的嵌套深度；现有调用
点也只由当前任务读写。下一轮将验证能否把它改为当前任务本地的 `Cell<usize>`，
去掉 RISC-V 上每次锁获取和释放的 AMO 原子读改写指令。

这项修改比上一轮风险更高，重点要静态证明：

1. 没有其他 CPU 读取或写入该计数；
2. 同一 CPU 中断嵌套不会在 `get` 与 `set` 之间破坏计数；
3. `can_preempt(1)` 和 `can_preempt(2)` 的 run queue 约定不变；
4. 计数减到零后不再使用可能因调度而失效的当前任务借用；
5. 溢出和下溢要显式检测，不能依赖 release 模式回绕。
