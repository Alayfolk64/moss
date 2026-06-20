#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(2)

= 进程与线程管理

进程与线程管理负责组织用户程序的执行主体及其资源，向内连接地址空间、文件描述符和信号等内核对象，向外提供创建、执行、退出、等待和控制接口。

== 概述

StarryOS 已经建立了可运行的进程线程框架。每个用户执行流对应一个 `Thread`，同一进程中的线程共同持有 `ProcessData`。前者保存线程私有状态，后者保存地址空间、文件资源、进程级信号和父子关系等共享状态。在功能上，StarryOS 已经提供了 `fork`、`clone`、`clone3`、`execve`、`exit` 和 `waitpid`，能够完成进程创建、程序映像替换、线程退出和僵尸进程回收，进程组、会话和 pidfd 也已有基本实现。

然而，基线只处理了"创建子进程、运行、退出、父进程回收"这条主路径，接口组合复杂后问题便逐渐暴露：clone 的标志约束和失败回滚不够严密，exec 缺少 `execveat` 及执行权限检查，wait 无法完整表达停止和继续状态，UID/GID 部分接口直接返回成功而没有保存身份变化，ptrace 和 pidfd 的观测链路也尚未打通。换句话说，基线能建立进程，却难以把进程的身份、状态和生命周期完全串联起来。

moss 保留了 StarryOS 的主体结构，重点补齐这些接口之间的联系。主要增量如表所示：

#figure(
  table(
    columns: (0.9fr, 2.3fr, 2.65fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([内容], [StarryOS 基线], [moss 的增量]),
    [对象与状态], [`Thread` 与 `ProcessData` 分离], [补充凭据、停止/继续状态、子进程时间和 namespace 元数据],
    [创建与执行], [`clone3`、`execve` 已实现], [补严 clone ABI 和失败回滚，增加 `execveat` 与执行约束],
    [退出与等待], [线程退出、僵尸进程和 `waitpid`], [完善资源清理，增加 `waitid`、状态事件和 pidfd 等待],
    [管理接口], [进程组、会话、基础凭据与 pidfd], [修正全局登记，实现有状态凭据和最小 ptrace 控制链路],
  ),
  caption: [进程与线程模块的主要增量],
) <tab:process-overview>

== 核心设计思想

StarryOS 已经搭起了进程线程管理的基本框架，moss 的工作是在这套框架中进一步理清状态归属和对象关系：哪些数据属于单个线程，哪些资源应由整个进程共享；进程之间如何组织；身份信息怎样参与权限判断；一个任务又该如何从创建走到退出和回收。下面从这几个问题分别说明。

=== 双层对象模型

进程和线程并不是两套互不相干的对象。moss 延续 StarryOS 的做法，将状态分为两层：`Thread` 表示单个执行流，`ProcessData` 表示线程共同拥有的进程资源。创建普通线程时复用 `ProcessData`；创建新进程时再根据 clone 标志复制或共享其中的资源。

这种划分回答了一个最基本的问题：某项状态究竟属于线程，还是属于整个进程。线程名称、线程信号状态和退出字段放在 `Thread` 中；地址空间、程序路径、文件表作用域、凭据和子进程集合放在 `ProcessData` 中。只有边界清楚，clone、exec 和 exit 才不会各自维护一份互相冲突的状态。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-2.2.1.svg", width: 85%)
    ]
  ],
  caption: [Thread 与 ProcessData 的双层对象模型],
) <fig:thread-process-model>

=== 三类组织关系

moss 同时维护三类关系。

- 归属关系：多个线程通过共享 `ProcessData` 归属于同一进程。
- 亲缘关系：进程通过 parent 和 children 形成父子树，供退出托管和 wait 使用。
- 分组关系：进程依次归入进程组和会话，形成 PID、PGID、SID 三层标识。

这三类关系用途不同。线程组解决"哪些执行流共享资源"，父子树解决"谁负责回收谁"，进程组和会话解决"哪些进程作为一个作业被统一管理"。moss 没有把它们压进同一张表，而是分别维护，再通过全局索引提供查询。

=== 三元凭据

moss 的进程身份设计参考了 Linux 的凭据模型：UID 不只保存一个值，而是拆成 real、effective、saved 三个字段，GID 采用相同结构。real ID 表示身份来源，effective ID 用于当前权限判断，saved ID 允许进程在受限范围内恢复先前身份。moss 将 UID/GID 三元组放入 `ProcessData`，让凭据真正参与程序执行、信号发送和 ptrace 权限检查。具体实现见 @sec:uid-gid。

=== 生命周期闭环

进程管理不是若干独立系统调用的集合，而是一条连续的生命周期：

#align(center)[
  #box(
    inset: 8pt,
    stroke: 0.6pt + gray,
    radius: 3pt,
  )[
    创建 `clone` → 执行 `exec` → 退出 `exit` → 观察与回收 `wait`
  ]
]

创建阶段建立身份、资源和父子关系；exec 替换程序映像，但保留 PID、父子关系和进程组；exit 释放运行资源并留下退出状态；wait 读取状态并完成最终回收。moss 的多项修改都围绕这条闭环展开，尤其重视失败路径和中间状态，而不仅是系统调用成功时的主路径。

== 关键数据结构

=== Thread 与 ProcessData

`Thread` 和 `ProcessData` 是这部分最重要的两类数据，下面是关键代码片段。

#figure(
  [
    ```rust
    pub struct Thread {
        // 共享进程级资源
        pub proc_data: Arc<ProcessData>,
        // 线程私有状态
        pub signal: Arc<ThreadSignalManager>,
        pub exit: Arc<AtomicBool>,
    }

    pub struct ProcessData {
        // 进程身份及组织关系
        pub proc: Arc<Process>,
        // 线程共享的进程资源
        pub aspace: Arc<Mutex<AddrSpace>>,
        pub scope: RwLock<Scope>,
        pub wait_status: SpinNoIrq<Option<ProcessWaitStatus>>,
        // UID/GID 三元组
        uid: AtomicU32, euid: AtomicU32, suid: AtomicU32,
        gid: AtomicU32, egid: AtomicU32, sgid: AtomicU32,
    }
    ```
  ],
  caption: [线程私有状态与进程共享状态],
) <lst:thread-process-data>

同一进程中的线程持有同一个 `Arc<ProcessData>`，因此能看到相同的地址空间、文件资源和凭据。`Thread` 自身则保存只与当前执行流相关的数据。moss 新增字段时继续遵循这一边界，没有另建一套进程控制块。

=== 进程状态

StarryOS 原有退出码可以描述进程终止，但并不适合表示暂时停止或继续运行。moss 增加了 `ProcessWaitStatus`：

#figure(
  [
    ```rust
    pub enum ProcessWaitStatus {
        // 被信号暂停，但进程仍然存在
        Stopped(Signo),
        // 从停止状态恢复
        Continued(Signo),
    }
    ```
  ],
  caption: [进程停止与继续状态],
) <lst:process-wait-status>

退出状态仍由 `Process` 保存；停止和继续状态则独立记录在 `ProcessData` 中。@fig:process-state 展示了进程在四个状态之间的完整流转关系：运行中与已停止之间通过 SIGSTOP/SIGCONT 互相切换，exit 触发进入僵尸状态，父进程调用 wait 后完成最终回收。父进程通过 wait 所能感知的三类事件（Stopped、Continued、退出）都对应图中的状态转换。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-2-进程状态机.svg", width: 60%)
    ]
  ],
  caption: [进程生命周期状态机],
) <fig:process-state>

=== 子进程等待语义

wait 系列调用的核心问题是：父进程怎样知道子进程发生了什么，以及知道之后是否立即消费这条状态。这个问题比表面上看起来复杂——子进程可能停止、恢复，也可能退出，每种情况的处理语义完全不同。

以 Shell 的作业控制为例。用户按下 Ctrl+Z，前台进程组收到 SIGSTOP 后暂停；此后执行 fg 或 bg 时，Shell 需要知道这些进程是"暂停等待继续"而不是"已经退出"。如果 wait 只能观测退出状态，Shell 就无法区分两者，作业控制便无从实现。WUNTRACED 正为此而来——它让父进程感知子进程的停止状态，而不把子进程当作已终止。WCONTINUED 处理对称的情况：SIGCONT 让暂停进程恢复运行，这次状态变化同样需要一个观测窗口才能被父进程感知。父进程可以等待指定 PID、某个进程组或任意直接子进程，没有符合条件的直接子进程时返回 `ECHILD`。

#figure(
  table(
    columns: (1.5fr, 4.5fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([选项], [作用]),
    [`WNOHANG`], [当前没有可报告状态时立即返回，不阻塞父进程],
    [`WUNTRACED`], [允许报告子进程停止状态],
    [`WCONTINUED`], [允许报告子进程继续运行状态],
    [`WNOWAIT`], [读取状态但暂不消费，后续仍可再次查询],
  ),
  caption: [子进程等待的主要选项],
) <tab:child-wait-options>

如 @fig:wait-timeline 所示，停止、继续和退出都可以成为一次 `wait` 的报告。前两者只表示子进程的运行状态发生了变化：父进程读取报告后，子进程仍然存在，可以继续运行或再次被停止。退出状态则是最后一次报告；父进程按通常方式读取后，内核才回收这个僵尸进程。`WNOHANG` 用于当前没有可报告状态时立即返回，`WNOWAIT` 则允许父进程先查看结果、暂不消费，之后仍可再次读取。

传统的 `waitpid` 和 `wait4` 返回的是整数状态，调用者需要自己去解析，而 moss 新增的 `waitid` 使用 `siginfo_t` 结构化这些信息，能够区分正常退出、信号终止、停止和继续这些状态。moss 将这些入口汇入同一套目标筛选和状态消费逻辑：不符合当前选项的状态会保留，等待之后合适的 `wait` 调用读取。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-2-wait语义.svg", width: 92%)
    ]
  ],
  caption: [子进程状态变化与父进程 wait 事件消费的时序关系],
) <fig:wait-timeline>

`waitid` 还可以通过 pidfd 选择目标。pidfd 是指向进程对象的文件描述符，不会因为 PID 数值复用而转向另一个进程。配合 `WNOWAIT`，调用者可以先观察状态，之后再完成回收。父进程在暂时没有可报告状态时如何进入阻塞，以及状态变化怎样唤醒父进程，将在第三章结合通用等待机制说明。

=== 四张全局索引表

内核需要按 TID、PID、PGID 和 SID 查找对象。moss 使用四张弱引用表分别索引线程、进程、进程组和会话：

#figure(
  table(
    columns: (1.45fr, 1.3fr, 2.8fr),
    inset: 5pt,
    stroke: 0.5pt + gray,
    table.header([索引表], [键], [作用]),
    [`TASK_TABLE`], [TID], [查找具体线程和线程私有状态],
    [`PROCESS_TABLE`], [PID], [查找进程共享资源和父子关系],
    [`PROCESS_GROUP_TABLE`], [PGID], [查找进程组并执行组级操作],
    [`SESSION_TABLE`], [SID], [查找会话及其进程组层级],
  ),
  caption: [进程线程对象的全局索引],
) <tab:process-index>

表中保存弱引用，不会因为全局索引而阻止对象释放。moss 还修正了新建进程组和会话后的登记过程，避免对象已经创建，却无法通过 PGID 或 SID 查到。

== 进程与线程创建

=== fork 与统一的 clone 路径

`fork` 在 moss 中并未缺席。在 x86_64 平台上，`sys_fork` 会调用 `sys_clone`，其中不设置资源共享标志，只指定子进程退出时发送 `SIGCHLD`。因此，`fork` 创建的是拥有独立 `ProcessData` 的子进程；`clone` 和 `clone3` 则允许用户通过标志决定创建进程还是线程，以及资源如何继承。

这些入口最终都把参数整理为 `CloneArgs`，交给 `CloneArgs::do_clone` 执行。带有 `CLONE_THREAD` 时，新任务复用当前 `ProcessData`；否则调用 `Process::fork` 建立子进程及父子关系，再创建新的 `ProcessData`。地址空间、文件表、文件系统上下文和信号处理动作分别由 `CLONE_VM`、`CLONE_FILES`、`CLONE_FS` 和 `CLONE_SIGHAND` 决定共享或复制。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-2.4.1.svg", width: 50%)
    ]
  ],
  caption: [fork、clone 与 clone3 的统一创建路径],
) <fig:clone-path>

#figure(
  table(
    columns: (1.5fr, 4.5fr),
    inset: 5pt,
    stroke: 0.5pt + gray,
    table.header([标志], [作用]),
    [`CLONE_THREAD`], [加入调用者的线程组，共享同一个 `ProcessData`],
    [`CLONE_VM`], [共享虚拟地址空间],
    [`CLONE_FILES`], [共享文件描述符表],
    [`CLONE_FS`], [共享文件系统上下文],
    [`CLONE_SIGHAND`], [共享信号处理动作表],
  ),
  caption: [进程线程创建中的主要 clone 标志],
) <tab:clone-flags>

这些标志可以组合使用，但并非任意组合都合法。`CLONE_THREAD` 决定新任务是否仍属于原进程，其余标志主要控制资源如何继承，或指定创建前后的附加操作。

=== 参数约束

moss 补充了 clone 标志之间的依赖关系，让不合法的参数组合直接报错，而不是创建一个运行后才暴露问题的异常线程/进程，把 bug 扼杀在摇篮里。例如，线程必须共享地址空间和信号处理动作，共享信号处理动作又必须共享地址空间：

#figure(
  [
    ```rust
    // 线程必须共享地址空间和信号处理动作
    if flags.contains(CloneFlags::THREAD)
        && !flags.contains(CloneFlags::VM | CloneFlags::SIGHAND)
    {
        return Err(AxError::InvalidInput);
    }
    // 共享信号处理动作必须共享地址空间
    if flags.contains(CloneFlags::SIGHAND)
        && !flags.contains(CloneFlags::VM)
    {
        return Err(AxError::InvalidInput);
    }
    ```
  ],
  caption: [clone 标志组合校验],
) <lst:clone-validate>

对于 `clone3`，moss 进一步检查结构体最小长度、栈地址与栈大小的配对关系，以及超出当前结构体版本的尾部字节。额外字节必须可读且全部为零，使旧内核能够安全接收新版本用户态传入的结构。

=== 创建失败回滚

这里处理的情况和 2.3.3 的不同。2.3.3 讨论的是子进程已经运行后，父进程怎样通过 `wait` 读取它的状态；如果子进程还没有开始运行，`clone` 就已经失败，那么系统调用会向调用者返回错误，但内核此前可能已登记 PID、父子关系和线程记录，必须把这些半成品全部撤销。

`do_clone` 会先建立进程对象和父子关系，分配 `ProcessData`，再创建线程记录并加入全局索引。若调用者请求 `CLONE_PIDFD`，内核还要分配文件描述符，并将其编号写回用户地址。只有这些步骤都完成后，新任务才会提交给调度器正式运行，如@fig:clone-rollback 所示。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-2-4-3.svg", width: 82%)
    ]
  ],
  caption: [do_clone 执行步骤与失败回滚路径],
) <fig:clone-rollback>

pidfd 分配时文件描述符表可能已满，向用户地址写回 pidfd 编号时也可能遇到无效指针。这些错误发生时，进程对象已经登记进 `PROCESS_TABLE`，线程记录已经加入 `TASK_TABLE`，但新任务尚未运行，因此不会自行调用 `exit()` 进入僵尸状态。若直接返回错误，父进程会在 `waitpid(-1, ...)` 中等不到这个子进程的退出通知，PID 也会被永久占用；`PROCESS_TABLE` 中的条目还会因父进程 `children` 表的强引用而无法自动释放。

moss 因此在任务提交调度器之前统一处理失败路径：释放已分配的 pidfd，从全局索引移除线程记录；若新进程没有其他存活线程，再一并释放进程对象。这样 `do_clone` 返回错误时，内核状态与调用前一致，调用者看到的“创建失败”才真正表示这次创建没有留下任何痕迹。

== 程序执行

=== execve 基线

StarryOS 的 `execve` 已经能够装载 ELF 文件，替换地址空间和用户上下文，更新程序路径与命令行，重置信号处理动作，并关闭带有 `CLOEXEC` 标志的文件描述符。exec 不创建新进程，因此 PID、父子关系、进程组和会话保持不变。基线的不足在于只能按路径执行文件，对执行权限、`MS_NOEXEC` 挂载点和正在写入的可执行文件缺少检查，也没有 `execveat`。

=== execveat 与执行检查

传统 `execve` 只能接收路径，绝对路径依赖进程看到的文件系统，相对路径又依赖当前工作目录。程序若已经打开了可信目录或目标文件，仍需重新拼接并解析路径，期间目录项可能被改名或替换。`execveat` 可以从稳定的目录文件描述符开始解析，或使用 `AT_EMPTY_PATH` 执行已经打开的文件，减少重复路径解析带来的竞态，也便于容器运行时、动态加载器等程序在受控目录中启动目标。

moss 增加该入口后，将 `execve` 与 `execveat` 汇入同一条 `execve_common` 路径，后续装载和状态更新仍由一套逻辑完成。在替换旧地址空间之前，moss 先完成执行条件检查：

#figure(
  [
    ```rust
    // 挂载点禁止执行
    if exec_loc.mount_flags() & MS_NOEXEC != 0 {
        return Err(AxError::PermissionDenied);
    }
    // 按有效 UID/GID 检查执行位
    if !has_exec_permission(exec_loc, proc_data.euid(), proc_data.egid())? {
        return Err(AxError::PermissionDenied);
    }
    // 拒绝正在被写入的可执行文件
    if is_text_busy(exec_loc) {
        return Err(AxError::from(LinuxError::ETXTBSY));
    }
    ```
  ],
  caption: [程序执行前的约束检查],
) <lst:exec-check>

这些检查全部通过后，内核才装载新程序并提交地址空间。若解析、权限或装载失败，原程序仍可继续运行。成功后，内核还会清除旧程序遗留的线程退出指针，避免新地址空间继续引用失效地址。

== 退出与资源回收

=== 单个线程退出与进程结束

一个进程可以包含多个线程，因此某个线程退出并不等于整个进程结束。内核先执行线程级退出清理，再把它从进程的线程集合中移除。清理内容包括 robust futex 和 `clear_child_tid`，其唤醒语义放在第三章的 futex 退出路径中说明。如果还有其他线程存活，共享的地址空间、文件表和 `ProcessData` 都继续保留。

只有最后一个线程退出时，内核才结束整个进程。此时进程进入僵尸状态，保存退出码，向父进程发送退出信号并唤醒正在等待的父进程。moss 还在这一阶段释放文件锁、租约、共享内存记录和 ptrace 关系。进程对象不会立即删除，要等父进程调用 `wait` 读取退出状态后才完成回收。

=== exit_group：结束全部线程

普通 `exit` 只结束调用线程，`exit_group` 则需要主动让进程中的全部线程退出。moss 先标记线程组正在整体退出，再向其他存活线程发送终止信号。各线程仍按上一节的路径完成清理，最后离开的线程负责执行进程级退出工作。这一区别对多线程程序尤其重要。

=== 退出后的孤儿进程托管

进程结束后，还要处理它尚未退出的子进程。子进程不能随父进程一起删除——它们之后退出时需要有进程负责读取退出状态，否则将永远停留在僵尸状态占用内核资源。moss 沿用 StarryOS 的做法，把这些子进程从原父进程的 `children` 表中移出，改由 init 进程托管。如 @fig:orphan-reparent 所示，整个过程只修改父进程指针，子进程本身的 PID、PGID 和 SID 均保持不变，仍可继续运行。它们以后成为僵尸进程时，由 init 调用 `wait` 完成回收。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-2-孤儿托管.svg", width: 95%)
    ]
  ],
  caption: [孤儿进程托管示意图],
) <fig:orphan-reparent>

== 进程组与会话

=== PID、PGID 与 SID

进程组和会话用于组织多个相关进程。每个进程有 PID，所属进程组有 PGID，进程组所属会话有 SID。通常，组长的 PID 等于 PGID，会话首进程的 PID 等于 SID。这种层级常见于 Shell：一条管道中的多个进程属于同一进程组，一个终端登录环境中的多个进程组属于同一会话。它们仍是独立进程，但可以按组查询、等待或发送信号。

在实现上，moss 沿用 StarryOS 的对象关系：`Process` 保存自身 PID 和所属的 `ProcessGroup`，`ProcessGroup` 保存 PGID、所属 `Session` 及组内进程，`Session` 再保存 SID 和会话内的进程组。

#figure(
  [
    ```rust
    pub struct Process {
        pid: Pid,
        // 每个进程只属于一个进程组
        group: SpinNoIrq<Arc<ProcessGroup>>,
    }

    pub struct ProcessGroup {
        pgid: Pid,
        // 进程组归属于一个会话
        session: Arc<Session>,
        // 记录组内的所有进程
        processes: SpinNoIrq<WeakMap<Pid, Weak<Process>>>,
    }

    pub struct Session {
        sid: Pid,
        // 一个会话可以包含多个进程组
        process_groups: SpinNoIrq<WeakMap<Pid, Weak<ProcessGroup>>>,
    }
    ```
  ],
  caption: [进程、进程组与会话的对象关系],
) <lst:process-group-session>

查询一个进程的三个标识时，内核沿对象引用逐层取得结果：先按 PID 找到进程对象，再沿所属关系取得进程组和会话标识。反向操作则依靠成员表完成——向进程组发送信号时，内核遍历 `ProcessGroup::processes`，逐个找到组内进程。

=== 进程组创建与迁移

StarryOS 已提供 `setpgid`、`getpgid` 等基础接口，但新建的进程组没有同步写入全局 PGID 索引，导致其他系统调用可能无法通过 PGID 找到该进程组。

moss 根据目标 PGID 分两种情况处理。目标 PGID 等于进程自身 PID 时，内核创建新进程组并立即登记到全局索引，此后组信号和按进程组等待等操作都能查到该对象。目标 PGID 指向已有进程组时，内核先从全局索引中找到目标组，检查它与当前进程是否属于同一会话，通过后将进程从旧组迁移到新组，同时更新全局索引。若目标组不存在或属于其他会话，则拒绝迁移。这样修改后，进程所属组、成员表和全局索引始终保持一致。

=== 会话创建

`setsid` 会同时创建新会话和新进程组。moss 在创建成功后分别登记 `Session` 和 `ProcessGroup`：

#figure(
  [
    ```rust
    if let Some((session, group)) = proc.create_session() {
        // setsid 同时创建会话和进程组
        add_session_to_table(&session);
        add_process_group_to_table(&group);
        // SID 等于调用进程的 PID
        Ok(session.sid() as _)
    }
    ```
  ],
  caption: [新会话与新进程组的创建和登记],
) <lst:setsid-register>

== 权限与进程控制

=== UID/GID 三元组 <sec:uid-gid>

进程的身份不能只用一个 UID 表示。系统既要知道它原本由谁启动，也要知道它此刻按谁的权限访问文件、执行程序或向其他进程发送信号。为此，moss 沿用 Linux 的三元凭据：real UID 记录原始用户，effective UID 用于当前权限检查，saved UID 保存此前可恢复的有效身份；GID 采用同样的结构。

这套设计用于受控的权限切换。普通用户运行带有 set-user-ID 属性的程序时，程序可以暂时以文件所有者的权限完成受保护操作，随后将 effective UID 降回普通用户。若后续仍需该权限，程序可从 saved UID 恢复，而不必始终以高权限运行。

StarryOS 基线中的部分身份设置接口只返回成功，并未保存身份变化；调用者看似完成了切换，后续权限检查却仍在使用旧身份。moss 在 `ProcessData` 中加入完整的六个 UID/GID 字段，并在 `setresuid`、`setresgid` 等接口中检查允许的转换范围：

#figure(
  [
    ```rust
    let allowed = [old_ruid, old_euid, old_suid];
    if old_euid != 0 {
        // 非特权进程不能把三元组改成从未拥有过的身份
        if !valid_id_change(ruid, &allowed)
            || !valid_id_change(euid, &allowed)
            || !valid_id_change(suid, &allowed)
        {
            return Err(eperm());
        }
    }
    // 分别更新三个字段，UID_GID_NO_CHANGE 表示不修改
    if ruid != UID_GID_NO_CHANGE { proc_data.set_real_uid(ruid); }
    if euid != UID_GID_NO_CHANGE { proc_data.set_euid(euid); }
    if suid != UID_GID_NO_CHANGE { proc_data.set_suid(suid); }
    ```
  ],
  caption: [UID 三元组的保存与受限转换],
) <lst:uid-triplet>

普通进程不能凭空把自己改成另一个用户，只能在 real、effective 和 saved 三个已有值之间切换；effective UID 为 0 的进程才可以设置新的身份。effective UID/GID 一旦改变，后续权限检查也随之改变：进程可能失去执行受保护文件的权限，不能向其他用户的进程发送 `kill` 信号，或不能通过 ptrace 附加调试目标。凭据由此成为实际的权限依据，而不是查询接口返回的一组固定数字。

capability 则把传统上集中在 root 身上的权限拆成多项能力。moss 分别保存 effective、permitted、inheritable 和 bounding 集合：当前可使用的能力、允许启用的能力、可在执行新程序时传递的能力，以及能力的最高边界。更新这些集合时，内核会检查它们之间的约束，进程不能仅靠一次系统调用把原本没有的能力加入自身权限。

=== ptrace 最小控制链路

moss 新增 ptrace 的基础状态管理，建立 `TRACEME`、`ATTACH`、`DETACH`、停止、继续和 wait 报告之间的关系。attach 时先检查目标是否存在、是否已被跟踪以及调用者是否具有相应权限；目标停止后，跟踪者通过 wait 获取事件。

但是当前实现只能完成最小控制流程。`PTRACE_SYSCALL` 和 `PTRACE_SINGLESTEP` 暂按普通继续处理，寄存器、目标内存和 siginfo 读写请求返回 `EIO`，因此还不能视为完整的 GDB 后端。

== 本章小结

moss 选择沿用 StarryOS 的进程线程框架，围绕四条主线完成加固：以 `Thread` 和 `ProcessData` 组成双层对象模型，以线程归属、父子树和进程组会话维护三类组织关系，以 real、effective、saved ID 构成三元凭据，并把 clone、exec、exit、wait 串成完整生命周期。

在此基础上，moss 补严了 `clone3` 的 ABI 和失败回滚，增加 `execveat`，完善子进程状态、等待接口和退出清理，并加入有状态的权限和 ptrace 控制。不足之处在于当前还未完成完整 vfork、多线程 exec 时仅保留调用线程的收敛、终端作业控制（前台进程组、后台读写限制和孤儿进程组规则）、namespace 隔离和 ptrace 数据访问，但进程从创建到回收的主要管理关系已经比 StarryOS 基线更完整。
