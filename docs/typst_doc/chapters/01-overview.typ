#import "../template.typ": os_name, evidence

= 概述

== 项目定位

#os_name 是一个使用 Rust 编写的 Linux 兼容内核工程。工作区 `Cargo.toml` 将项目描述为 “A Linux-compatible OS kernel built on ArceOS unikernel”，主体 crate 为 `starry-kernel`，顶层二进制 `starryos` 负责选择 init 脚本并调用内核入口。

从当前代码看，系统不是从零实现所有底层机制，而是在 ArceOS/ax 组件之上组织 Linux 用户态兼容层。`kernel/Cargo.toml` 依赖 `axruntime`、`axhal`、`axtask`、`axmm`、`axfs`、`axnet` 等模块，并启用 `uspace`、`multitask`、`sched-rr`、`fs-ext4`、`fs-fat`、`rtc` 等 feature。内核自身负责用户地址空间、ELF 装载、系统调用、进程/线程共享状态、伪文件系统、信号和部分 Linux 兼容对象。

== 当前代码确认的能力

#table(
  columns: (2.2fr, 6.8fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*模块*], [*当前实现范围*],
  [启动与 init], [`src/main.rs` 内置 init 脚本，`kernel/src/entry.rs` 挂载伪文件系统、创建 init 进程、加载用户 ELF 并进入用户态。],
  [系统调用], [`kernel/src/syscall/mod.rs` 使用 `syscalls::Sysno` 统一分发到 fs、mm、task、signal、net、ipc、sync、time、io_mpx、resources 等子模块。],
  [内存管理], [`AddrSpace` 组合用户虚拟地址范围、`MemorySet<Backend>` 和页表；后端包含 alloc、CoW、shared、file、linear 映射。],
  [进程/线程], [`Thread` 作为 `axtask` 扩展，`ProcessData` 保存进程共享的地址空间、资源域、信号管理、futex 表、命名空间和 rlimit。],
  [文件与伪文件系统], [`FileLike` 抽象统一普通文件、目录、管道、socket、eventfd、timerfd、signalfd、pidfd、BPF 对象等；启动时挂载 `/dev`、`/dev/shm`、`/tmp`、`/proc` 和部分 `/sys` 节点。],
  [信号], [`starry_signal` 作为核心信号库，内核实现 `rt_sigaction`、`rt_sigprocmask`、`kill/tkill/tgkill`、信号检查、默认动作和 job-control stop/continue。],
  [网络], [`net` feature 开启时 socket 走 `axnet`，覆盖 TCP/UDP、Unix socket、send/recv、socket options；关闭时相关系统调用返回 `ENOSYS`。],
  [IPC/同步], [包含 futex、robust list、SysV message queue、SysV shared memory、eventfd、pipe、epoll/poll/select 等等待与复用机制。],
  [设备支持], [`/dev/fb0` 由 `axdisplay::has_display()` 决定是否暴露；`input` feature 下提供 evdev 风格输入目录；还包含 tty/pty、rtc、loop、random、zero、null 等设备节点。],
)

== 文档边界

本文档只描述当前源码可以确认的设计。没有代码证据的能力不写成“已支持”：例如桌面环境、X11/Wayland 服务、完整 eBPF verifier、真实网卡驱动细节、完整 Linux VM 语义和测试成绩，都不在本文中扩展。测试章节只记录已有脚本入口和执行机制，不捏造通过率或性能分数。

== 代码依据

#evidence((
  [`项目描述`], [`Cargo.toml`], [`workspace.package.description` 明确 Linux-compatible 与 ArceOS unikernel 定位。],
  [`内核边界`], [`kernel/src/lib.rs`], [`pub mod entry`，内部模块包含 `config/file/mm/pseudofs/syscall/task/time`。],
  [`功能开关`], [`Cargo.toml`, `kernel/Cargo.toml`], [`qemu` 依赖 `starry-kernel/net`，内核 feature 包含 `net/input/dev-log/memtrack`。],
))
