# RISC-V `kernel_guard` 跨 crate 内联迭代报告

## 结论

本轮只在已经完整跑通两次的 `acquire-fastpath` 基线上，为以下四个
`BaseGuard` 方法增加普通 `#[inline]`：

- `NoPreempt::acquire`
- `NoPreempt::release`
- `NoPreemptIrqSave::acquire`
- `NoPreemptIrqSave::release`

没有使用 `#[inline(always)]`，也没有改变 guard 状态、抢占计数、任务指针
生命周期或本地中断的保存/恢复顺序。

三遍静态审查、profiling 内核构建、全新官方镜像、cagent 10/10、完整
tg-xtask 预构建和正式 BuildStorm 均通过。生成了 112 张同口径 SVG，采样
完整：

```text
dropped_cpu=0
dropped_wait=0
dropped_pending=0
skipped_cpu=0
```

正式 BuildStorm 从 512.13 秒变为 508.98 秒，单轮快 3.15 秒，即 0.62%。
这个变化方向是好的，但幅度仍不足以证明稳定的端到端提速。准确结论是：

1. 编译器接受了内联提示，独立的 `NoPreemptIrqSave::release` 机器码符号已
   消失；
2. 调用点仍必须进入 `crate_interface` 导出的 preempt 接口桩；
3. 正式编译单轮快 0.62%，可以保留这项低风险修改，但不能把它宣称为已经
   稳定获得 0.62%；
4. CPU 火焰图中 `release` 的宽度存在明显的“延迟中断采样”偏差，不能继续
   按它的叶样本占比估算 release 自身耗时。

## 完整测试结果

### 测试口径

- Ubuntu 宿主直接启动 QEMU，不套 Docker；
- RISC-V `virt`，8 GiB，8 vCPU；
- 根文件系统使用 `-snapshot`；
- 从官方压缩包重新复制、解压，不复用上一轮 target；
- CPU 采样 10 Hz；
- off-CPU 1/8 抽样并按采样率还原；
- ext4 1/16；
- page cache 1/32；
- AddrSpace 写锁持有 1/16；
- mprotect 1/32。

本轮镜像：

```text
source=/home/wuxun/Downloads/sdcard-rv-pub.img.gz
image=tmp/profile-rv-guard-inline-20260725/sdcard-rv.img
size=15032385536
sha256=c03c6091eb1c400d4c1f400130d11810b00e18811d4e5c201e7f75802dfafde0
```

运行结果：

```text
cagent: 10/10 pass
tg-xtask prebuild: 19m 09s
BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=508.98 cores=8
Cargo release build: 8m 13s
profile host elapsed: 1756.233 s
```

`BUILDSTORM_MINIBUILD fail` 仍是此前已知的独立 minibuild 检查结果；本轮正式
BuildStorm 成功，内联修改没有涉及该脚本。

### 与接受基线对比

| 指标 | acquire-fastpath 基线 | 本轮 | 变化 |
| --- | ---: | ---: | ---: |
| setup phase | 4.29 s | 4.25 s | -0.03 s |
| tg-xtask 预构建 phase | 1144.14 s | 1150.13 s | +5.99 s / +0.52% |
| Cargo 预构建自报 | 19m 03s | 19m 09s | +6 s |
| 正式 profile phase | 556.49 s | 584.82 s | +28.33 s |
| BuildStorm 自报 | 512.13 s | **508.98 s** | **-3.15 s / -0.62%** |
| Cargo 正式构建自报 | 8m 16s | **8m 13s** | -3 s |
| 三个 profile phase 合计 | 1704.91 s | 1739.20 s | +34.29 s |
| 宿主全流程 | 1711.17 s | 1756.23 s | +45.07 s |

`timed phase` 和宿主全流程包括 BuildStorm END 后等待 guest 停止 profiler、
稳定统计表和导出数据的收尾。本轮 END 到 READY 的收尾比基线更长，所以
这两个数字不能代替编译耗时。最接近用户实际目标的指标是
`BUILDSTORM_COMPILE elapsed_s`。

## 三遍静态审查

### 第一遍：回滚和语义

- 已失败的 `pending-self` 修改完全撤销；
- direct-current helper 修改完全撤销；
- `axtask/api.rs` 和 `axtask/task.rs` 恢复到完整成功的
  acquire-fastpath 版本；
- `current_check_preempt_pending()` 继续按原语义重新获取 current task；
- 本轮相对接受基线只增加四个普通内联提示；
- 获取仍是“关本地中断，再增加当前任务 preempt 计数”；
- 释放仍是“恢复原中断状态，再减少 preempt 计数”。

### 第二遍：并发、ABI 和架构

- `NoPreempt::State` 仍为 `()`；
- `NoPreemptIrqSave::State` 仍为保存的 `usize` IRQ 状态；
- `#[inline]` 只向跨 crate 优化提供 MIR，由优化器决定是否采用；
- 没有强制内联，不会无条件复制所有调用点；
- RISC-V 的 `sstatus.SIE` 保存/恢复指令未改变；
- current task 尚未初始化时仍走 `None`；
- preempt 计数仍属于任务，不改成 per-CPU 状态。

### 第三遍：机械检查

- `cargo fmt --all --check` 通过；
- `git diff --check` 通过；
- `cargo metadata --offline --no-deps` 通过；
- 11 个已修改 vendor Rust 文件与各自 `.cargo-checksum.json` 全部匹配；
- 三遍检查完成后才清理 `kernel_guard` 的 RISC-V release 产物并构建。

## 内联实际发生了什么

当前 ELF 中已经找不到独立的：

```text
kernel_guard::...NoPreemptIrqSave...release
```

机器码符号。这说明 release 的指令已被放入调用者。火焰图仍显示
`NoPreemptIrqSave::release`，是因为 DWARF 会保留“这一段内联指令来自哪个
函数”的信息，不代表运行时仍调用了同名独立函数。

但是以下导出桩仍存在：

```text
ffffffc080431982  size=0x3a  __KernelGuardIf_enable_preempt
```

它来自 `crate_interface::impl_interface` 生成的 `extern "Rust"` +
`export_name` 接口。`kernel_guard` 只声明这个外部接口，真实实现位于
`axtask`，因此即使启用 LTO，当前二进制仍保留一次显式调用。

该桩的常见路径做：

1. 建立小栈帧；
2. 从 per-CPU 区读取当前任务指针；
3. 找到 `preempt_disable_count`；
4. 原子减一；
5. 若旧值不是 1，直接返回；
6. 若旧值是 1，进入 pending reschedule 检查。

所以本轮消除的是 guard release 自身的跨 crate 调用边界，不是整条 preempt
释放路径。

## CPU 样本对比

以下都排除了 `task:idle`，并按各自阶段秒数归一化。

| 叶路径 | 基线预构建 | 本轮预构建 | 归一化变化 | 基线正式 | 本轮正式 | 归一化变化 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `NoPreemptIrqSave::release` | 1727 | 1163 | -33.01% | 656 | 417 | -39.51% |
| `release + __KernelGuardIf_enable_preempt` | 1769 | 1279 | -28.08% | 680 | 471 | -34.09% |
| `CurrentTask::from_raw_ptr` | 796 | 760 | -5.02% | 276 | 276 | -4.84% |

本轮非 idle 总样本为：

```text
prebuild=9569
timed=4304
```

当前主要非 idle 叶路径：

| 叶路径 | 预构建 | 正式 |
| --- | ---: | ---: |
| `NoPreemptIrqSave::release` 的内联区间 | 1163 / 12.15% | 417 / 9.69% |
| `CurrentTask::from_raw_ptr` | 760 / 7.94% | 276 / 6.41% |
| lwext4 `memset` 总计 | 578 / 6.04% | 242 / 5.62% |
| `UserContext::run` | 549 / 5.74% | 210 / 4.88% |
| lwext4 `crc32` | 80 / 0.84% | 337 / 7.83% |

## 为什么不能把 release 的 12.15% 当成真实自耗时

CPU profiler 由周期性定时器中断驱动。`NoPreemptIrqSave` 持有期间，本地
中断关闭，因此：

```text
进入临界区
    IRQ disabled
    执行临界区中的实际工作
    定时器到期，但中断暂时不能进入
release
    恢复 SIE
    积压的定时器中断立即进入
    TrapFrame.sepc 落在 release 附近
```

火焰图会把这次样本记到恢复中断附近。它可能代表：

- release 的几条真实指令；
- 此前整个 IRQ-off 临界区积压的定时器；
- 内联后的调用点和 DWARF 行号变化。

因此：

- release 样本下降可以证明采样落点发生了改善；
- 反汇编可以证明独立调用边界被删除；
- 但仅凭下降的 33%～40%，不能声称 release 的真实 CPU 时间下降了同样
  比例；
- 必须用 BuildStorm 端点或专门的 IRQ-off 持有时长探针验证收益。

本轮 BuildStorm 只快 0.62%，与上述限制一致。

## 当前同步热点

本轮写锁持有探针给出的估算总任务时间如下。它们是多个任务的累计时间，
不是可以直接相加的墙钟时间：

| 事件 | 预构建 | 正式 |
| --- | ---: | ---: |
| clone child 写锁持有 | 70.80 s | 30.37 s |
| exit clear 写锁持有 | 59.35 s | 25.25 s |
| mprotect 写锁持有 | 48.30 s | 21.58 s |
| exec clear 写锁持有 | 35.54 s | 12.10 s |
| clone parent 写锁持有 | 35.51 s | 16.58 s |
| munmap 写锁持有 | 28.38 s | 11.03 s |

这些路径仍值得后续继续拆，但本轮内联没有修改它们，不能把其变化归因于
guard。

## 下一轮选择：优化 lwext4 `memset`

排除延迟中断导致的 release 归因偏差后，最稳定、同时出现在预构建和正式
阶段的真实 CPU 叶热点是 lwext4 自带的 `memset`：

```c
while (n) {
    *p++ = (unsigned char)c;
    --n;
}
```

它对 4 KiB 块也逐字节写入。当前样本：

```text
prebuild: 578 / 9569 = 6.04%
timed:     242 / 4304 = 5.62%
```

下一轮将把它改成：

1. 先逐字节写到 `sizeof(size_t)` 对齐；
2. 构造重复字节的 `size_t` 填充值；
3. 每轮写多个机器字；
4. 最后处理不足一个机器字的尾部。

这不会改变 ext4 数据、锁、页缓存或 I/O 语义，且比继续根据偏置的 release
火焰宽度修改调度器更可验证。`crc32` 是正式阶段更大的单阶段热点，但它的
校验算法修改风险更高，放在 `memset` 验证之后。

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
