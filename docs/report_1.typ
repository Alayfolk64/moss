#set text(
  font: ("Times New Roman", "SimSun"),
  size: 11pt,
  lang: "zh",
)
#set page(
  paper: "a4",
  margin: (x: 2.5cm, y: 2.2cm),
)
#set par(
  first-line-indent: (amount: 2em, all: true),
  justify: true,
  leading: 0.75em,
)
#show raw.where(block: true): it => {
  block(
    fill: rgb("#f6f6f6"),
    inset: 1em,
    radius: 6pt,
    width: 100%,
    text(
      font: ("Consolas", "KaiTi"),
      size: 10pt,
      it,
    ),
  )
}
#set heading(numbering: "1.1")

= 系统概述

== 系统简介

moss 是基于 StarryOS 继续开发的 Rust 操作系统内核。StarryOS 提供了组件化的内核基础，包括任务管理、地址空间管理、文件系统框架、系统调用分发、硬件抽象层和多架构平台支持。moss 延续这一设计思路，并在其基础上进一步完善内核功能。

moss 的主要目标是增强系统运行 Linux 用户态程序的能力。系统围绕用户态运行环境和常见系统调用语义，对进程与线程管理、调度与同步、内存管理、文件系统、I/O、信号处理和网络等模块进行了扩展，使用户程序能够获得更完整、稳定的运行环境。

在整体设计上，moss 保持 StarryOS 的模块化结构，通过补齐系统调用语义、资源管理、错误处理、阻塞唤醒和伪文件系统接口等能力，提升系统的实用性与兼容性。同时，moss 支持 RISC-V64 与 LoongArch64 双架构运行，为后续功能验证、应用适配和系统扩展提供了基础。

== 系统架构与模块划分

moss 采用分层与模块化结合的系统结构。整体上，系统可以划分为用户空间、系统服务接口层、内核核心功能层、组件化内核基础层以及平台适配层。用户程序通过系统调用、异常或中断等入口进入内核，内核根据请求类型调度相应的功能模块完成资源管理与系统服务，并最终通过硬件抽象层适配不同体系结构和运行平台。

#figure(
  image("pictrues/架构图.svg", width: 85%),
  caption: [moss 系统整体架构图],
) <fig:moss-arch>

如 @fig:moss-arch 所示，moss 的核心功能并不是由单一模块完成，而是由多个内核子系统协同构成。任务与执行管理负责进程、线程和调度相关逻辑；地址空间与内存管理负责虚拟内存、物理内存和缺页处理；文件与存储管理负责文件系统抽象、路径解析和文件对象管理；I/O 与事件通知负责文件描述符、异步 I/O、事件等待和多路复用；IPC 与同步机制负责信号、管道、futex 和等待队列等进程间协作能力。网络通信子系统、系统资源管理和内核运行支撑则共同补充完整的操作系统运行环境。

从代码组织上看，moss 主要目录结构如下所示。

#figure(
  [
    ```text
    moss
    ├── Cargo.toml                    # Rust workspace 配置
    ├── Makefile                      # 项目构建入口
    ├── src/                          # 用户态入口与初始化脚本
    │   ├── main.rs                   # 用户态启动入口配置
    │   ├── init.sh                   # 初始化脚本与运行环境准备
    │   ├── init_shell.sh             # 交互式 shell 初始化脚本
    │   └── ltp_testcases.sh          # 测试用例列表与运行配置
    ├── kernel/src/                   # moss 内核主体
    │   ├── config/                   # 内核配置与运行参数
    │   ├── task/                     # 任务、进程、线程、信号与 futex
    │   ├── mm/                       # 地址空间、ELF 加载、mmap 与缺页处理
    │   │   └── aspace/               # 用户地址空间与映射后端
    │   ├── file/                     # 文件对象、FD 表、pipe、eventfd 与 epoll
    │   ├── pseudofs/                 # procfs、sysfs、devfs 等伪文件系统
    │   │   └── dev/                  # 设备文件节点与 tty 支持
    │   └── syscall/                  # Linux 风格系统调用分发与实现
    │       ├── task/                 # 进程、线程、wait、exec、ptrace 等调用
    │       ├── mm/                   # brk、mmap、mremap、mlock 等调用
    │       ├── fs/                   # VFS、路径、mount、xattr 与文件 I/O 调用
    │       ├── io_mpx/               # select、poll、epoll 等 I/O 多路复用
    │       ├── ipc/                  # System V IPC 与共享内存相关调用
    │       ├── sync/                 # futex、membarrier 等同步相关调用
    │       └── net/                  # socket、网络选项与 BPF 相关调用
    ├── third_party/                  # 第三方组件与外部库适配
    │   ├── lwext4_rust/              # Ext4 支持库及 Rust 封装
    │   └── syscalls/                 # 多架构 syscall 编号与辅助表
    ├── vendor/                       # 固定版本的本地依赖与 ArceOS 组件
    ├── make/                         # 构建规则、平台配置与链接脚本
    ├── scripts/                      # 镜像准备、运行测试与辅助脚本
    └── docs/                         # 项目文档与报告材料
    ```
  ],
  caption: [moss 主要代码目录],
) <lst:moss-code-layout>

== 系统主要完成情况

当前 moss 已经能够完成 RISC-V64 和 LoongArch64 双架构构建，启动 busybox 用户态，准备 musl/glibc 运行环境，并运行 LTP 与比赛测试。系统主要完成情况如下。

#[
  #set text(size: 9pt)
  #table(
    columns: (1.1fr, 3.6fr),
    align: (left, left),
    inset: 5pt,
    table.header([模块], [当前完成情况]),
    [双架构运行], [支持 RISC-V64、LoongArch64，生成 `kernel-rv`、`kernel-la`，可通过 QEMU 启动运行。],
    [用户态环境], [以 busybox 为入口，准备 musl/glibc、动态链接器、基础目录和 LTP 运行环境。],
    [系统调用], [已覆盖进程、内存、文件、I/O、时间、信号、同步、网络、IPC 等主要 syscall 类别。],
    [进程管理], [支持 fork、clone、exec、wait、pidfd、ptrace、UID/GID、capability 和进程组等语义。],
    [内存管理], [支持 ELF 加载、COW、lazy allocation、mmap、mremap、mlock 和用户态缺页处理。],
    [文件系统], [支持 Ext4、tmpfs/ramfs、VFS 路径解析、FD 表、mount、xattr、stat/statx 和 loop device。],
    [伪文件系统], [补充 `/proc`、`/sys`、`/dev`，提供进程信息、挂载信息、设备节点和测试探测接口。],
    [I/O 与事件], [支持 read/write、pread/pwrite、sendfile、splice、AIO、pipe、fcntl、poll/epoll、inotify、fanotify。],
    [时间与同步], [支持 clock、gettimeofday、nanosleep、itimer、POSIX timer、timerfd、time namespace 和 futex。],
    [网络与 BPF], [支持本地回环 TCP/UDP、Unix socket、部分 socket option 和最小 BPF map/program 行为。],
    [工程化评测], [固定工具链和 vendor 依赖，提供镜像准备、运行库注入、自动运行和结果汇总脚本。],
  )
]

相对 StarryOS 基线，moss 的主要增量包括四点：一是 syscall 分发项由 211 扩展到 264，新增 54 项，新增 61 个 `sys_*` 实现并增强 82 个已有实现；二是补齐文件系统、I/O、进程、时间、信号和内存等模块的 Linux 兼容语义；三是扩展 `/proc`、`/sys`、`/dev` 和 init 脚本，使 busybox、glibc/musl 程序和 LTP 用例能够稳定运行；四是补齐 RISC-V64、LoongArch64 双架构构建运行与评测结果收集流程。

在后续章节中，本文将按照功能模块展开介绍 moss 的设计与实现。其中，第 2 章介绍进程与线程管理，第 3 章介绍调度与同步机制，第 4 章介绍内存管理，第 5 章介绍文件系统，第 6 章介绍 I/O 与事件通知，第 7 章介绍信号与异常处理，第 8 章介绍网络与 BPF 兼容，第 9 章介绍平台适配与工程化评测。

= 进程与线程管理

== StarryOS 基线与不足

=== StarryOS 的进程线程模型

StarryOS 提供了基础的进程线程管理能力。在数据结构设计上，StarryOS 将进程相关数据划分为两层：Thread 结构存储线程级数据，包括线程级信号管理器、clear_child_tid、robust_list_head、时间统计等字段；ProcessData 结构存储进程内所有线程共享的数据，包括地址空间、资源作用域、信号处理器、futex 表、资源限制等。

在系统调用层面，StarryOS 实现了 fork、clone、execve、exit、wait4 等基本调用，支持进程创建、程序加载和进程回收。clone 系统调用支持 CLONE_VM、CLONE_FILES、CLONE_SIGHAND、CLONE_THREAD 等标志，可以创建共享地址空间的线程或独立地址空间的进程。wait4 系统调用可以等待子进程退出并回收僵尸进程。

=== 存在的主要问题

尽管 StarryOS 提供了进程线程管理的基础框架，但在 Linux 兼容性和功能完整性上仍存在明显不足，主要体现在以下五个方面。

第一，权限管理缺失。ProcessData 结构仅有 umask 字段用于文件权限掩码，缺少 UID/GID 管理机制。Linux 采用 real UID、effective UID、saved UID 三元组设计，分别用于身份标识、权限检查和权限恢复，同时 GID 也有对应的三元组。StarryOS 缺少这些字段，导致 setuid、setreuid、setresuid 等系统调用无法实现，权限检查无法正确进行，许多需要权限切换的程序无法运行。

第二，等待语义不完整。wait4 系统调用仅支持等待僵尸进程，不支持作业控制状态的等待。Linux 的 wait 系统调用通过 WUNTRACED 和 WCONTINUED 选项可以等待被 SIGSTOP 停止或 SIGCONT 继续的子进程，这是作业控制的核心机制。StarryOS 缺少 ProcessWaitStatus 状态记录，缺少 stop_event 唤醒机制，导致 shell 的作业控制功能无法正常工作，相关 LTP 测试无法通过。

第三，命名空间缺失。Linux 容器技术依赖命名空间实现进程隔离，包括 PID namespace、UTS namespace、User namespace 等。StarryOS 完全缺少命名空间支持，无法实现 `/proc/self/ns/*` 接口，无法支持 CLONE_NEWPID、CLONE_NEWUTS 等标志，容器化场景无法运行。

第四，调试支持薄弱。ptrace 系统调用是 Linux 调试器的基础，StarryOS 缺少完整的 ptrace 实现，无法支持 gdb 等调试工具。prctl 系统调用选项不完整，许多进程属性无法配置。pidfd 机制完全缺失，无法以文件描述符方式操作进程。

第五，进程组与会话管理不完善。setpgid/getpgid 实现存在问题，setsid 创建会话的语义不正确，作业控制信号处理缺失，导致终端作业控制功能无法正常使用。

== 权限管理的完善

=== UID/GID 三元组设计

Linux 进程权限管理采用三元组设计：real UID（RUID）表示进程实际所有者，用于身份标识和信号发送权限检查；effective UID（EUID）表示进程当前有效权限，用于文件访问、系统调用等权限检查；saved UID（SUID）保存进程的原始特权身份，用于在降低权限后恢复。GID 也有对应的三元组设计。

moss 在 ProcessData 结构中新增六个 AtomicU32 字段：uid、euid、suid、gid、egid、sgid，完整支持 Linux 权限模型。所有字段使用原子类型，避免在多线程访问时的数据竞争。初始化时，所有 UID/GID 字段均设置为 0（root 权限），与 StarryOS 保持兼容。


ProcessData 结构中权限相关字段如下：

```rust
pub struct ProcessData {
    // ... 其他字段 ...
    umask: AtomicU32,
    ioprio: AtomicU32,
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

setuid 系统调用的实现逻辑如下：

```rust
pub fn sys_setuid(uid: u32) -> AxResult<isize> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    if proc_data.euid() == 0 {
        // 特权进程：同时设置所有 UID
        proc_data.set_uid(uid);
        Ok(0)
    } else if uid == proc_data.uid() || uid == proc_data.suid() {
        // 非特权进程：仅能在已有身份间切换
        proc_data.set_euid(uid);
        Ok(0)
    } else {
        Err(AxError::from(LinuxError::EPERM))
    }
}
```

setreuid 实现了更灵活的权限切换，支持交换 RUID 和 EUID。关键代码片段：

```rust
pub fn sys_setreuid(ruid: u32, euid: u32) -> AxResult<isize> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let old_ruid = proc_data.uid();
    let old_euid = proc_data.euid();
    let old_suid = proc_data.suid();

    // 计算新的 EUID（-1 表示不修改）
    let new_euid = if euid != UID_GID_NO_CHANGE { euid } else { old_euid };

    // 非特权进程权限检查
    if old_euid != 0 {
        if !valid_id_change(ruid, &[old_ruid, old_euid])
            || !valid_id_change(euid, &[old_ruid, old_euid, old_suid])
        {
            return Err(AxError::from(LinuxError::EPERM));
        }
    }

    // 应用修改
    if ruid != UID_GID_NO_CHANGE {
        proc_data.set_real_uid(ruid);
    }
    if euid != UID_GID_NO_CHANGE {
        proc_data.set_euid(euid);
    }

    // Linux 语义：修改 RUID 或将 EUID 设为非 RUID 时，更新 SUID
    if ruid != UID_GID_NO_CHANGE || (euid != UID_GID_NO_CHANGE && euid != old_ruid) {
        proc_data.set_suid(new_euid);
    }

    Ok(0)
}
```

这些实现使得 moss 通过了 LTP 的 setuid01-04、setreuid01-05、setresuid01-04 等测试用例。


=== 权限检查的应用

UID/GID 三元组在 moss 的多个模块中应用于权限控制。在 execve 系统调用中，`has_exec_permission` 函数基于文件权限位和进程 UID/GID 判断是否允许执行：

```rust
fn has_exec_permission(loc: &Location, uid: u32, gid: u32) -> AxResult<bool> {
    let meta = loc.metadata()?;
    if meta.node_type != NodeType::RegularFile {
        return Ok(false);
    }

    let mode = meta.mode.bits() as u32;

    // root 特权：只要有任意执行位即可
    if uid == 0 {
        return Ok(mode & 0o111 != 0);
    }

    // 检查所有者执行权限
    if uid == meta.uid {
        return Ok(mode & 0o100 != 0);
    }

    // 检查组执行权限
    if gid == meta.gid {
        return Ok(mode & 0o010 != 0);
    }

    // 检查其他用户执行权限
    Ok(mode & 0o001 != 0)
}
```

在信号发送中，kill 系统调用检查发送者是否有权限向目标进程发送信号。检查规则为：发送者 EUID 必须为 0（root），或发送者 EUID 等于接收者 RUID 或 EUID。

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
    /// 作业控制状态，供 wait 系统调用查询
    pub wait_status: SpinNoIrq<Option<ProcessWaitStatus>>,
    /// 唤醒等待继续的线程
    pub stop_event: Arc<PollSet>,
    // ...
}
```

当进程收到 SIGSTOP、SIGTSTP、SIGTTIN、SIGTTOU 等停止信号时，信号处理逻辑（位于 `kernel/src/task/signal.rs`）执行以下操作：

```rust
SignalOSAction::Stop => {
    // 记录停止状态
    *thr.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Stopped(signo));
    // 唤醒父进程（可能正在 wait）
    wake_waiting_parent(&thr.proc_data);
    // 阻塞当前线程，直到收到 SIGCONT
    wait_for_continue(&thr.proc_data);
}
```

wait_for_continue 函数通过异步轮询实现阻塞等待：

```rust
fn wait_for_continue(proc_data: &ProcessData) {
    block_on(poll_fn(|cx| {
        let stopped = matches!(
            *proc_data.wait_status.lock(),
            Some(ProcessWaitStatus::Stopped(_))
        );
        if !stopped || proc_data.proc.is_zombie() {
            Poll::Ready(())
        } else {
            proc_data.stop_event.register(cx.waker());
            Poll::Pending
        }
    }));
}
```

当进程收到 SIGCONT 信号时，设置状态为 Continued 并唤醒被阻塞的线程：

```rust
SignalOSAction::Continue => {
    *thr.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Continued(signo));
    wake_waiting_parent(&thr.proc_data);
    // stop_event.wake() 在其他地方调用
}
```


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

wait 系统调用需要检查所有子进程，判断是否有满足条件的状态变化。关键实现位于 `kernel/src/syscall/task/wait.rs`：

```rust
fn wait_status_siginfo(child: &Process, options: WaitOptions)
    -> AxResult<Option<siginfo_t>>
{
    let Ok(proc_data) = get_process_data(child.pid()) else {
        return Ok(None);
    };

    let mut status = proc_data.wait_status.lock();
    let Some(wait_status) = *status else {
        return Ok(None);
    };

    match wait_status {
        // 检查是否报告停止状态
        ProcessWaitStatus::Stopped(signo) if options.contains(WaitOptions::WUNTRACED) => {
            // 非破坏性查询：不清除状态
            if !options.contains(WaitOptions::WNOWAIT) {
                *status = None;
                proc_data.stop_event.wake();
            }
            Ok(Some(make_sigchld_info(child.pid(), signo as i32, CLD_STOPPED)))
        }
        // 检查是否报告继续状态
        ProcessWaitStatus::Continued(signo) if options.contains(WaitOptions::WCONTINUED) => {
            if !options.contains(WaitOptions::WNOWAIT) {
                *status = None;
            }
            Ok(Some(make_sigchld_info(child.pid(), signo as i32, CLD_CONTINUED)))
        }
        _ => Ok(None),
    }
}
```

waitid 系统调用构造 siginfo_t 结构返回详细状态，包括退出码、终止信号等信息：

```rust
fn make_exit_siginfo(child: &Process) -> siginfo_t {
    let wait_status = child.exit_code();
    let term_sig = wait_status & 0x7f;

    let (status, code) = if term_sig == 0 {
        (wait_status >> 8, CLD_EXITED)      // 正常退出
    } else if wait_status & 0x80 != 0 {
        (term_sig, CLD_DUMPED)              // 信号终止且 core dump
    } else {
        (term_sig, CLD_KILLED)              // 信号终止
    };

    make_sigchld_info(child.pid(), status, code)
}
```

通过这些增强，moss 支持了 shell 的作业控制功能，LTP 的 waitpid05-13、waitid01-02 等测试用例得以通过。

=== 进程组与会话管理

moss 实现了进程组和会话管理，支持 setpgid、getpgid、setsid 等系统调用。进程组用于批量信号发送，会话用于终端管理。

setpgid 系统调用将进程移动到指定进程组，实现位于 `kernel/src/syscall/task/job.rs`：

```rust
pub fn sys_setpgid(pid: u32, pgid: u32) -> AxResult<isize> {
    let curr = current();
    let curr_pid = curr.as_thread().proc_data.proc.pid();

    // pid=0 表示当前进程
    let target_pid = if pid == 0 { curr_pid } else { pid };

    // pgid=0 表示使用 target_pid 作为 pgid
    let new_pgid = if pgid == 0 { target_pid } else { pgid };

    // 获取目标进程
    let target_proc = if target_pid == curr_pid {
        curr.as_thread().proc_data.proc.clone()
    } else {
        get_process_data(target_pid)?.proc.clone()
    };

    // 设置进程组
    target_proc.group().set_pgid(new_pgid);
    Ok(0)
}
```


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

pub struct ProcessData {
    // ... 其他字段 ...
    namespaces: SpinNoIrq<ProcessNamespaces>,
    // ...
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

clone 系统调用根据 CLONE_NEWPID、CLONE_NEWUTS、CLONE_NEWUSER 标志创建新命名空间。命名空间继承逻辑实现如下：

```rust
pub fn inherit_namespaces_from(
    &self,
    parent: &ProcessData,
    new_pid: bool,
    new_user: bool,
    new_uts: bool,
) {
    let mut namespaces = parent.namespaces();

    if new_pid {
        // 创建新 PID namespace，当前进程成为 init 进程
        namespaces.pid = NamespaceState::child_of(
            namespaces.pid,
            Some(self.proc.pid())
        );
    }
    if new_user {
        namespaces.user = NamespaceState::child_of(namespaces.user, None);
    }
    if new_uts {
        namespaces.uts = NamespaceState::child_of(namespaces.uts, None);
    }

    *self.namespaces.lock() = namespaces;
}
```

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

命名空间 inode 用于实现 `/proc/self/ns/*` 接口，每个命名空间文件的 inode 编号对应 NamespaceState.inode，供用户空间程序查询和比较命名空间。

                                     == 进程创建与执行

                                     === clone 系统调用完善

                                     clone 是 Linux 统一的进程/线程创建接口，通过 CloneFlags 控制资源共享策略。moss 支持的主要标志如下表所示。

                                     #[
                                       #set text(size: 9pt)
                                       #table(
                                         columns: (1.5fr, 3.5fr),
                                         align: (left, left),
                                         inset: 5pt,
                                         table.header(
                                           [CloneFlags],
                                           [含义],
                                         ),
                                         [CLONE_VM],
                                         [共享地址空间（线程）；否则 COW 复制（进程）],
                                         [CLONE_FILES],
                                         [共享文件描述符表；否则复制 FD 表],
                                         [CLONE_SIGHAND],
                                         [共享信号处理器；否则继承处理器],
                                         [CLONE_THREAD],
                                         [加入同一线程组，共享 PID],
                                         [CLONE_PARENT_SETTID],
                                         [在父进程内存写入子进程 TID],
                                         [CLONE_CHILD_SETTID],
                                         [在子进程内存写入子进程 TID],
                                         [CLONE_CHILD_CLEARTID],
                                         [子进程退出时清零指定地址并唤醒 futex],
                                         [CLONE_PIDFD],
                                         [创建 pidfd 文件描述符],
                                         [CLONE_VFORK],
                                         [父进程阻塞直到子进程 exec 或 exit],
                                       )
                                     ]


                                     clone 系统调用首先验证标志组合的合法性。Linux 要求 CLONE_THREAD 必须同时指定 CLONE_VM 和 CLONE_SIGHAND，因为线程必须共享地址空间和信号处理器；CLONE_SIGHAND 必须指定 CLONE_VM，因为信号处理器位于共享内存中。验证代码位于 `kernel/src/syscall/task/clone.rs`：

                                     ```rust
                                     fn validate(&self) -> AxResult<()> {
                                         let flags = self.flags;

                                         if flags.contains(CloneFlags::THREAD)
                                             && !flags.contains(CloneFlags::VM | CloneFlags::SIGHAND)
                                         {
                                             return Err(AxError::InvalidInput);
                                         }

                                         if flags.contains(CloneFlags::SIGHAND) && !flags.contains(CloneFlags::VM) {
                                             return Err(AxError::InvalidInput);
                                         }

                                         if flags.contains(CloneFlags::VFORK | CloneFlags::THREAD) {
                                             return Err(AxError::InvalidInput);
                                         }

                                         Ok(())
                                     }
                                     ```

                                     根据标志组合，clone 创建不同类型的任务。若包含 CLONE_THREAD，则创建线程，共享 ProcessData；否则创建新进程，fork ProcessData 并复制地址空间：

                                     ```rust
                                     let new_proc_data = if flags.contains(CloneFlags::THREAD) {
                                         // 线程：共享 ProcessData
                                         new_task.ctx_mut().set_page_table_root(old_proc_data.aspace.lock().page_table_root());
                                         old_proc_data.clone()
                                     } else {
                                         // 进程：fork 新 ProcessData
                                         let proc = old_proc_data.proc.fork(tid);
                                         let new_aspace = if flags.contains(CloneFlags::VM) {
                                             old_proc_data.aspace.clone()  // CLONE_VM：共享地址空间
                                         } else {
                                             Arc::new(Mutex::new(old_proc_data.aspace.lock().fork()))  // COW 复制
                                         };
                                         // 创建新 ProcessData...
                                     };
                                     ```

                                     moss 修复了 CLONE_PARENT_SETTID、CHILD_SETTID、CHILD_CLEARTID 的多个 bug（commit f82940ee），确保 TID 写入的时机和地址正确。修复了 fork 时堆指针未继承的问题（commit 759a887e），避免子进程 brk 损坏。实现了 clone3 系统调用（commit 0f15bf48），支持更多扩展参数。

                                     === execve 系统调用增强

                                     execve 系统调用用于加载新程序，替换当前进程的地址空间。moss 在 StarryOS 基础上增强了 execve 的多个方面。

                                     第一，实现 execveat 系统调用（commit aaeea303），支持相对目录文件描述符打开可执行文件，并支持 AT_EMPTY_PATH 和 AT_SYMLINK_NOFOLLOW 标志：

                                     ```rust
                                     pub fn sys_execveat(
                                         uctx: &mut UserContext,
                                         dirfd: c_int,
                                         path: *const c_char,
                                         argv: *const *const c_char,
                                         envp: *const *const c_char,
                                         flags: u32,
                                     ) -> AxResult<isize> {
                                         let path = vm_load_string(path)?;
                                         let exec_loc = if path.is_empty() && (flags & AT_EMPTY_PATH != 0) {
                                             // AT_EMPTY_PATH：dirfd 直接指向可执行文件
                                             let fd_table = FD_TABLE.current();
                                             let file = fd_table.get(dirfd)?.inner.downcast_ref::<File>()?;
                                             Some(file.inner().location().clone())
                                         } else {
                                             // 普通路径解析
                                             resolve_at(dirfd, &path, flags).ok()
                                         };
                                         execve_common(uctx, path, exec_loc, None, argv, envp)
                                     }
                                     ```

                                     第二，实现 shebang 支持（commit c720e652）。当可执行文件以 `#!` 开头时，解析 shebang 行，提取解释器路径，将原文件作为参数传递给解释器：

                                     ```rust
                                     if content.starts_with(b"#!") {
                                         let shebang_line = content.split(|&b| b == b'\n').next()?;
                                         let interpreter = parse_shebang(shebang_line)?;
                                         // 递归执行解释器，原文件路径作为参数
                                         return execve_common(uctx, interpreter, None, Some(&path), argv, envp);
                                     }
                                     ```

                                     第三，修复 execve 允许 NULL argv 和 envp（commit ed0e8be4），符合 POSIX 语义。修复 execve 导致的 brk 损坏（commit 26d53945），在加载新程序时正确重置堆指针。

                                     第四，execve 执行前进行安全检查，包括权限检查、ETXTBSY 检测（文件被写入打开时禁止执行）、fanotify 执行通知等。


                                     == 调试与控制接口

                                     === ptrace 调试支持

                                     ptrace 是 Linux 调试器的基础，允许一个进程观察和控制另一个进程的执行。moss 实现了 sys_ptrace 系统调用（commit 09d4c46c），支持多种 ptrace 操作。

                                     #[
                                       #set text(size: 9pt)
                                       #table(
                                         columns: (1.8fr, 3.2fr),
                                         align: (left, left),
                                         inset: 5pt,
                                         table.header(
                                           [ptrace 操作],
                                           [功能],
                                         ),
                                         [PTRACE_TRACEME],
                                         [标记当前进程为被跟踪状态，父进程成为调试器],
                                         [PTRACE_ATTACH],
                                         [附加到目标进程，开始跟踪],
                                         [PTRACE_DETACH],
                                         [停止跟踪目标进程],
                                         [PTRACE_CONT],
                                         [继续执行被停止的进程],
                                         [PTRACE_SYSCALL],
                                         [继续执行，在下一个系统调用入口或出口停止],
                                         [PTRACE_SINGLESTEP],
                                         [单步执行一条指令],
                                         [PTRACE_GETREGS],
                                         [读取目标进程的寄存器],
                                         [PTRACE_SETREGS],
                                         [设置目标进程的寄存器],
                                         [PTRACE_PEEKDATA],
                                         [读取目标进程内存],
                                         [PTRACE_POKEDATA],
                                         [写入目标进程内存],
                                       )
                                     ]

                                     ptrace 实现维护了一个全局的跟踪状态表，记录每个进程的调试状态和等待事件。当被跟踪进程执行系统调用、收到信号或触发断点时，会停止执行并通知调试器：

                                     ```rust
                                     pub fn ptrace_wait_if_stopped(thr: &Thread) {
                                         let pid = thr.proc_data.proc.pid();
                                         if !ptrace_is_traced(pid) {
                                             return;
                                         }

                                         // 检查是否有待处理的停止事件
                                         while ptrace_should_stop(pid) {
                                             // 阻塞等待调试器发送 PTRACE_CONT 或 PTRACE_SYSCALL
                                             block_on(ptrace_wait_continue(pid));
                                         }
                                     }
                                     ```

                                     ptrace 修复了多个等待事件处理的 bug（commit 71e46a7c、faa52684），确保调试器能正确获取进程状态。通过 ptrace 实现，moss 支持了 gdb、strace 等调试工具。

                                     === prctl 进程控制

                                     prctl 系统调用提供了进程属性的细粒度控制。moss 实现了多个常用的 prctl 选项（commit 6b393c38、4669f0fa）：

                                     ```rust
                                     pub fn sys_prctl(option: i32, arg2: usize, arg3: usize, arg4: usize, arg5: usize)
                                         -> AxResult<isize>
                                     {
                                         match option as u32 {
                                             PR_SET_NAME => {
                                                 // 设置进程名称（最多 16 字节）
                                                 let name = vm_load_until_nul(arg2 as *const u8, 16)?;
                                                 current().set_name(name);
                                                 Ok(0)
                                             }
                                             PR_GET_NAME => {
                                                 // 获取进程名称
                                                 let name = current().name();
                                                 let buf = arg2 as *mut u8;
                                                 buf.vm_write_slice(name.as_bytes())?;
                                                 Ok(0)
                                             }
                                             PR_SET_PDEATHSIG => {
                                                 // 设置父进程死亡时发送的信号
                                                 // 实现省略
                                                 Ok(0)
                                             }
                                             PR_CAPBSET_READ => {
                                                 // 查询 capability bounding set
                                                 let cap = arg2 as u32;
                                                 Ok(if current_has_cap(cap) { 1 } else { 0 })
                                             }
                                             _ => {
                                                 warn!("Unsupported prctl option: {}", option);
                                                 Ok(0)  // 返回成功，避免程序崩溃
                                             }
                                         }
                                     }
                                     ```

                                     === pidfd 文件描述符机制

                                     pidfd 将进程表示为文件描述符，提供了更安全的进程操作方式。传统的 PID 可能被回收重用，而 pidfd 在进程存活期间保持有效。

                                     moss 实现了 pidfd_open 和 pidfd_send_signal（commit 71c73e0d）：

                                     ```rust
                                     pub fn sys_pidfd_open(pid: u32, flags: u32) -> AxResult<isize> {
                                         // 获取目标进程
                                         let proc_data = get_process_data(pid)?;

                                         // 创建 PidFd 对象
                                         let pidfd = PidFd::new(pid, proc_data.exit_event.clone());

                                         // 加入文件描述符表
                                         let fd_table = FD_TABLE.current();
                                         let fd = fd_table.insert(Arc::new(pidfd))?;

                                         Ok(fd as isize)
                                     }

                                     pub fn sys_pidfd_send_signal(pidfd: i32, sig: u32, info: *mut siginfo_t, flags: u32)
                                         -> AxResult<isize>
                                     {
                                         let fd_table = FD_TABLE.current();
                                         let pidfd_obj = fd_table.get(pidfd)?.inner.downcast_ref::<PidFd>()?;

                                         let pid = pidfd_obj.pid();
                                         let signo = Signo::from_repr(sig as u8).ok_or(AxError::InvalidInput)?;

                                         // 发送信号到目标进程
                                         send_signal_to_process(pid, SignalInfo::new_kernel(signo))?;

                                         Ok(0)
                                     }
                                     ```

                                     clone 系统调用支持 CLONE_PIDFD 标志，自动创建 pidfd 并写回用户空间指针。pidfd 可以通过 epoll 监听进程退出事件（commit 46a389f3），为异步进程管理提供了便利。

                                     moss 的进程与线程管理从 StarryOS 的基础框架出发，通过补齐权限管理、作业控制、命名空间、调试接口等关键能力，显著提升了 Linux 兼容性。这些改进使得 moss 能够运行更多真实世界的 Linux 程序，通过了大量 LTP 测试用例，为后续系统功能扩展奠定了坚实基础。

