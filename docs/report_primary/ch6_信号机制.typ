#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(6)

= 信号机制

信号是 Unix/Linux 用于异步事件通知的基本机制。进程之间借助信号传递事件，内核也利用信号将异常、终端中断、子进程状态变化等事件转交给进程处理。

moss 基于 StarryOS 改造而来，信号机制的整体框架沿用了 StarryOS 已有的设计。在此基础上，moss 针对若干语义缺口进行了补充：进程的可暂停状态、信号发送时的权限校验，以及父进程对子进程停止与继续事件的可见性。本章先介绍 StarryOS 信号机制的整体结构与现存问题，随后逐一说明 moss 所做的增量。

== StarryOS 的信号机制

=== 整体结构

StarryOS 的信号机制由底层库 `starry-signal` 提供核心语义，内核负责将其接入任务管理、系统调用以及返回用户态的处理路径。信号管理器按进程和线程两层组织：

- *进程级*：每个进程持有一个进程级信号管理器，保存进程共享的待处理信号队列、信号处理动作表，以及进程内所有线程的引用。当信号发往进程时，会从中挑选一个未屏蔽该信号的线程负责处理。
- *线程级*：每个线程持有一个线程级信号管理器，保存该线程私有的待处理信号、屏蔽集和信号栈。

待处理信号按类型分别存储。标准信号仅保留一份，重复发送会被合并；实时信号按发送顺序排队，确保每次发送都不丢失。

=== 信号检查与递送

信号检查发生在每次从内核返回用户态之前。内核从待处理队列中取出一个未被屏蔽的信号，根据当前的信号处理动作决定后续行为。可能的动作包括默认终止、默认核心转储、默认忽略、默认停止与继续，以及调用用户注册的信号处理函数。

对于用户处理函数，内核在用户栈上构造一个信号栈帧，其中保存被中断时的寄存器状态、信号信息以及用于恢复执行的上下文，随后将控制流转移到用户处理函数。用户处理函数执行结束后，通过 `sigreturn` 系统调用回到内核，由内核恢复栈帧并继续原有执行流。

这一发送、入队、检查、递送、恢复的完整链路在 StarryOS 中已经实现，`kill`、`tkill`、`tgkill`、`sigaction`、`sigprocmask`、`sigsuspend`、`sigtimedwait` 等系统调用均建立在该链路之上，moss 完全沿用。

=== 现存问题

围绕信号语义的完整性，StarryOS 的实现仍存在若干明显缺口：

1. *停止与继续语义未真正实现*。StarryOS 在收到 `SIGSTOP`、`SIGTSTP` 等停止信号时直接令进程退出，对 `SIGCONT` 则不做任何处理。其结果是进程无法真正暂停，Shell 的作业控制功能（如 `Ctrl-Z`、`fg`、`bg`）无法工作。

2. *信号发送缺少权限校验*。各信号发送接口未对发送者与目标进程之间的身份关系做检查。按 POSIX 规范，普通用户只能向同 UID 的目标发送信号，root 才能向任意进程发送信号，缺少此检查将导致权限隔离失效。

3. *父进程无法感知子进程的状态变化*。`wait` 系列系统调用只能获取子进程的退出状态，对子进程的停止和继续事件无任何通知机制，`waitid` 的 `WSTOPPED`、`WCONTINUED` 语义形同虚设。

== 进程可暂停状态的引入

实现停止与继续语义的关键在于：进程需要能够处于"存活但不参与调度"的状态。StarryOS 中进程仅有运行与退出两种状态，缺少描述"暂停"的中间形态。为此，moss 引入了一个新的进程等待状态枚举，用以表示进程对外可见的暂停与恢复信息：

#figure(
  [
    ```rust
    pub enum ProcessWaitStatus {
        Stopped(Signo),   // 进程被某停止信号挂起，记录触发的信号
        Continued(Signo), // 进程被某继续信号唤醒，记录触发的信号
    }
    ```
  ],
  caption: [进程等待状态],
) <lst:wait-status-enum>

该状态及其配套的事件通道一并挂载到进程的共享数据结构 `ProcessData` 上：

#figure(
  [
    ```rust
    pub struct ProcessData {
        ...
        // 父进程通过 waitid 可见的暂停/继续状态
        pub wait_status: SpinNoIrq<Option<ProcessWaitStatus>>,
        // 被停止线程在此事件上挂起，等待 SIGCONT 或 SIGKILL 唤醒
        pub stop_event: Arc<PollSet>,
    }
    ```
  ],
  caption: [`ProcessData` 上新增的字段],
) <lst:wait-status-fields>

`wait_status` 是父进程通过 `waitid` 系统调用可以读到的状态；`stop_event` 则承担线程挂起与唤醒的职责，被停止的线程在该事件上阻塞，直到收到 `SIGCONT` 或 `SIGKILL` 后被唤醒。

凭借这一对结构，moss 将信号检查路径中原本空缺的停止与继续两条分支补齐。`check_signals` 内的停止与继续动作处理如下：

#figure(
  [
    ```rust
    SignalOSAction::Stop => {
        // 写入对外可见的停止状态，并记录触发的信号
        *thr.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Stopped(signo));
        // 通知父进程的 waitid 调用可以返回
        wake_waiting_parent(&thr.proc_data);
        // 当前线程在 stop_event 上挂起，直至被唤醒
        wait_for_continue(&thr.proc_data);
    }
    SignalOSAction::Continue => {
        // 状态切回继续，父进程的 waitid 同样需要被通知
        *thr.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Continued(signo));
        wake_waiting_parent(&thr.proc_data);
    }
    ```
  ],
  caption: [停止与继续动作的处理],
) <lst:stop-continue>

进入停止分支后，当前线程在 `stop_event` 上挂起，直至状态翻转或进程被强制终止。信号发送路径上，识别到 `SIGCONT` 时将 `wait_status` 置为 `Continued`、唤醒 `stop_event` 上的线程并通知父进程；识别到 `SIGKILL` 时同样唤醒 `stop_event`，以确保被停止的进程能够走完退出流程。

该机制使进程首次具备真正意义上的可暂停状态。Shell 在按下 `Ctrl-Z` 后能够保留前台作业的完整执行现场，并在 `fg` 之后从原位置继续执行；GDB 等调试器所依赖的暂停与恢复语义也得以建立在此机制之上。

== 进程身份与信号权限校验

POSIX 对信号发送的权限规定大致为：除 root 外，发送者只能向与自己 UID 或 SUID 匹配的目标发送信号；`SIGCONT` 例外，同一会话内可以跨 UID 发送，以支持 Shell 对换过身份的子进程进行作业控制。

StarryOS 的 `ProcessData` 中并未包含任何 Unix 身份相关字段，权限校验缺乏基础。moss 为 `ProcessData` 增补了完整的身份字段，包括真实、有效与保存的 UID 及 GID。在此基础上，moss 在信号发送路径上引入了统一的权限判断逻辑，其规则可概括为：

#figure(
  table(
    columns: (1.4fr, 2fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([条件], [允许发送]),
    [发送者有效 UID 为 0（root）], [是],
    [发送者 UID 或 EUID 与目标 UID 或 SUID 匹配], [是],
    [发送 `SIGCONT` 且与目标处于同一会话], [是],
    [其余情形], [否],
  ),
  caption: [信号发送的权限规则],
) <tab:signal-perm>

`sys_kill` 支持四种目标语义：指定进程、当前进程组、广播以及指定进程组。moss 为每种语义都包裹了一层带权限校验的发送过程。对于组发送和广播这类批量操作，校验结果需要进一步区分：只要存在至少一个有权限的目标，操作即视为成功；若找到了目标但全部因权限不足而失败，则返回 `EPERM`；若完全没有匹配的目标，则返回 `ESRCH`。

身份字段就位后，moss 的信号发送行为符合 POSIX 权限模型，进程间的隔离得到保障。会话内 `SIGCONT` 例外规则使 Shell 能够对已通过 `setuid` 切换身份的子进程发出恢复命令，从而支持完整的作业控制流程。

== 父进程对子进程状态的可见性

POSIX 规定子进程的退出、停止与继续都应通知父进程，以便父进程通过 `wait` 系列接口获取状态。StarryOS 已经具备一个用于子进程退出通知的事件通道（`child_exit_event`），但仅在子进程退出时被触发，停止与继续两种事件没有相应的唤醒动作，导致 `waitid` 的 `WSTOPPED`、`WCONTINUED` 语义无法生效。

moss 没有新建通知通道，而是在停止与继续两条状态变更路径上复用 `child_exit_event`，使其语义扩展为"子进程任何状态变化均触发"。具体而言，在前述停止与继续的处理过程中，moss 在更新 `wait_status` 之后统一调用一个辅助函数，由其负责唤醒父进程：

#figure(
  [
    ```rust
    fn wake_waiting_parent(proc_data: &ProcessData) {
        // 子进程状态发生变化时，唤醒父进程在 waitid 中的等待
        if let Some(parent) = proc_data.proc.parent()
            && let Ok(parent_data) = get_process_data(parent.pid())
        {
            parent_data.child_exit_event.wake();
        }
    }
    ```
  ],
  caption: [父进程唤醒],
) <lst:wake-parent>

父进程从 `waitid` 中返回时，通过查看 `wait_status` 字段即可区分子进程当前是处于退出、停止还是继续状态。该机制使 Shell 能够在后台作业被停止时立即提示用户，在作业恢复运行时即时更新提示符，避免依赖轮询。前述各项改动也由此真正在用户态可用。

== 小结

StarryOS 已经搭建起完整的信号发送、入队、屏蔽与用户处理函数递送链路，moss 在此基础上针对若干语义缺口进行了补充：

#figure(
  table(
    columns: (1fr, 1.6fr, 1.6fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([能力], [核心结构], [支撑的语义]),
    [进程可暂停], [`ProcessWaitStatus`、`wait_status`、`stop_event`], [Shell 作业控制],
    [信号权限], [`ProcessData` 上的 UID/EUID/SUID 等身份字段], [POSIX 信号权限模型],
    [父进程可见性], [`child_exit_event` 在停止与继续路径上的复用], [`waitid` 的 `WSTOPPED`、`WCONTINUED` 语义],
  ),
  caption: [moss 对信号机制的补充],
) <tab:signal-deltas>
