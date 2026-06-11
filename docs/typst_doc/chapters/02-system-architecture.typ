#import "../template.typ": os_name, evidence

= 系统架构

== 分层结构

当前 #os_name 的代码可以分为四层：

#table(
  columns: (2fr, 4fr, 4fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*层次*], [*主要代码*], [*职责*],
  [顶层镜像], [`src/main.rs`, `src/init.sh`], [选择 init 脚本，构造 BusyBox shell 命令行，注入 LTP 用例环境变量，调用 `starry_kernel::entry::init`。],
  [内核入口], [`kernel/src/entry.rs`], [挂载伪文件系统，创建用户地址空间，加载 init ELF，绑定控制终端，设置 stdio，启动并等待 init 任务。],
  [Linux 兼容层], [`kernel/src/syscall`, `kernel/src/task`, `kernel/src/mm`, `kernel/src/file`, `kernel/src/pseudofs`], [处理系统调用、用户地址空间、任务共享状态、文件描述符对象和 Linux 风格伪文件系统。],
  [ArceOS/ax 基础层], [`axruntime`, `axhal`, `axtask`, `axmm`, `axfs`, `axnet`], [提供平台启动、页表与陷入上下文、调度、文件系统、网络、设备和运行时基础设施。],
)

系统调用从用户上下文返回后进入 `handle_syscall`，按 `Sysno` 分发到各子模块。普通文件、设备、管道、socket、eventfd 等都被包装为 `FileLike`，由文件描述符表保存。用户内存通过 `UserPtr`、`UserConstPtr` 和 `starry_vm` 访问，避免系统调用直接信任用户地址。

== 模块关系

```text
用户程序
  -> UserContext::run()
  -> syscall / page fault / exception / interrupt
  -> syscall::{fs, mm, task, signal, net, ipc, sync, time, io_mpx}
  -> task::ProcessData / mm::AddrSpace / file::FileLike / pseudofs
  -> axfs / axnet / axtask / axhal / axmm
```

`Thread` 挂在 `axtask::TaskInner` 的扩展槽中；调度和任务运行由 `axtask` 提供，线程进入/离开时设置 `scope-local` 资源域。进程实体来自 `starry_process::Process`，进程的 OS 兼容状态由 `ProcessData` 保存。

== 代码目录

#table(
  columns: (3fr, 7fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*目录或文件*], [*作用*],
  [`src/main.rs`], [顶层 `starryos` 二进制入口，内置默认 init 脚本并按 feature 选择 shell/debug 入口。],
  [`kernel/src/entry.rs`], [内核 initproc 创建流程。],
  [`kernel/src/syscall`], [Linux 系统调用分发和各类实现。],
  [`kernel/src/mm`], [用户地址空间、ELF 装载、用户指针与 mmap/brk 支持。],
  [`kernel/src/task`], [线程扩展、进程共享数据、信号、futex、timer、资源限制。],
  [`kernel/src/file`], [文件描述符对象、`FileLike`、pipe、eventfd、timerfd、signalfd、epoll、pidfd、bpf 和 socket 包装。],
  [`kernel/src/pseudofs`], [devfs、procfs、tmpfs 和通用 SimpleFs/Device 支持。],
  [`kernel/src/config`], [按架构选择用户空间、栈、堆、trampoline 等常量。],
)

== Feature 与条件编译

顶层 `qemu` feature 选择默认平台、PCI 总线和内核网络功能；`vf2` 启用 VisionFive2 相关 SDMMC 驱动；`smp` 打开 axfeat 的 SMP；`shell-init` 让入口使用交互 shell 脚本。内核 crate 还定义 `net`、`input`、`dev-log`、`memtrack` 等 feature。网络相关源码在 `kernel/src/syscall/net` 中通过 `#[cfg(feature = "net")]` 切换，输入设备也在 `/dev/input` 下通过 `#[cfg(feature = "input")]` 暴露。

== 代码依据

#evidence((
  [`模块声明`], [`kernel/src/lib.rs`], [`entry/file/mm/pseudofs/syscall/task/time` 构成内核主要模块边界。],
  [`任务扩展`], [`kernel/src/task/mod.rs`], [`Thread` 实现 `TaskExt`，`AsThread` 从 `TaskInner` 取出用户线程状态。],
  [`文件抽象`], [`kernel/src/file/mod.rs`], [`FileLike` 统一 read/write/stat/ioctl/poll/path，fd 表使用 `FlattenObjects<FileDescriptor, AX_FILE_LIMIT>`。],
  [`条件编译`], [`kernel/src/syscall/net/mod.rs`], [`net` 关闭时 `sys_socket` 等函数返回 `ENOSYS`，开启时 re-export net 子模块。],
))
