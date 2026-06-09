#import "../template.typ": evidence

= IPC 与同步

== Futex

`kernel/src/task/futex.rs` 实现 futex wait queue 和 futex table。`FutexKey` 会区分私有地址、shared mmap 和文件映射：普通私有地址用进程私有 futex 表，共享内存使用 `SharedPages` 的弱引用，文件映射使用文件后端的 futex handle。这样不同进程对同一共享映射地址可以落到共享 wait queue。

`sys_futex` 支持 `FUTEX_WAIT`、`FUTEX_WAIT_BITSET`、`FUTEX_WAKE`、`FUTEX_WAKE_BITSET`、`FUTEX_REQUEUE`、`FUTEX_CMP_REQUEUE`。等待路径会先读取用户地址做 fast path 检查，然后进入 `WaitQueue::wait_if`；唤醒路径按数量和 bitset 唤醒等待者。robust list 通过 `sys_get_robust_list` 和 `sys_set_robust_list` 读取或写入 `Thread.robust_list_head`。

== SysV IPC

`syscall/ipc` 分为 message queue 和 shared memory。

#table(
  columns: (2fr, 3fr, 5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*类型*], [*路径*], [*实现要点*],
  [Message Queue], [`syscall/ipc/msg.rs`], [`MessageQueue` 保存 `msqid_ds`、按 mtype 分类的消息、队列字节数和删除标记；`MsgManager` 维护 key 到 msqid 和 msqid 到队列的映射。],
  [Shared Memory], [`syscall/ipc/shm.rs`], [`ShmInner` 保存 shmid、页数、共享页、attach 进程地址范围和 `ShmidDs`；`ShmManager` 管理 key/shmid/pid/vaddr 关系。],
)

共享内存物理页使用 `SharedPages`，映射时进入 `Backend::Shared`，因此与 mmap 的 shared backend 共用底层设计。

== I/O 复用

`epoll` 分为 syscall 层和文件对象层。`sys_epoll_create1` 创建 `Epoll` fd，`sys_epoll_ctl` 添加/修改/删除 interest，`sys_epoll_pwait` 与 `sys_epoll_pwait2` 调用 `poll_io` 等待 ready events。`file/epoll.rs` 中的 interest 支持 level-trigger、edge-trigger 和 one-shot 模式，并通过 Waker 把被观察 fd 的 readiness 推入 ready queue。

`poll`、`ppoll`、`select`、`pselect` 位于 `syscall/io_mpx` 的其他文件，和 `FileLike::poll`/`register` 共享 readiness 抽象。

== eventfd、pipe 与 signalfd

这些对象都实现 `FileLike + Pollable`：

- `eventfd` 用原子计数器实现跨 fd 通知。
- `pipe` 用 ring buffer 实现字节流，并支持 `FIONREAD`、非阻塞和 `SIGPIPE`。
- `signalfd` 将 pending signal 转成可 poll 的 fd。
- `timerfd` 将定时器 expiration 转成可读 fd。

它们既属于文件描述符对象，也构成用户态同步/通知机制的一部分。

== 代码依据

#evidence((
  [`futex 表`], [`kernel/src/task/futex.rs`], [`FutexKey` 区分 private/shared/file，`WaitQueue` 支持 wait/wake/requeue。],
  [`futex syscall`], [`kernel/src/syscall/sync/futex.rs`], [`sys_futex` 分发 wait/wake/requeue 和 robust list 接口。],
  [`消息队列`], [`kernel/src/syscall/ipc/msg.rs`], [`MessageQueue` 与 `MsgManager` 管理 SysV message queue 状态。],
  [`共享内存`], [`kernel/src/syscall/ipc/shm.rs`], [`ShmInner` 与 `ShmManager` 管理 shared pages、attach/detach 和 shmid。],
  [`epoll`], [`kernel/src/syscall/io_mpx/epoll.rs`, `kernel/src/file/epoll.rs`], [`Epoll` fd 管理 interest、trigger mode 和 ready queue。],
))
