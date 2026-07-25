# RISC-V BuildStorm 任务本地抢占计数优化报告

## 结论

本轮将 `TaskInner::preempt_disable_count` 从 `AtomicUsize` 改为任务本地
`Cell<usize>`，目标是去掉每次 spin guard 获取和释放时的 RISC-V AMO
读改写指令。

完整冷镜像测试通过，但结果不能证明这项修改带来了稳定性能收益：

| 指标 | 上轮 release fast path | 本轮 local count | 变化 |
| --- | ---: | ---: | ---: |
| tg-xtask 预构建输出 | 19m13s | **19m07s** | -6s |
| profile 预构建阶段 | 1154.52 s | **1148.04 s** | -6.48 s / -0.56% |
| Cargo release 输出 | 8m12s | 8m14s | +2s |
| BuildStorm | **508.49 s** | 510.42 s | +1.93 s / +0.38% |
| profile 正式阶段 | **575.20 s** | 577.39 s | +2.19 s / +0.38% |
| profile 三阶段总和 | 1734.07 s | **1729.65 s** | -4.42 s / -0.25% |
| 宿主全流程 | 1741.263 s | 1741.285 s | +0.022 s |

预构建略快、正式阶段略慢，而宿主全流程几乎完全相同。这种幅度小于单轮
crate 调度和宿主负载波动，准确结论是“性能中性”，不是“已经提速”。

CPU 采样也没有给出一致的局部下降：

| 目标路径，非 idle 样本 | 上轮预构建 | 本轮预构建 | 上轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| `NoPreemptIrqSave::release` | 1561 | 1719 | 577 | 551 |
| 每阶段秒的 release 样本 | 1.352 | 1.497 | 1.003 | 0.954 |
| `disable_preempt` | 437 | 476 | 192 | 216 |
| 每阶段秒的 disable 样本 | 0.379 | 0.415 | 0.334 | 0.374 |

release 在预构建变宽约 10.7%，正式阶段变窄约 4.9%；disable 两个阶段都
变宽。源码中 AMO 已经消失，但采样 PC 会把内联指令归到调用者行，且不同
一轮执行的 guard 次数不完全相同，因此不能用 `atomic_add/sub` 的少量符号
样本证明收益。

本轮修改没有造成功能回归，也没有让 8 核运行退化为单核，但根据现有证据，
不应把它作为一次已经验证有效的性能优化。

## 修改原理

旧代码：

```text
disable_preempt -> AtomicUsize::fetch_add
enable_preempt  -> AtomicUsize::fetch_sub
can_preempt     -> AtomicUsize::load
```

新代码：

```text
disable_preempt -> Cell::get -> checked_add -> Cell::set
enable_preempt  -> Cell::get -> 下溢检查 -> Cell::set
can_preempt     -> Cell::get
```

抢占嵌套深度属于任务正在执行的内核栈状态。现有全部访问点都是：

- 当前 CPU 当前任务的 guard 获取；
- 当前 CPU 当前任务的 guard 释放；
- 当前 run queue 判断当前任务能否抢占；
- 当前任务阻塞前核对预期嵌套深度。

远端 CPU 可以通过 `need_resched: AtomicBool` 请求调度，但不读取或修改
`preempt_disable_count`。`need_resched` 仍保留 Acquire/Release 原子同步，
本轮没有用普通计数替代跨核通知。

## 正确性边界

`TaskInner` 因调度上下文本来就有手写的 `unsafe impl Sync`。为了防止将来
误对非当前任务访问 `Cell`，三个计数入口被改成了 crate 内部的 `unsafe`
函数，前置条件是：

```text
该 TaskInner 必须是当前 CPU 正在执行的当前任务；
其他 CPU 不得同时访问它的抢占嵌套计数。
```

四个调用点都写明了为什么满足条件。

同 CPU 中断可以插入普通 `get/set` 之间。IRQ 入口自身使用嵌套的
`NoPreempt`，其计数变化完整返回时是：

```text
n -> n + 1 -> n
```

所以外层恢复执行时看到的净状态没有变化。若中断触发调度，原任务会暂停；
`on_cpu` 协议保证它不会同时在两个 CPU 执行，恢复后才继续外层操作。

计数增加使用 `checked_add`，计数减少显式检查大于零，避免 release 模式下
溢出或下溢静默回绕。

## 构建前的三遍静态检查

### 第一遍：访问归属与任务唯一执行

第一次审查发现，普通安全方法无法表达 `Cell` 只能由当前任务访问的不变量。
修正为 `unsafe` 内部接口后，第一遍从头重审：

- 全仓库只有四处调用；
- `disable/enable` 的对象来自当前 CPU 的 current-task 指针；
- `preempt_resched` 和 `blocked_resched` 使用 run queue guard 保存的当前任务；
- `on_cpu` 在上下文切换和远端唤醒之间建立唯一执行协议；
- 没有远端 CPU 读取计数的调试、统计或调度路径。

### 第二遍：中断、嵌套与调度

- `NoPreemptIrqSave` 获取顺序仍是先增加计数、再关闭 IRQ；
- 释放顺序仍是先恢复 IRQ、再减少计数；
- 中断插入 `get/set` 之间时，嵌套计数完整恢复；
- 计数从 2 到 1 时不会错误触发调度；
- 只有最外层从 1 到 0 且 `need_resched` 为真时进入调度检查；
- 当前任务切出后，guard 状态随其内核栈保存，不会由另一个任务释放。

### 第三遍：配置、unsafe、格式和 vendor

- `preempt` 关闭时字段和方法完整裁剪；
- 单核和 SMP 配置使用相同的当前任务前置条件；
- `need_resched` 的原子内存序未变；
- 本轮新增代码通过目标文件 rustfmt；
- `run_queue.rs` 存在本轮之前的整文件 rustfmt 差异，没有机械格式化用户已有
  调度代码；
- `git diff --check` 和 `cargo metadata --offline --no-deps` 通过；
- `vendor/axtask/.cargo-checksum.json` 与三个修改文件的 SHA-256 一致。

完成三遍审查后才执行 `make PROFILE=y kernel-rv`，构建用时 12.60 秒并通过。

## 完整测试口径

根文件系统由官方镜像重新独立解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
tmp/profile-rv-preempt-count-local-20260725/sdcard-rv.img
15032385536 bytes
sha256=c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0
```

运行条件：

- Ubuntu 宿主直接运行；
- QEMU RISC-V `virt`；
- 8 GiB、8 vCPU；
- `-snapshot`；
- 无其他 QEMU；
- 从空 target 完整执行 cagent、tg-xtask 预构建、正式 BuildStorm 和退出回收；
- CPU 10 Hz；
- off-CPU 1/8 抽样；
- ext4 1/16 抽样；
- page cache 1/32 抽样；
- 地址空间写锁 1/16 抽样；
- mprotect 1/32 抽样。

cagent 10/10 通过，正式构建成功：

```text
Finished dev target(s) in 19m07s
Finished release target(s) in 8m14s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=510.42
```

`BUILDSTORM_MINIBUILD fail` 仍然存在，但上一轮同样失败，因此不是本轮抢占
计数修改引入的回归。它与后续成功的 tg-xtask 预构建和正式 BuildStorm
分开记录。

采样完整性：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

## 全程监测

前约 12 分钟 QEMU 大部分时间使用 7.7～8.0 个宿主核。预构建到
`440/446` 后，`rustls/reqwest/ureq/axbuild/tg-xtask` 的依赖图收窄，
出现约单核的末端 codegen/link 区间。

正式阶段同样在普通依赖展开时接近 8 核；为目标 JSON 重建
`core/alloc/std/proc_macro` 和最后链接时出现低并行区间。所有低 CPU 区间
都有后续 console 进展，没有满足“长时间无输出且低 CPU”的死锁条件。

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

## 下一轮依据

本轮非 idle CPU 样本中：

```text
预构建：
  NoPreemptIrqSave::release  1719 / 9769
  disable_preempt             476 / 9769
  CurrentTask::from_raw_ptr    729 / 9769

正式：
  NoPreemptIrqSave::release   551 / 4203
  disable_preempt             216 / 4203
  CurrentTask::from_raw_ptr    357 / 4203
```

去掉 AMO 后，release 仍然很宽；每次 guard 释放仍要重新读取 per-CPU
current-task 指针并构造临时 `CurrentTask`。下一轮将让 guard 在获取时保存
它已经解析出的任务指针 token，并在配对释放时直接使用同一个 token：

```text
acquire:
  读取 current task 一次
  增加该任务计数
  将任务 token 保存在 guard state

release:
  从 guard state 取回同一任务 token
  直接减少该任务计数
  不再次读取 per-CPU current task
```

guard state 随任务的内核栈一起切换，因此即使任务在持 guard 时被切出或迁移，
配对释放仍操作原任务。这个修改直接针对 `release + from_raw_ptr` 的持续热点，
但会改变 `kernel_guard` 接口和 guard state，下一轮必须重新做三遍静态审查。
