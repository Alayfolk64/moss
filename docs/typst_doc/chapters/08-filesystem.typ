#import "../template.typ": evidence

= 文件与伪文件系统

== FileLike 与 fd 表

内核使用 `FileLike` trait 统一文件描述符对象。普通文件、目录、管道、socket、eventfd、timerfd、signalfd、epoll、pidfd、BPF map/program 都可以放入 fd 表。`FileLike` 提供 `read`、`write`、`stat`、`ioctl`、`poll`、`path`、`nonblocking`、`status_flags` 等接口；fd 表通过 `scope_local` 绑定到当前进程的 `Scope`。

`AX_FILE_LIMIT` 当前为 1024，fd 表类型是 `FlattenObjects<FileDescriptor, AX_FILE_LIMIT>`。`FileDescriptor` 除了对象本身，还保存 `cloexec` 标志，供 exec 处理 close-on-exec 行为。

== 普通文件与目录

`kernel/src/file/fs.rs` 包装 `axfs` 的文件和目录对象。`resolve_at` 实现 `AT_FDCWD`、`AT_EMPTY_PATH`、`AT_SYMLINK_NOFOLLOW` 等 Linux 风格路径解析。`metadata_to_kstat` 把 VFS metadata 转换为内核内部 `Kstat`，再由 `From<Kstat> for stat/statx` 输出给用户态。

文件读写路径会根据 VFS node flags 区分直接阻塞和 `poll_io` 等待；读取成功时还按 mount flags 和 `O_NOATIME` 更新 atime。`fcntl` 的异步 I/O owner 状态由 `ASYNC_FCNTL_STATE` 保存，可在 pipe 写入时触发 `SIGIO`。

== 伪文件系统挂载

`pseudofs::mount_all` 在 init 进程启动前挂载以下节点：

#table(
  columns: (2fr, 3fr, 5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*挂载点*], [*文件系统*], [*说明*],
  [`/dev`], [`devfs`], [字符设备、块设备 stub、tty/pty、loop、random/zero/null/full、rtc、可选 fb/input。],
  [`/dev/shm`], [`tmpfs`], [内存文件系统，供共享内存和测试程序使用。],
  [`/tmp`], [`tmpfs`], [临时文件目录。],
  [`/proc`], [`procfs`], [进程目录、状态文件、部分 sysctl 和信息文件。],
  [`/sys`], [`tmpfs` + 静态节点], [创建 graphics/fb0/device、loop queue 等测试期望路径。],
)

`pseudofs/fs.rs` 提供 `SimpleFs`，用 slab 分配 inode；`pseudofs/tmp.rs` 提供 `MemoryFs`，支持内存目录、普通文件、符号链接和扩展属性等基础操作；`pseudofs/device.rs` 定义 `DeviceOps` 和 `DeviceMmap`。

== 管道与匿名 fd

`Pipe` 使用 ring buffer 保存数据，并通过 `PollSet` 支持读写等待和关闭唤醒。写端检测读端关闭时发送 `SIGPIPE` 并返回 `BrokenPipe`。`pipe2` 支持 `O_CLOEXEC` 和 `O_NONBLOCK`。

`eventfd` 使用一个 `AtomicU64` 计数器实现，支持 semaphore 模式和 poll；`timerfd` 维护 deadline、interval、expiration 计数，并在需要时启动内核 worker；`signalfd` 从当前线程 pending signal 中按 mask 取出信号并输出 Linux `signalfd_siginfo` 大小的结构。

== procfs

`kernel/src/pseudofs/proc.rs` 动态生成进程和线程目录，例如 `/proc/[pid]/task/[tid]`。它还能输出 task status、pagemap、timens offsets、部分 dummy 信息文件。由于文件里存在 `DUMMY_MEMINFO`、`DUMMY_CPUINFO` 等常量，本文把这些写作“测试兼容信息”，不把它们解释成真实硬件统计。

== 代码依据

#evidence((
  [`fd 抽象`], [`kernel/src/file/mod.rs`], [`FileLike` trait 和 `FD_TABLE` 定义文件描述符对象模型。],
  [`普通文件`], [`kernel/src/file/fs.rs`], [`File`/`Directory` 包装 axfs，`resolve_at` 和 `metadata_to_kstat` 支持 Linux 路径与 stat。],
  [`伪文件系统`], [`kernel/src/pseudofs/mod.rs`], [`mount_all` 挂载 `/dev`、`/dev/shm`、`/tmp`、`/proc`、`/sys`。],
  [`tmpfs`], [`kernel/src/pseudofs/tmp.rs`], [`MemoryFs` 保存 inode、目录项、文件长度、symlink 和 xattrs。],
  [`匿名 fd`], [`kernel/src/file/pipe.rs`, `event.rs`, `timerfd.rs`, `signalfd.rs`], [pipe/eventfd/timerfd/signalfd 都实现 `FileLike` 和 `Pollable`。],
))
