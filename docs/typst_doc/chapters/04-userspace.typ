#import "../template.typ": evidence

= 系统调用设计

== 分发入口

系统调用统一进入 `kernel/src/syscall/mod.rs::handle_syscall`。入口从 `UserContext` 取出系统调用号，使用 `syscalls::Sysno::new` 解析；无效编号直接返回 `-ENOSYS`。有效系统调用进入一个按类别组织的大型 `match`，调用具体 `sys_*` 函数，最后把 `AxResult<isize>` 转换为 Linux 风格返回值。

`handle_syscall` 不是按文件自动生成的表，而是源码中显式列出每个 `Sysno` 的参数取法和目标函数。部分系统调用带架构条件，例如 x86_64 上保留 `open`、`mkdir`、`poll/select` 等旧 ABI 名称，其他架构更多使用 `openat`、`mkdirat` 等接口。

== 分类

#table(
  columns: (2fr, 3fr, 5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*类别*], [*目录*], [*实现范围*],
  [文件/目录], [`syscall/fs`], [open/read/write/stat/mount/xattr/aio/memfd/inotify/fanotify/pidfd/signalfd/pipe/eventfd 等。],
  [I/O 复用], [`syscall/io_mpx`], [epoll、poll、ppoll、select/pselect 相关入口。],
  [内存], [`syscall/mm`], [brk、mmap、munmap、mprotect、mremap、madvise、msync、mlock、mincore 等。],
  [任务], [`syscall/task`], [clone/clone3、execve、exit、wait、sched、线程控制、作业控制、ptrace 等。],
  [信号], [`syscall/signal.rs`], [rt_sigaction、rt_sigprocmask、kill、tkill、tgkill、sigpending、sigsuspend 等信号接口。],
  [网络], [`syscall/net`], [socket、bind/connect/listen/accept、send/recv、getsockopt/setsockopt、地址转换；受 `net` feature 控制。],
  [IPC], [`syscall/ipc`], [SysV message queue 与 shared memory。],
  [同步], [`syscall/sync`], [futex、robust list、membarrier。],
  [系统/资源/时间], [`syscall/sys.rs`, `resources.rs`, `time.rs`], [uname、prlimit、getrlimit、clock/gettimeofday/nanosleep/timer 相关接口。],
  [BPF], [`syscall/bpf.rs`, `file/bpf.rs`], [面向测试的 map/prog fd 支持和有限指令分类，不是完整 eBPF 框架。],
)

== 用户指针与错误处理

系统调用参数中的用户地址通过 `mm::UserPtr<T>`、`mm::UserConstPtr<T>`、`starry_vm::VmPtr` 等包装读取。`UserPtr` 会检查地址对齐、所属地址空间和访问权限，并在需要时调用 `populate_area` 让懒分配页实际映射。字符串参数通过 null-terminated 扫描校验，普通 buffer 通过切片视图或 `VmBytes/VmBytesMut` 访问。

错误类型主要使用 `axerrno::{AxError, LinuxError}`。内核内部返回 `AxResult<isize>`，系统调用入口负责把错误码写回用户上下文。特殊值 `SYSCALL_RESTART` 用于表示需要按信号语义重启的系统调用。

== BPF 的实际范围

当前 `sys_bpf` 支持 `BPF_MAP_CREATE`、`BPF_MAP_LOOKUP_ELEM`、`BPF_MAP_UPDATE_ELEM` 和 `BPF_PROG_LOAD`。`file/bpf.rs` 中的 map 类型包含 hash、array、ringbuf；program 会按少量指令模式分类成 `Prog01`、`Prog02`、`Prog05` 或 `Noop`，并在 `SO_ATTACH_BPF` 时执行有限逻辑。这是为了兼容相关测试路径的内核对象支持，不能写成完整 verifier、JIT 或 eBPF 运行时。

== 代码依据

#evidence((
  [`分发入口`], [`kernel/src/syscall/mod.rs`], [`handle_syscall` 按 `Sysno` 显式匹配 fs/mm/task/net/signal/time/ipc/sync 等系统调用。],
  [`用户指针`], [`kernel/src/mm/access.rs`], [`UserPtr`/`UserConstPtr` 通过 `check_region`、`can_access_range` 和 `populate_area` 校验访问。],
  [`网络开关`], [`kernel/src/syscall/net/mod.rs`], [`#[cfg(not(feature = "net"))]` 下 socket 族系统调用统一返回 `ENOSYS`。],
  [`BPF 范围`], [`kernel/src/syscall/bpf.rs`, `kernel/src/file/bpf.rs`], [仅实现 map 创建/查找/更新、program load 分类和 attach 时的有限执行。],
))
