#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(2)

= 进程与线程管理

进程与线程管理负责组织用户程序的执行主体及其资源，向内连接地址空间、文件描述符和信号等内核对象，向外提供创建、执行、退出、等待和控制接口。

== 概述

StarryOS 已经建立了可运行的进程线程框架。每个用户执行流对应一个 `Thread`，同一进程中的线程共同持有 `ProcessData`。前者保存线程私有状态，后者保存地址空间、文件资源、进程级信号和父子关系等共享状态。在功能上，StarryOS 已经提供了 `fork`、`clone`、`clone3`、`execve`、`exit` 和 `waitpid`，能够完成进程创建、程序映像替换、线程退出和僵尸进程回收，进程组、会话和 pidfd 也已有基本实现。

然而，基线只处理了"创建子进程、运行、退出、父进程回收"这条主路径，接口组合复杂后问题便逐渐暴露：clone 的标志约束和失败回滚不够严密，exec 缺少 `execveat` 及执行权限检查，wait 无法完整表达停止和继续状态，UID/GID 部分接口直接返回成功而没有保存身份变化，ptrace 和 pidfd 的观测链路也尚未打通。换句话说，基线能建立进程，却没有把进程的身份、状态和生命周期完全串联起来。

moss 保留 StarryOS 的主体结构，重点补齐这些接口之间的联系。主要增量如表所示。

#figure(
  table(
    columns: (1.35fr, 2.3fr, 2.65fr),
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

StarryOS 已经搭起了进程线程管理的基本框架，moss 的工作是在这套框架中进一步理清状态归属和对象关系：哪些数据属于单个线程，哪些资源应由整个进程共享；进程之间如何组织；身份信息怎样参与权限判断；一个任务又如何从创建走到退出和回收。下面从这几个问题分别说明。

=== 双层对象模型

进程和线程并不是两套互不相干的对象。moss 延续 StarryOS 的做法，将状态分为两层：`Thread` 表示单个执行流，`ProcessData` 表示线程共同拥有的进程资源。创建普通线程时复用 `ProcessData`；创建新进程时再根据 clone 标志复制或共享其中的资源。

这种划分回答了一个最基本的问题：某项状态究竟属于线程，还是属于整个进程。线程名称、线程信号状态和退出字段放在 `Thread` 中；地址空间、程序路径、文件表作用域、凭据和子进程集合放在 `ProcessData` 中。只要边界清楚，clone、exec 和 exit 才不会各自维护一份互相冲突的状态。

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

Linux 不使用单一 UID 表示进程身份，而是维护 real、effective、saved 三个值，GID 采用相同结构。real ID 表示身份来源，effective ID 用于当前权限判断，saved ID 允许进程在受限范围内恢复先前身份。moss 将 UID/GID 三元组放入 `ProcessData`，让凭据真正参与程序执行、信号发送和 ptrace 权限检查。具体实现见 @sec:uid-gid。

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

`Thread` 和 `ProcessData` 是本模块最重要的两类数据。下面的代码省略了与本节无关的字段，只保留二者的关系。

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
        // UID/GID 三元组（GID 结构相同）
        uid: AtomicU32, euid: AtomicU32, suid: AtomicU32,
        gid: AtomicU32, egid: AtomicU32, sgid: AtomicU32,
    }
    ```
  ],
  caption: [线程私有状态与进程共享状态],
) <lst:thread-process-data>

同一进程中的线程持有同一个 `Arc<ProcessData>`，因此能看到相同的地址空间、文件资源和凭据。`Thread` 自身则保存只与当前执行流相关的数据。moss 新增字段时继续遵循这一边界，没有另建一套进程控制块。

=== 进程状态

StarryOS 原有退出码可以描述进程终止，却不适合表示暂时停止或继续运行。moss 增加 `ProcessWaitStatus`：

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

退出状态仍由 `Process` 保存；停止和继续状态则独立记录在 `ProcessData` 中。这样，父进程可以区分"子进程已经结束"和"子进程仍然存在，但状态发生变化"，也便于 `waitpid` 与 `waitid` 按不同选项消费事件。

=== 子进程等待语义

父进程可以等待指定子进程、某个进程组或任意直接子进程。moss 将这些目标转换成统一的内部表示（`Pid`、`Pgid` 和 `Any`），再从当前进程的子进程集合中筛选。没有符合条件的直接子进程时返回 `ECHILD`。

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

传统的 `waitpid` 和 `wait4` 返回整数状态，`waitid` 则使用 `siginfo_t` 区分正常退出、信号终止、停止和继续。moss 将这些入口汇入同一套目标筛选和状态消费逻辑。退出状态被读取后，子进程可以完成回收；停止和继续只消费本次事件，不会结束子进程。未被当前选项选中的状态继续保留，供后续调用读取。

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

moss 补充了 clone 标志之间的依赖关系。例如，线程必须共享地址空间和信号处理动作，共享信号处理动作又必须共享地址空间：

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

创建子进程需要建立进程对象、线程记录、父子关系和 pidfd，最后才把新任务加入系统。若 pidfd 分配或写回用户地址失败，已建立的状态不能继续保留——否则系统中会出现"有 PID、无可运行线程"的半初始化对象，既占用 PID 空间，又无法被 wait 回收。

moss 在任务真正启动前设置回滚路径。发生错误时，内核移除刚加入的线程记录，并在其成为最后一个线程时释放新进程。这样，系统调用返回失败后不会留下残留对象。

当前 `CLONE_VFORK` 将 `CLONE_VM` 标志去掉后退化为普通创建，不暂停父进程等待子进程 exec/exit，与 POSIX 语义有差异，对依赖 vfork 优化写时复制的程序可能有影响。namespace 标志也只完成识别和元数据继承，不建立真正的隔离视图。

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

当前多线程进程调用 exec 时会返回失败，Linux 允许的"exec 后只保留调用线程"的收敛过程尚未实现。

== 退出与资源回收

=== 单个线程退出与进程结束

一个进程可以包含多个线程，因此某个线程退出并不等于整个进程结束。内核先执行线程级退出清理，再把它从进程的线程集合中移除。清理内容包括 robust futex 和 `clear_child_tid`，其唤醒语义放在第三章的 futex 退出路径中说明。如果还有其他线程存活，共享的地址空间、文件表和 `ProcessData` 都继续保留。

只有最后一个线程退出时，内核才结束整个进程。此时进程进入僵尸状态，保存退出码，向父进程发送退出信号并唤醒正在等待的父进程。moss 还在这一阶段释放文件锁、租约、共享内存记录和 ptrace 关系。进程对象不会立即删除，要等父进程调用 `wait` 读取退出状态后才完成回收。

=== exit_group：结束全部线程

普通 `exit` 只结束调用线程，`exit_group` 则需要主动让进程中的全部线程退出。moss 先标记线程组正在整体退出，再向其他存活线程发送终止信号。各线程仍按上一节的路径完成清理，最后离开的线程负责执行进程级退出工作。这一区别对多线程程序尤其重要。

=== 退出后的孤儿进程托管

进程结束后，还要处理它尚未退出的子进程。子进程不能随父进程一起删除——它们之后退出时需要有进程负责读取退出状态，否则将永远停留在僵尸状态占用内核资源。moss 沿用 StarryOS 的做法，把这些子进程从原父进程的 `children` 表中移出，改由 init 进程托管：

#align(center)[
  原父进程 → 子进程 #h(1em) 变为 #h(1em) init → 子进程
]

子进程本身仍继续运行，原有 PID、进程组和会话不变；变化的只是父进程。它们以后成为僵尸进程时，由 init 调用 `wait` 完成回收。

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

当前实现已经能够维护 PID、PGID 和 SID 的基本关系，但终端前台进程组、后台读写限制和完整孤儿进程组规则尚未形成闭环。

== 权限与进程控制

=== UID/GID 三元组 <sec:uid-gid>

Linux 不用一个 UID 同时表示进程的来源和当前权限，而是维护 real、effective、saved 三个值。real UID 表示进程原本属于哪个用户；effective UID 是执行文件访问、程序执行和信号发送等权限检查时使用的身份；saved UID 保存进程曾经拥有的有效身份。这样设计是为了支持受控的权限切换：带有 set-user-ID 属性的程序可以先以较高权限完成必要操作，再降低 effective UID，之后若还需执行受保护操作，可以从 saved UID 恢复。GID 也采用相同的三元组。

StarryOS 基线中的部分身份设置接口只返回成功，没有保存身份变化。moss 在 `ProcessData` 中加入完整的六个原子字段，并在 `setresuid`、`setresgid` 等接口中检查转换范围：

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

非特权进程只能在原有三元组中切换，effective UID 为 0 的特权进程才可以设置新的身份。修改后的 effective UID/GID 会参与程序执行权限（见 @lst:exec-check）、`kill` 和 ptrace attach 等检查，凭据因此成为实际权限判断依据，而不再只是查询接口返回的固定数值。capability 也由固定结果改为保存 effective、permitted、inheritable 和 bounding 集合，并限制进程任意扩大自身能力。

=== ptrace 最小控制链路

moss 新增 ptrace 的基础状态管理，建立 `TRACEME`、`ATTACH`、`DETACH`、停止、继续和 wait 报告之间的关系。attach 时先检查目标是否存在、是否已被跟踪以及调用者是否具有相应权限；目标停止后，跟踪者通过 wait 获取事件。

当前实现只能完成最小控制流程。`PTRACE_SYSCALL` 和 `PTRACE_SINGLESTEP` 暂按普通继续处理，寄存器、目标内存和 siginfo 读写请求返回 `EIO`，因此还不能视为完整的 GDB 后端。

=== namespace 的当前状态

moss 为 PID、UTS 和 User namespace 保存 inode、父 namespace 和 init PID 等元数据，也能识别 clone 与 unshare 中的相关标志，使 `/proc/self/ns/*` 能够报告不同的 inode 编号。但当前不会建立独立 PID 视图、主机名隔离或 UID 映射，代码也明确将相关路径标记为兼容桩。因此，这部分只能描述为 namespace 接口和状态预留，不能计作完整的容器隔离能力。

== 本章小结

moss 选择沿用 StarryOS 的进程线程框架，围绕四条主线完成加固：以 `Thread` 和 `ProcessData` 组成双层对象模型，以线程归属、父子树和进程组会话维护三类组织关系，以 real、effective、saved ID 构成三元凭据，并把 clone、exec、exit、wait 串成完整生命周期。

在此基础上，moss 补严 `clone3` 的 ABI 和失败回滚，增加 `execveat`，完善子进程状态、等待接口和退出清理，并加入有状态的权限和 ptrace 控制。第三章继续说明等待过程如何接入任务阻塞与唤醒。当前还未完成完整 vfork、多线程 exec、namespace 隔离和 ptrace 数据访问，但进程从创建到回收的主要管理关系已经比 StarryOS 基线更完整。
