# RISC-V BuildStorm 抢占 guard 任务 token 优化报告

## 结论

本轮让 `NoPreempt` 和 `NoPreemptIrqSave` 在获取时保存当前任务 token，配对
释放时直接使用同一 token，不再从 per-CPU 区域重新读取 current-task 指针。

完整冷镜像运行成功，但与使用同样 `AtomicUsize` 计数的第六轮相比，端到端
没有收益：

| 指标 | 第六轮 release fast path | 本轮 state token | 变化 |
| --- | ---: | ---: | ---: |
| tg-xtask 预构建输出 | 19m13s | 19m12s | -1s |
| profile 预构建阶段 | 1154.52 s | 1153.30 s | -1.22 s / -0.11% |
| BuildStorm | **508.49 s** | 509.17 s | +0.68 s / +0.13% |
| profile 正式阶段 | **575.20 s** | 576.71 s | +1.51 s / +0.26% |
| profile 三阶段总和 | **1734.07 s** | 1734.33 s | +0.26 s / +0.015% |
| 宿主全流程 | 1741.263 s | **1741.194 s** | -0.069 s |

这组差异接近零，应判定为性能中性。

局部 CPU 采样说明成本发生了转移：

| 非 idle 路径 | 第六轮预构建 | 本轮预构建 | 第六轮正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| `NoPreemptIrqSave::release` | 1561 | **1462** | 577 | **575** |
| `disable_preempt` | **437** | 557 | **192** | 209 |
| release + disable | **1998** | 2019 | **769** | 784 |

按阶段秒数归一化：

- release：预构建下降约 6.2%，正式下降约 0.6%；
- disable：预构建上升约 27.6%，正式上升约 8.6%；
- 两者合计：预构建上升约 1.2%，正式上升约 1.7%。

释放端少了一次 current-task 读取，但获取端需要生成 token，所有 guard state
也从单个 IRQ 状态扩为“任务 token + IRQ 状态”。额外的 state 搬运抵消了
释放端局部下降。端点与局部合计证据一致：这项修改不值得保留。

## 修改方式

`KernelGuardIf` 从：

```text
disable_preempt()
enable_preempt()
```

改为：

```text
state = disable_preempt()
enable_preempt(state)
```

`state` 是 `Arc::as_ptr(current_task)` 得到的不透明 token。guard 获取后已经
增加该任务的抢占禁用计数，因此任务不能被抢占；若任务通过显式调度切出，
guard state 会随它的内核栈保存，恢复后仍使用原 token。

`NoPreemptIrqSave::State` 从一个 `usize` 变成两个 `usize`：

```text
(preempt_state, irq_state)
```

释放顺序仍然是：

```text
释放 spin lock
-> 恢复本地 IRQ
-> 用 preempt_state 递减原任务的抢占计数
-> 必要时检查 need_resched
```

## 安全修正

本轮最初建立在第七轮的任务本地 `Cell<usize>` 上。第一遍静态审查进一步
发现：裸 kernel guard 类型可以被移动，仓库当前虽然没有跨任务移动它，但
普通 `Cell` 会让误用升级为潜在数据竞争；第七轮也没有测出收益。

因此在构建前已经将抢占计数恢复为 `AtomicUsize`。本轮最终二进制是：

```text
AtomicUsize preempt count + saved task token
```

这也使它可以与第六轮：

```text
AtomicUsize preempt count + release 重新读取 current task
```

直接比较，隔离 token 的效果。

## 三遍静态检查

### 第一遍：token 生命周期和接口覆盖

- 全仓库只有一个 `KernelGuardIf` 实现；
- 所有手工 `BaseGuard::acquire` 调用通过关联类型保存 state，没有写死类型；
- 当前任务切出前，scheduler、等待队列或退出队列保留任务强引用；
- guard 会释放的任务不可能在 token 使用前析构；
- 无当前任务的启动阶段返回 token 0，配对释放保持 no-op。

### 第二遍：配对、嵌套和上下文切换

- `lock()` 成功路径把 token 保存进 guard；
- `try_lock()` 失败路径立即用同一 token 配对释放；
- 嵌套 guard 各自保存同一任务指针并按栈顺序递减；
- IRQ 恢复仍发生在最外层抢占计数递减之前；
- 任务切出后恢复的是自己的内核栈和自己的 guard state；
- token 使用后不再借用任务指针，计数归零触发调度不会形成悬空访问。

### 第三遍：cfg、ABI、格式和 vendor

- `preempt` 关闭时 state 为 0，接口调用完整裁剪；
- user-mode guard 仍别名为 `NoOp`；
- crate-interface 支持 `usize` 返回值和参数；
- 新增代码通过目标格式检查；
- 原 `kernel_guard` 示例块注释有既存 rustfmt 差异，未机械改写；
- `git diff --check`、离线 metadata 和三个 vendor checksum 通过。

首次构建暴露 Cargo 复用了 7 月 24 日的 vendored `kernel_guard` rlib，导致
axtask 看到旧 ABI。只清理 `kernel_guard@0.1.3` 的 RISC-V release 生成缓存
后，同一份已审查源码构建通过；没有为通过构建修改代码。

## 完整测试

官方镜像重新独立解压：

```text
/home/wuxun/Downloads/sdcard-rv-pub.img.gz
tmp/profile-rv-preempt-state-token-20260725/sdcard-rv.img
15032385536 bytes
sha256=c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0
```

条件保持不变：

- Ubuntu 宿主直接运行；
- QEMU RISC-V `virt`；
- 8 GiB、8 vCPU；
- `-snapshot`；
- 无其他 QEMU；
- 完整 cagent、tg-xtask 预构建、正式 BuildStorm、退出回收；
- CPU 10 Hz，off-CPU 1/8，ext4 1/16，page cache 1/32；
- 地址空间写锁 1/16，mprotect 1/32。

cagent 10/10 通过：

```text
Finished dev target(s) in 19m12s
Finished release target(s) in 8m13s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=509.17
```

既有的 `BUILDSTORM_MINIBUILD fail` 仍存在，正式流程成功。采样完整性：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

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

## 决策

本轮复杂化三个 crate 的接口、增大每个 guard state，却没有净收益。下一步
撤销 token 接口并回到第六轮简单快速路径，不再继续在采样归属不稳定的
guard 微路径上叠加复杂度。

后续重新选择缺页路径中可归因的热点：`handle_page_fault_concurrent`、
页表遍历和 backend 元数据查找。它们在预构建非 idle 样本中占比更大，也
可以通过明确减少查找次数或页表层级访问来验证，而不是继续移动 guard 成本。
