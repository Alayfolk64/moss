#import "../template.typ": evidence

= 信号机制

== 信号状态

StarryOS 使用 `starry_signal` crate 保存信号语义，内核侧把进程级和线程级状态挂到 `ProcessData` 与 `Thread`：

#table(
  columns: (2.3fr, 3fr, 4.7fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*位置*], [*类型*], [*作用*],
  [`ProcessData.signal`], [`ProcessSignalManager`], [进程级 pending signal、signal actions 和 trampoline 地址。],
  [`Thread.signal`], [`ThreadSignalManager`], [线程级 pending/blocked signal，并和进程级 manager 协作。],
  [`ProcessData.wait_status`], [`ProcessWaitStatus`], [保存 job-control stop/continue 状态，供 waitid 等接口观察。],
  [`ProcessData.stop_event`], [`PollSet`], [唤醒因停止信号阻塞的线程。],
)

信号 trampoline 地址来自 `config::SIGNAL_TRAMPOLINE`，在 ELF 装载时映射到用户地址空间，权限为用户可读可执行。

== 系统调用接口

`kernel/src/syscall/signal.rs` 实现 Linux 信号相关系统调用。当前代码中可确认的核心接口包括：

- `rt_sigaction`：读取/设置进程 signal action，禁止修改 `SIGKILL` 和 `SIGSTOP`。
- `rt_sigprocmask`：按 `SIG_BLOCK`、`SIG_UNBLOCK`、`SIG_SETMASK` 修改线程 blocked mask。
- `rt_sigpending`：读取当前线程 pending signal。
- `kill`：按 pid、0、-1、负 pid 的 Linux 规则发送信号，并做 uid/euid/suid 权限检查。
- `tkill` / `tgkill`：按 tid 或 tgid+tid 发送线程定向信号。
- `sigsuspend`、`sigaltstack`、`rt_sigtimedwait` 等接口也在同一文件中实现。

== 信号投递与默认动作

`task/signal.rs::check_signals` 是用户态返回点前的信号检查路径。它先处理 ptrace stop，再从线程/进程信号管理器中取出可递送信号。默认动作按 `SignalOSAction` 处理：

#table(
  columns: (2fr, 8fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*动作*], [*内核行为*],
  [`Terminate`], [调用 `do_exit(signo, true)` 退出进程。],
  [`CoreDump`], [以 `signo | 0x80` 形式退出。],
  [`Stop`], [设置 `wait_status = Stopped(signo)`，唤醒父进程，然后等待 continue。],
  [`Continue`], [设置 `wait_status = Continued(signo)`，唤醒 stop waiters 和父进程。],
  [`Handler`], [信号库已经修改用户上下文准备进入 handler，内核侧不再执行默认动作。],
)

Page fault 和异常也通过信号反馈给用户程序：无法处理的用户缺页会产生 `SIGBUS` 或 `SIGSEGV`，非法指令、断点、不对齐访问等异常会映射到 `SIGILL`、`SIGTRAP` 或 `SIGBUS`。

== signalfd

`signalfd4` 在文件系统 syscall 模块中实现，最终创建 `file/signalfd.rs::Signalfd`。该对象实现 `FileLike` 和 `Pollable`，按 mask 从当前线程 pending signal 中出队，并向用户缓冲区写入 128 字节的 `signalfd_siginfo` 兼容结构。它提供了把信号纳入 poll/epoll 的路径。

== 代码依据

#evidence((
  [`系统调用`], [`kernel/src/syscall/signal.rs`], [`rt_sigaction`、`rt_sigprocmask`、`kill/tkill/tgkill` 等接口直接操作 signal manager。],
  [`信号检查`], [`kernel/src/task/signal.rs`], [`check_signals` 按 `SignalOSAction` 处理 terminate/core/stop/continue/handler。],
  [`用户返回点`], [`kernel/src/task/user.rs`], [每轮用户上下文返回后调用 `check_signals`，缺页/异常失败会发 fatal signal。],
  [`signalfd`], [`kernel/src/syscall/fs/signalfd.rs`, `kernel/src/file/signalfd.rs`], [`signalfd4` 创建或更新 `Signalfd`，read 时输出 `signalfd_siginfo`。],
))
