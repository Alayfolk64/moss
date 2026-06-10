= 进程与线程管理

== StarryOS 基线与不足

=== StarryOS 的进程线程模型

StarryOS 提供了基础的进程线程管理能力。在数据结构设计上，StarryOS 将进程相关数据划分为两层：Thread 结构存储线程级数据，包括线程级信号管理器、clear_child_tid、robust_list_head、时间统计等字段；ProcessData 结构存储进程内所有线程共享的数据，包括地址空间、资源作用域、信号处理器、futex 表、资源限制等。

在系统调用层面，StarryOS 实现了 fork、clone、execve、exit、wait4 等基本调用，支持进程创建、程序加载和进程回收。clone 系统调用支持 CLONE_VM、CLONE_FILES、CLONE_SIGHAND、CLONE_THREAD 等标志，可以创建共享地址空间的线程或独立地址空间的进程。wait4 系统调用可以等待子进程退出并回收僵尸进程。

=== 存在的主要问题

尽管 StarryOS 提供了进程线程管理的基础框架，但在 Linux 兼容性和功能完整性上仍存在明显不足，主要体现在以下五个方面。

第一，权限管理缺失。ProcessData 结构仅有 umask 字段用于文件权限掩码，缺少 UID/GID 管理机制。Linux 采用 real UID、effective UID、saved UID 三元组设计，分别用于身份标识、权限检查和权限恢复，同时 GID 也有对应的三元组。StarryOS 缺少这些字段，导致 setuid、setreuid、setresuid 等系统调用无法实现，权限检查无法正确进行，许多需要权限切换的程序无法运行。

第二，等待语义不完整。wait4 系统调用仅支持等待僵尸进程，不支持作业控制状态的等待。Linux 的 wait 系统调用通过 WUNTRACED 和 WCONTINUED 选项可以等待被 SIGSTOP 停止或 SIGCONT 继续的子进程，这是作业控制的核心机制。StarryOS 缺少 ProcessWaitStatus 状态记录，缺少 stop_event 唤醒机制，导致 shell 的作业控制功能无法正常工作，相关 LTP 测试无法通过。

第三，命名空间缺失。Linux 容器技术依赖命名空间实现进程隔离，包括 PID namespace、UTS namespace、User namespace 等。StarryOS 完全缺少命名空间支持，无法实现 /proc/self/ns/* 接口，无法支持 CLONE_NEWPID、CLONE_NEWUTS 等标志，容器化场景无法运行。

第四，调试支持薄弱。ptrace 系统调用是 Linux 调试器的基础，StarryOS 缺少完整的 ptrace 实现，无法支持 gdb 等调试工具。prctl 系统调用选项不完整，许多进程属性无法配置。pidfd 机制完全缺失，无法以文件描述符方式操作进程。

第五，进程组与会话管理不完善。setpgid/getpgid 实现存在问题，setsid 创建会话的语义不正确，作业控制信号处理缺失，导致终端作业控制功能无法正常使用。

== 权限管理的完善

=== UID/GID 三元组设计

Linux 进程权限管理采用三元组设计：real UID（RUID）表示进程实际所有者，用于身份标识和信号发送权限检查；effective UID（EUID）表示进程当前有效权限，用于文件访问、系统调用等权限检查；saved UID（SUID）保存进程的原始特权身份，用于在降低权限后恢复。GID 也有对应的三元组设计。

moss 在 ProcessData 结构中新增六个 AtomicU32 字段：uid、euid、suid、gid、egid、sgid，完整支持 Linux 权限模型。所有字段使用原子类型，避免在多线程访问时的数据竞争。初始化时，所有 UID/GID 字段均设置为 0（root 权限），与 StarryOS 保持兼容。

```rust
pub struct ProcessData {
    // ... 其他字段 ...
    uid: AtomicU32,
    euid: AtomicU32,
    suid: AtomicU32,
    gid: AtomicU32,
    egid: AtomicU32,
    sgid: AtomicU32,
}
```

=== setuid 系列系统调用实现

moss 实现了完整的 UID/GID 管理系统调用，这些调用在 `kernel/src/syscall/task/ctl.rs` 中实现。根据 Linux 语义，权限切换的安全性规则如下表所示。

#[
  #set text(size: 9pt)
  #table(
    columns: (1.2fr, 1.5fr, 2.3fr),
    align: (left, left, left),
    inset: 5pt,
    table.header([系统调用], [调用者权限], [行为]),
    [setuid(uid)], [EUID = 0], [同时设置 RUID、EUID、SUID 为 uid],
    [], [EUID ≠ 0], [仅当 uid 为 RUID 或 SUID 时，设置 EUID 为 uid],
    [setreuid(ruid, euid)], [EUID = 0], [可设置为任意值（-1 表示不修改）],
    [], [EUID ≠ 0], [ruid 仅可设为 RUID/EUID，euid 仅可设为 RUID/EUID/SUID],
    [setresuid(ruid, euid, suid)], [EUID = 0], [可独立设置三个 UID 为任意值],
    [], [EUID ≠ 0], [每个 UID 仅可设为当前三个 UID 之一],
  )
]

setuid 系统调用根据调用者权限采用不同策略：若 EUID 为 0（特权进程），则同时设置 RUID、EUID、SUID 为目标值，实现完全的身份切换；若 EUID 非 0（非特权进程），则仅当目标值为当前 RUID 或 SUID 时允许设置 EUID，用于在已保存的身份之间切换。

setreuid 支持同时修改 RUID 和 EUID，并接受 -1 表示不修改对应字段。关键在于 SUID 的更新规则：当 RUID 被修改，或 EUID 被设置为非 RUID 的值时，系统将 SUID 更新为新的 EUID。这一规则防止进程在降低权限后通过 setreuid 重新提升权限，确保权限切换的单向性。setresuid 提供了最灵活的控制，允许独立设置三个 UID，非特权进程仅能将每个 UID 设为当前三个 UID 之一。

这些实现使得 moss 通过了 LTP 的 setuid01-04、setreuid01-05、setresuid01-04 等测试用例，支持了 su、sudo 等权限管理工具的正常运行。

=== 权限检查的应用

UID/GID 三元组在 moss 的多个模块中应用于权限控制。在 execve 系统调用中，has_exec_permission 函数基于文件权限位和进程 UID/GID 判断是否允许执行：root 用户只要文件有任意执行位即可执行；普通用户按照所有者、组、其他的顺序检查对应的执行位（0o100、0o010、0o001）。在信号发送中，kill 系统调用检查发送者 EUID 是否为 0，或发送者 EUID 是否等于接收者 RUID 或 EUID。

== 作业控制与等待语义

=== 作业控制状态设计

作业控制是 Unix 终端管理的核心机制，允许用户通过 Ctrl-Z 暂停前台进程、通过 fg/bg 命令恢复进程。实现作业控制需要进程能够记录和报告 stopped 与 continued 状态。

moss 新增 ProcessWaitStatus 枚举和相关字段：

```rust
#[derive(Debug, Clone, Copy)]
pub enum ProcessWaitStatus {
    Stopped(Signo),    // 进程被停止信号暂停
    Continued(Signo),  // 进程被 SIGCONT 继续
}

pub struct ProcessData {
    // ... 其他字段 ...
    pub wait_status: SpinNoIrq<Option<ProcessWaitStatus>>,
    pub stop_event: Arc<PollSet>,
}
```

当进程收到 SIGSTOP、SIGTSTP、SIGTTIN、SIGTTOU 等停止信号时，信号处理逻辑将 wait_status 设置为 Stopped 状态，唤醒父进程，并通过异步轮询阻塞当前线程直到收到 SIGCONT。收到 SIGCONT 信号时，设置状态为 Continued 并唤醒被阻塞的线程。

=== wait 系列调用增强

moss 增强了 wait4 和 waitid 系统调用，支持完整的作业控制语义。wait 系统调用支持的选项如下表所示。

#[
  #set text(size: 9pt)
  #table(
    columns: (1.2fr, 3.8fr),
    align: (left, left),
    inset: 5pt,
    table.header([选项], [含义]),
    [WNOHANG], [不阻塞，若无可报告状态立即返回 0],
    [WUNTRACED], [报告被停止信号（SIGSTOP/SIGTSTP）暂停的子进程状态],
    [WCONTINUED], [报告被 SIGCONT 继续的子进程状态],
    [WEXITED], [报告已终止的子进程（waitid 专用）],
    [WNOWAIT], [非破坏性查询，不清除子进程状态（可重复查询）],
  )
]

wait 系统调用通过 wait_status_siginfo 检查子进程的作业控制状态。若子进程处于 Stopped 状态且调用者指定了 WUNTRACED，则构造 siginfo_t 返回 CLD_STOPPED 码；若处于 Continued 状态且指定了 WCONTINUED，则返回 CLD_CONTINUED 码。WNOWAIT 选项实现非破坏性查询，不清除子进程状态。

进程退出时，根据 exit_code 的不同值判断退出类型：若终止信号为 0 表示正常退出（CLD_EXITED），bit 7 置位表示 core dump（CLD_DUMPED），否则为信号终止（CLD_KILLED）。通过这些增强，moss 支持了 shell 的作业控制功能，LTP 的 waitpid05-13、waitid01-02 等测试用例得以通过。

=== 进程组与会话管理

moss 实现了进程组和会话管理，支持 setpgid、getpgid、setsid 等系统调用。进程组用于批量信号发送，会话用于终端管理。

setpgid 系统调用将进程移动到指定进程组。pid=0 表示当前进程，pgid=0 表示使用目标进程的 PID 作为进程组 ID。实现通过 Process.group().set_pgid() 修改进程组归属。setsid 创建新会话，将调用进程设为会话首进程和进程组组长。

== 命名空间支持

=== 命名空间数据结构

Linux 命名空间（namespace）是容器技术的基础，为进程提供隔离的系统视图。moss 实现了 PID、UTS、User 三种命名空间的基础支持。

命名空间数据结构定义如下：

```rust
#[derive(Debug, Clone, Copy)]
pub struct NamespaceState {
    pub inode: u64,                // 命名空间的唯一标识
    pub parent_inode: Option<u64>, // 父命名空间 inode
    pub init_pid: Option<u32>,     // 命名空间内的 init 进程 PID
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessNamespaces {
    pub pid: NamespaceState,
    pub uts: NamespaceState,
    pub user: NamespaceState,
}
```

初始命名空间使用预定义的 inode 编号：

#[
  #set text(size: 9pt)
  #table(
    columns: (1.5fr, 1.5fr, 2fr),
    align: (left, left, left),
    inset: 5pt,
    table.header([命名空间类型], [初始 inode], [用途]),
    [PID namespace], [0x0f00_0001], [进程 PID 隔离，支持容器内 PID 重映射],
    [UTS namespace], [0x0f00_0002], [主机名和域名隔离],
    [User namespace], [0x0f00_0003], [UID/GID 映射，支持非特权容器],
  )
]

新创建的命名空间从 0x0f00_0100 开始分配 inode，通过 AtomicU64 保证唯一性。

=== 命名空间继承与 PID 转换

clone 系统调用根据 CLONE_NEWPID、CLONE_NEWUTS、CLONE_NEWUSER 标志创建新命名空间。创建 PID namespace 时，当前进程成为该命名空间的 init 进程（init_pid 字段记录），子命名空间的 parent_inode 指向父命名空间。

PID namespace 的关键特性是 PID 重映射：命名空间内的 init 进程在内部看到自己的 PID 为 1，而在外部看到的是真实 PID。visible_pid 方法实现此转换：

```rust
pub fn visible_pid(&self) -> u32 {
    let namespaces = self.namespaces.lock();
    if namespaces.pid.init_pid == Some(self.proc.pid()) {
        1  // 命名空间 init 进程显示为 PID 1
    } else {
        self.proc.pid()  // 其他进程显示真实 PID
    }
}
```

命名空间 inode 用于实现 /proc/self/ns/* 接口，每个命名空间文件的 inode 编号对应 NamespaceState.inode，供用户空间程序查询和比较命名空间。

== 进程创建与执行

=== clone 系统调用完善

clone 是 Linux 统一的进程/线程创建接口，通过 CloneFlags 控制资源共享策略。moss 支持的主要标志如下表所示。

#[
  #set text(size: 9pt)
  #table(
    columns: (1.5fr, 3.5fr),
    align: (left, left),
    inset: 5pt,
    table.header([CloneFlags], [含义]),
    [CLONE_VM], [共享地址空间（线程）；否则 COW 复制（进程）],
    [CLONE_FILES], [共享文件描述符表；否则复制 FD 表],
    [CLONE_SIGHAND], [共享信号处理器；否则继承处理器],
    [CLONE_THREAD], [加入同一线程组，共享 PID],
    [CLONE_PARENT_SETTID], [在父进程内存写入子进程 TID],
    [CLONE_CHILD_SETTID], [在子进程内存写入子进程 TID],
    [CLONE_CHILD_CLEARTID], [子进程退出时清零指定地址并唤醒 futex],
    [CLONE_PIDFD], [创建 pidfd 文件描述符],
    [CLONE_VFORK], [父进程阻塞直到子进程 exec 或 exit],
  )
]

clone 系统调用首先验证标志组合的合法性。Linux 要求 CLONE_THREAD 必须同时指定 CLONE_VM 和 CLONE_SIGHAND，因为线程必须共享地址空间和信号处理器；CLONE_SIGHAND 必须指定 CLONE_VM，因为信号处理器位于共享内存中。验证逻辑位于 `kernel/src/syscall/task/clone.rs`：

```rust
if flags.contains(CloneFlags::THREAD)
    && !flags.contains(CloneFlags::VM | CloneFlags::SIGHAND) {
    return Err(AxError::InvalidInput);
}
if flags.contains(CloneFlags::SIGHAND) && !flags.contains(CloneFlags::VM) {
    return Err(AxError::InvalidInput);
}
```

根据标志组合，clone 创建不同类型的任务。CLONE_THREAD 表示线程，共享 ProcessData 和页表；否则创建新进程，通过 proc.fork() 建立父子关系。CLONE_VM 决定是否共享地址空间，否则执行 COW 复制。

moss 修复了 CLONE_PARENT_SETTID、CHILD_SETTID、CHILD_CLEARTID 的多个 bug（commit f82940ee），确保 TID 写入的时机和地址正确。修复了 fork 时堆指针未继承的问题（commit 759a887e），避免子进程 brk 损坏。实现了 clone3 系统调用（commit 0f15bf48），支持更多扩展参数。

=== execve 系统调用增强

execve 系统调用用于加载新程序，替换当前进程的地址空间。moss 在 StarryOS 基础上增强了 execve 的多个方面。

第一，实现 execveat 系统调用（commit aaeea303），支持相对目录文件描述符打开可执行文件。当 path 为空且设置 AT_EMPTY_PATH 标志时，直接使用 dirfd 指向的文件；否则进行常规路径解析。

第二，实现 shebang 支持（commit c720e652）。当可执行文件以 `#!` 开头时，解析 shebang 行，提取解释器路径，将原文件作为参数传递给解释器：

```rust
if content.starts_with(b"#!") {
    let shebang_line = content.split(|&b| b == b'\n').next()?;
    let interpreter = parse_shebang(shebang_line)?;
    // 递归执行解释器，原文件路径作为参数
    return execve_common(uctx, interpreter, None, Some(&path), argv, envp);
}
```

第三，修复 execve 允许 NULL argv 和 envp（commit ed0e8be4），符合 POSIX 语义。修复 execve 导致的 brk 损坏（commit 26d53945），在加载新程序时正确重置堆指针。第四，execve 执行前进行安全检查，包括权限检查、ETXTBSY 检测（文件被写入打开时禁止执行）、fanotify 执行通知等。

== 调试与控制接口

=== ptrace 调试支持

ptrace 是 Linux 调试器的基础，允许一个进程观察和控制另一个进程的执行。moss 实现了 sys_ptrace 系统调用（commit 09d4c46c），支持多种 ptrace 操作。

#[
  #set text(size: 9pt)
  #table(
    columns: (1.8fr, 3.2fr),
    align: (left, left),
    inset: 5pt,
    table.header([ptrace 操作], [功能]),
    [PTRACE_TRACEME], [标记当前进程为被跟踪状态，父进程成为调试器],
    [PTRACE_ATTACH], [附加到目标进程，开始跟踪],
    [PTRACE_DETACH], [停止跟踪目标进程],
    [PTRACE_CONT], [继续执行被停止的进程],
    [PTRACE_SYSCALL], [继续执行，在下一个系统调用入口或出口停止],
    [PTRACE_SINGLESTEP], [单步执行一条指令],
    [PTRACE_GETREGS], [读取目标进程的寄存器],
    [PTRACE_SETREGS], [设置目标进程的寄存器],
    [PTRACE_PEEKDATA], [读取目标进程内存],
    [PTRACE_POKEDATA], [写入目标进程内存],
  )
]

ptrace 实现维护了一个全局的跟踪状态表，记录每个进程的调试状态和等待事件。被跟踪进程在执行系统调用、收到信号或触发断点时停止执行，并通过 ptrace_wait_if_stopped 阻塞等待调试器发送 PTRACE_CONT 或 PTRACE_SYSCALL 继续执行。ptrace 修复了多个等待事件处理的 bug（commit 71e46a7c、faa52684），确保调试器能正确获取进程状态。通过 ptrace 实现，moss 支持了 gdb、strace 等调试工具。

=== prctl 进程控制

prctl 系统调用提供了进程属性的细粒度控制。moss 实现了多个常用的 prctl 选项（commit 6b393c38、4669f0fa），包括 PR_SET_NAME/PR_GET_NAME 设置和获取进程名称，PR_SET_PDEATHSIG 设置父进程死亡时发送的信号，PR_CAPBSET_READ 查询 capability bounding set 等。对于未支持的选项，系统返回成功避免程序崩溃。

=== pidfd 文件描述符机制

pidfd 将进程表示为文件描述符，提供了更安全的进程操作方式。传统的 PID 可能被回收重用，而 pidfd 在进程存活期间保持有效。

moss 实现了 pidfd_open 和 pidfd_send_signal（commit 71c73e0d）。pidfd_open 创建 PidFd 对象并加入文件描述符表，PidFd 持有进程的 exit_event 用于等待进程退出。pidfd_send_signal 通过 pidfd 获取目标进程 PID，调用 send_signal_to_process 发送信号。clone 系统调用支持 CLONE_PIDFD 标志，自动创建 pidfd 并写回用户空间指针。pidfd 可以通过 epoll 监听进程退出事件（commit 46a389f3），为异步进程管理提供了便利。

moss 的进程与线程管理从 StarryOS 的基础框架出发，通过补齐权限管理、作业控制、命名空间、调试接口等关键能力，显著提升了 Linux 兼容性。这些改进使得 moss 能够运行更多真实世界的 Linux 程序，通过了大量 LTP 测试用例，为后续系统功能扩展奠定了坚实基础。
