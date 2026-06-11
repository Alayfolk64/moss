#import "../template.typ": evidence

= 进程、线程与调度

== 并发模型

StarryOS 的用户线程运行在 `axtask::TaskInner` 之上。内核为每个用户任务创建一个 `Thread` 扩展对象，并通过 `TaskExt` 的 `on_enter/on_leave` 在任务切换进入和离开时设置 `scope-local` 资源域。调度器本身由 ArceOS 的 `axtask` 提供；从 `kernel/Cargo.toml` 可见启用了 `multitask`、`task-ext` 和 `sched-rr` feature，因此文档只描述 StarryOS 在任务上附加的 Linux 兼容状态，不虚构调度器内部队列实现。

== Thread

`Thread` 是线程级状态，主要保存：

#table(
  columns: (2.5fr, 7.5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*字段*], [*含义*],
  [`proc_data`], [指向进程共享数据 `ProcessData`。],
  [`clear_child_tid`], [线程退出时按 `set_tid_address` 语义清除用户地址。],
  [`robust_list_head`], [robust futex 链表头，供 `get_robust_list/set_robust_list` 使用。],
  [`signal`], [线程级 `ThreadSignalManager`。],
  [`time`], [线程 CPU 时间和 interval timer 状态。],
  [`exit` / `exit_event`], [退出标记和可轮询退出事件。],
  [`accessing_user_memory`], [内核正在访问用户内存时的标记。],
)

`new_user_task` 创建的闭包循环执行 `UserContext::run()`。返回原因包括 syscall、page fault、interrupt 和 exception；系统调用交给 `handle_syscall`，缺页交给内存管理，异常按类型转成信号。

== ProcessData

`ProcessData` 是进程共享状态，和 `starry_process::Process` 配合使用：

#table(
  columns: (2.5fr, 7.5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*字段*], [*含义*],
  [`proc`], [`starry_process::Process`，保存 pid、线程、父子关系、进程组/会话等外部库状态。],
  [`exe_path`, `cmdline`], [可供 `/proc` 和 exec 相关路径查询的进程镜像信息。],
  [`aspace`], [该进程共享的用户地址空间。],
  [`scope`], [文件描述符表等 scope-local 资源的作用域。],
  [`heap_top`], [`brk` 当前堆顶。],
  [`rlim`], [进程资源限制，默认设置栈大小和最大 fd 数。],
  [`child_exit_event`, `exit_event`, `wait_status`, `stop_event`], [wait、退出、job-control stop/continue 的事件同步。],
  [`signal`], [进程级 `ProcessSignalManager` 和 signal actions。],
  [`futex_table`], [私有 futex 表。共享映射上的 futex 可通过共享后端/文件后端选择共享 key。],
  [`namespaces`], [pid、uts、user namespace 的 inode 和父子关系状态。],
  [`uid/euid/suid/gid/egid/sgid`], [权限和信号发送检查使用的身份字段。],
)

== 创建、执行和退出

进程/线程系统调用分散在 `kernel/src/syscall/task`。`clone`/`clone3` 创建新任务，`execve` 复用 ELF 装载路径替换地址空间，`exit`/`exit_group` 设置退出状态并唤醒等待者，`wait` 系列通过父子关系和事件等待子进程状态。init 进程在 `entry.rs` 中用 `Process::new_init(pid)` 创建，之后绑定 tty、设置 stdio 并加入任务表。

== 资源限制与命名空间

`task/resources.rs` 定义 `Rlimit`、`Rlimits` 和 `AX_FILE_LIMIT = 1024`。默认 `RLIMIT_STACK` 来自 `USER_STACK_SIZE`，`RLIMIT_NOFILE` 来自 `AX_FILE_LIMIT`。`ProcessNamespaces` 当前保存 pid、uts、user 三类 namespace 的 inode 状态；`inherit_namespaces_from` 可按 clone flag 创建子 namespace，并让新 pid namespace 的 init 进程在可见 pid 上呈现为 1。

== 代码依据

#evidence((
  [`线程扩展`], [`kernel/src/task/mod.rs`], [`Thread` 实现 `TaskExt`，`AsThread` 从 `TaskInner` 获取用户线程。],
  [`用户运行环`], [`kernel/src/task/user.rs`], [`new_user_task` 处理 syscall、page fault、exception 和 signal 检查。],
  [`进程共享状态`], [`kernel/src/task/mod.rs`], [`ProcessData` 保存地址空间、scope、rlimit、signal、futex、namespace、uid/gid 等状态。],
  [`资源限制`], [`kernel/src/task/resources.rs`], [`Rlimits::default` 设置 `RLIMIT_STACK` 与 `RLIMIT_NOFILE`。],
  [`任务系统调用`], [`kernel/src/syscall/task/*`], [clone/execve/exit/wait/thread/job/schedule/ptrace 按文件拆分实现。],
))
