#import "../template.typ": evidence

= 附录 B：系统调用分类索引

本文档不手工列全量 syscall。系统调用全集应以 `kernel/src/syscall/mod.rs` 的 `handle_syscall` match 为准，因为该文件同时记录了架构条件、参数取法和具体目标函数。

#table(
  columns: (2fr, 3fr, 5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*分类*], [*文件*], [*示例入口*],
  [文件控制], [`fs/ctl.rs`], [`chdir`、`mkdirat`、`unlinkat`、`renameat2`、`sync`、xattr 相关入口。],
  [fd 操作与 I/O], [`fs/fd_ops.rs`, `fs/io.rs`], [`openat`、`close`、`dup3`、`fcntl`、`read/write`、`pread/pwrite`、`sendfile`。],
  [特殊 fd], [`fs/pipe.rs`, `fs/event.rs`, `fs/signalfd.rs`, `fs/pidfd.rs`], [`pipe2`、`eventfd2`、`signalfd4`、`pidfd_*`。],
  [I/O 复用], [`io_mpx/*`], [`epoll_create1`、`epoll_ctl`、`epoll_pwait`、`ppoll`、`pselect`。],
  [内存], [`mm/*`], [`brk`、`mmap`、`munmap`、`mprotect`、`mremap`、`mincore`。],
  [任务], [`task/*`], [`clone`、`clone3`、`execve`、`wait4`、`exit_group`、`sched_yield`、`ptrace`。],
  [信号], [`signal.rs`], [`rt_sigaction`、`rt_sigprocmask`、`kill`、`tgkill`、`sigsuspend`。],
  [网络], [`net/*`], [`socket`、`bind`、`connect`、`accept4`、`sendmsg`、`recvmsg`、`getsockopt`。],
  [IPC/同步], [`ipc/*`, `sync/*`], [`msgget/msgsnd/msgrcv`、`shmget/shmat`、`futex`、`membarrier`。],
  [时间/资源/系统], [`time.rs`, `resources.rs`, `sys.rs`], [`clock_gettime`、`nanosleep`、`getrlimit/prlimit`、`uname` 等。],
  [BPF], [`bpf.rs`], [`bpf` 系统调用的有限 map/prog 支持。],
)

== 代码依据

#evidence((
  [`总分发`], [`kernel/src/syscall/mod.rs`], [`handle_syscall` 是系统调用支持情况的一手来源。],
  [`子模块`], [`kernel/src/syscall/*`], [`mod fs/mm/task/net/ipc/sync/io_mpx/resources/signal/time/sys/bpf` 按类别拆分实现。],
))
