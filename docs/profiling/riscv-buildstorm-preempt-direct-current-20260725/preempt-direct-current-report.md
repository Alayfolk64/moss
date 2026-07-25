# RISC-V preempt 当前任务无包装访问报告

## 结论

本轮尝试在 preempt acquire/release hook 内直接使用 per-CPU 当前任务裸指针，
避免先构造不增减引用计数的 `CurrentTask(ManuallyDrop<Arc<_>>)` 包装。

修改通过三遍静态审查、profiling 内核构建、全新官方镜像、cagent 10/10、
完整预构建和正式 BuildStorm。112 张同口径 SVG 已生成。采样结果为：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=2
```

两次 `skipped_cpu` 表示采样中断到达时对应 CPU 的 profile buffer
`try_lock` 正忙，约占数万次采样中的极小部分，不影响主要排序，但本轮不能
写成“零跳过”。

这个修改单独使用没有达到目标，不应凭 BuildStorm 的 0.71 秒差异宣称提速：

| 指标 | acquire 快路径基准 | 本轮 | 变化 |
| --- | ---: | ---: | ---: |
| setup | 4.29 s | 4.10 s | -0.18 s |
| tg-xtask 预构建 phase | 1144.14 s | 1145.63 s | +1.49 s / +0.13% |
| Cargo 预构建自报 | 19m 03s | 19m 04s | +1 s |
| 正式 profile phase | 556.49 s | 579.07 s | +22.58 s / +4.06% |
| BuildStorm 自报 | 512.13 s | 511.42 s | -0.71 s / -0.14% |
| Cargo 正式构建自报 | 8m 16s | 8m 15s | -1 s |
| 三个 profile phase 合计 | 1704.91 s | 1728.81 s | +23.89 s / +1.40% |
| 宿主全流程 | 1711.17 s | 1741.22 s | +30.05 s / +1.76% |

最接近真实正式编译的 `BUILDSTORM_COMPILE` 只变化 -0.14%，属于单轮波动；
较长的 profile phase 和宿主收尾时间也不能归因于这几条指针指令。

## 为什么 `from_raw_ptr` 没有消失

本轮改变了 hook：

```text
读取 CURRENT_TASK_PTR
ptr.as_ref()
直接调用 curr.disable_preempt / curr.enable_preempt
```

但 `TaskInner::enable_preempt` 在最外层计数由 1 减为 0 时，无论
`need_resched` 是否为真，都会调用：

```text
current_check_preempt_pending
    crate::current()
        CurrentTask::try_get
            CurrentTask::from_raw_ptr
```

所以 direct helper 删除了 hook 开头的一次包装，却在每次最外层 release
继续通过 pending 检查重新构造包装。火焰图中的父路径证明了这一点：

| `CurrentTask::from_raw_ptr` 父路径 | 预构建样本 | 占该符号 |
| --- | ---: | ---: |
| `BaseSpinLockGuard::drop` | 255 | **35.27%** |
| `drop_in_place<NoPreempt>` | 64 | 8.85% |
| 其他正常 `current()` 查询 | 404 | 55.88% |

也就是说，这个符号并不等于“hook 包装开销”；一半以上本来就是业务路径上
需要的当前任务查询。

## CPU 样本对比

| 非 idle 叶/路径 | 基准预构建 | 本轮预构建 | 基准正式 | 本轮正式 |
| --- | ---: | ---: | ---: | ---: |
| `NoPreemptIrqSave::release` | 1727 / 18.13% | 1895 / 19.91% | 656 / 16.08% | 671 / 15.79% |
| `CurrentTask::from_raw_ptr` | 796 / 8.36% | 723 / 7.60% | 276 / 6.76% | 344 / 8.10% |
| 普通 `disable_preempt` | 198 | 196 | 64 | 94 |
| `local_irq_save_and_disable` | 83 | 100 | 41 | 33 |

按阶段秒数归一化，`from_raw_ptr` 在预构建只下降约 9.3%，正式阶段反而
上升约 19.8%。不同阶段的任务构成会影响它，无法证明 direct helper 自身
带来稳定 CPU 收益。

## 三遍静态审查

### 第一遍：裸指针生命周期

- helper 不返回 `&AxTask`，引用不会逃逸；
- current 未初始化时空指针继续返回；
- 最外层 enable 触发调度时，调度器在切换前持有旧任务的其他 `Arc`，且有
  `strong_count > 1` 断言；
- helper 在可能 reschedule 的调用返回后不再访问旧指针；
- 退出队列和调度器同样在任务切换期间保活对象。

### 第二遍：guard 和架构语义

- 普通 `NoPreempt` release 由尚未递减的 preempt 计数保护 per-CPU 读取；
- `NoPreemptIrqSave` acquire 由 IRQ 关闭保护，release 由 preempt 计数保护；
- 嵌套 guard 保持逐层加减；
- RISC-V、LoongArch、AArch64 和 x86 的迁移保护条件不变；
- 两个 unsafe helper 只有 `KernelGuardIf` 实现处调用。

### 第三遍：机械检查

- `cargo fmt --all --check` 通过；
- `git diff --check` 通过；
- `cargo metadata --offline --no-deps` 通过；
- 全部 10 个已修改 vendor 源文件 checksum 匹配；
- 三遍结束后才精确清理 `axtask` 并构建，依赖内核组件全部重新编译。

## 下一步

最新火焰图给出了比“撤销 direct helper”更有价值的组合方案：

1. 保留不返回引用的 direct helper；
2. 将 `current_check_preempt_pending()` 改为接收已有的 `&self`；
3. 先用 `self.need_resched` 做快速判断；
4. 只有确实需要调度时才构造 `CurrentRunQueueRef`，该路径原本就必须取得
   current task；
5. 不在每次最外层 spin-lock release 上再次调用 `crate::current()`。

这个改动直接对应 `BaseSpinLockGuard::drop -> CurrentTask::from_raw_ptr` 的
255 个预构建样本。它不保存跨调度点 token，也不改变 guard 大小或 per-task
计数语义，风险比 per-CPU 重写 preempt count 小。

## 测试口径

- 官方全新 `sdcard-rv-pub.img.gz`；
- 镜像大小 15,032,385,536 字节；
- SHA-256
  `c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0`；
- Ubuntu 宿主直接运行；
- QEMU RISC-V `virt`，8 GiB，8 vCPU，`-snapshot`；
- CPU 10 Hz，off-CPU 1/8，ext4 1/16，page cache 1/32，
  AddrSpace hold 1/16，mprotect 1/32。

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
