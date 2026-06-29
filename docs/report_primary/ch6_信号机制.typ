#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(6)

= 信号机制

信号是进程间传递异步事件的基本手段。进程之间可以用信号通知对方，内核也借助信号把硬件异常、终端中断、子进程状态变化等事件交给进程处理。

Moss 沿用 StarryOS 的信号框架，重点补了三块缺口：进程的可暂停状态、信号发送的权限校验，以及父进程对子进程停止与继续事件的感知。

== StarryOS 的信号机制

=== 整体结构

StarryOS 的信号机制由底层库 `starry-signal` 提供核心语义，内核负责将其接入任务管理、系统调用以及返回用户态的处理路径。信号管理器分两层组织：

- *进程级*：保存共享的待处理信号队列、信号处理动作表，以及进程内所有线程的引用。信号发给进程时，从中挑一个没有屏蔽该信号的线程来处理。
- *线程级*：保存线程私有的待处理信号、屏蔽集和信号栈。

标准信号只保留一份，重复发送会合并；实时信号按发送顺序排队，每次发送都不丢。

=== 信号检查与递送

每次从内核返回用户态之前都会检查信号。内核从队列里取出一个未屏蔽的信号，根据处理动作决定后续行为——终止、忽略、停止，或者跳到用户注册的处理函数。

调用用户处理函数时，内核在用户栈上构造信号栈帧，保存被中断时的寄存器状态和恢复所需的上下文，再把控制流转过去。函数执行完后，通过 `sigreturn` 系统调用回到内核，内核恢复栈帧继续原来的执行流。

`kill`、`tkill`、`tgkill`、`sigaction`、`sigprocmask`、`sigsuspend`、`sigtimedwait` 等系统调用都建立在这条链路上，Moss 沿用了这些系统调用的实现。

=== 现存问题

StarryOS 的实现存在三个明显缺口：

1. *停止与继续没有实现*。收到 `SIGSTOP`、`SIGTSTP` 时直接让进程退出，对 `SIGCONT` 不做处理。Shell 的作业控制因此完全无法工作。

2. *信号发送没有权限校验*。普通用户理论上只能向同 UID 的进程发信号，这个检查在 StarryOS 里缺失。

3. *父进程看不到子进程的停止与继续*。`waitid` 的 `WSTOPPED`、`WCONTINUED` 标记形同虚设。

后面几节分别说明 Moss 如何处理这三个问题。

== 进程可暂停状态

=== 问题所在

StarryOS 里进程只有两条路：继续运行，或者退出。`SIGSTOP` 走的是退出路径，但 POSIX 标准规定停止信号应该让进程暂停、保留现场，等 `SIGCONT` 到来后再从原位置继续——停止不等于死。

Shell 按下 Ctrl+Z 时最能说明这个问题。前台进程收到 `SIGTSTP`，应该暂停；用户之后执行 `fg`，进程从断点继续。StarryOS 的实现里 Ctrl+Z 会直接杀掉进程，作业控制整个语义因此垮掉。GDB 等调试器的暂停和恢复依赖的也是同一套机制。

=== 核心结构

要支持暂停，进程需要一个对外可见的"暂停中"状态，以及一个让线程真正挂住自己的地方。Moss 引入 `ProcessWaitStatus` 枚举，并在 `ProcessData` 上新增两个字段：

#figure(
  [
    ```rust
    pub enum ProcessWaitStatus {
        Stopped(Signo),   // 被停止信号挂起，记录触发的信号编号
        Continued(Signo), // 被继续信号唤醒，记录触发的信号编号
    }

    pub struct ProcessData {
        // ...
        // 父进程通过 waitid 可见的暂停/继续状态
        pub wait_status: SpinNoIrq<Option<ProcessWaitStatus>>,
        // 被停止的线程在此事件上挂起，等 SIGCONT 或 SIGKILL 唤醒
        pub stop_event: Arc<PollSet>,
    }
    ```
  ],
  caption: [进程可暂停状态的核心结构],
) <lst:wait-status-struct>

`wait_status` 是对外暴露的状态，父进程通过 `waitid` 读到的就是这里；`stop_event` 是线程真正挂住自己的地方，停止后阻塞在这个事件上，等到被唤醒。

=== 处理路径

信号检查路径里，停止和继续两条分支现在变成了：

#figure(
  [
    ```rust
    SignalOSAction::Stop => {
        // 写入对外可见的停止状态
        *thr.proc_data.wait_status.lock() =
            Some(ProcessWaitStatus::Stopped(signo));
        // 通知父进程可以从 waitid 取结果了
        wake_waiting_parent(&thr.proc_data);
        // 线程在 stop_event 上挂起，等待被唤醒
        wait_for_continue(&thr.proc_data);
    }
    SignalOSAction::Continue => {
        *thr.proc_data.wait_status.lock() =
            Some(ProcessWaitStatus::Continued(signo));
        wake_waiting_parent(&thr.proc_data);
        // stop_event 上的线程在信号发送路径里被唤醒
    }
    ```
  ],
  caption: [停止与继续信号的处理路径],
) <lst:stop-continue-handler>

停止路径的顺序是：写状态、通知父进程、线程挂在 `stop_event` 上。继续路径：更新状态，通知父进程，同时唤醒 `stop_event`。`SIGKILL` 同样会唤醒 `stop_event`，确保被挂住的进程能走完退出流程，不会卡死在暂停状态里。

@fig:process-state 展示了进程在运行、暂停和僵尸三种状态之间的完整流转——停止与继续在运行和暂停之间切换，退出后进入僵尸状态，父进程调用 `wait` 完成最终回收。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-2-进程状态机.svg", width: 55%)
    ]
  ],
  caption: [进程状态机],
) <fig:process-state>

== 信号驱动的 wait 语义

进程停止和恢复是信号引起的状态变化，但父进程能否观察到这些变化，取决于 `wait` 系列调用的配置。`waitpid` 和 `waitid` 默认只报告退出事件，要让父进程感知停止与继续，需要额外的选项：

#figure(
  table(
    columns: (1.5fr, 4fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([选项], [作用]),
    [`WUNTRACED`], [允许报告子进程停止状态，否则停止事件对父进程不可见],
    [`WCONTINUED`], [允许报告子进程继续运行状态],
    [`WNOHANG`], [当前没有可报告状态时立即返回，不阻塞父进程],
    [`WNOWAIT`], [读取状态但不消费，后续仍可再次查询],
  ),
  caption: [子进程等待的主要选项],
) <tab:wait-options-signal>

还是以 Shell 为例。Ctrl+Z 让前台进程组暂停，Shell 需要知道这些进程是"暂停等继续"还是"已经退出"——没有 `WUNTRACED`，Shell 区分不了这两种情况。`WCONTINUED` 处理对称的一侧：`fg` 发出 `SIGCONT` 后，这次状态变化同样需要 `WCONTINUED` 才能被父进程感知到。

如 @fig:wait-timeline 所示，停止、继续和退出三类事件都可以成为一次 `wait` 的报告。前两者是状态变化通知，父进程读完后子进程仍然存在；退出才是最后一次报告，内核读完后才回收僵尸进程。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-2-wait语义.svg", width: 92%)
    ]
  ],
  caption: [子进程停止、继续、退出三类事件与父进程 wait 消费的时序],
) <fig:wait-timeline>

`waitid` 相比 `waitpid` 的好处是返回结构化的 `siginfo_t`，能直接区分正常退出、信号终止、停止和继续，不需要调用者手动解析整数状态。`waitid` 还支持通过 pidfd 选择目标——pidfd 是指向进程对象的文件描述符，不会随 PID 复用而改变指向，配合 `WNOWAIT` 可以先查看状态、之后再完成回收。父进程没有可报告状态时如何进入阻塞、状态变化后怎样被唤醒，见下一节。

== 父进程如何感知状态变化

=== 通知路径

子进程状态变化发生在子进程自己的执行流里，父进程可能正阻塞在 `waitid` 上。这里需要一套机制：状态变化时，主动唤醒正在等待的父进程。

StarryOS 已有一个 `child_exit_event`，但只在子进程退出时触发。停止和继续没有对应的唤醒动作，所以 `WSTOPPED`、`WCONTINUED` 的语义一直没法生效。

Moss 没有新建通道，而是让停止和继续两条路径也触发同一个 `child_exit_event`，把它的语义扩展为"子进程任何状态变化都触发"：

#figure(
  [
    ```rust
    fn wake_waiting_parent(proc_data: &ProcessData) {
        if let Some(parent) = proc_data.proc.parent()
            && let Ok(parent_data) = get_process_data(parent.pid())
        {
            parent_data.child_exit_event.wake();
        }
    }
    ```
  ],
  caption: [子进程状态变化时唤醒父进程],
) <lst:wake-parent>

停止路径和继续路径在写完 `wait_status` 之后都调用这个函数（见 @lst:stop-continue-handler）。父进程被唤醒后重新读 `wait_status`，就能拿到子进程的最新状态。

=== 防止通知丢失

父进程登记唤醒入口（Waker）和子进程状态变化之间有一个竞争窗口：父进程检查完 `wait_status`、还没把 Waker 挂上去，子进程的状态恰好先变了，这次通知就悄悄丢了，父进程会一直等下去。

Moss 用登记后复查覆盖这个窗口：

#figure(
  [
    ```rust
    match check_children().transpose() {
        Some(result) => Poll::Ready(result),
        None => {
            // 暂无结果，登记父进程的 Waker
            child_exit_event.register(cx.waker());
            // 登记完立刻再查一次，覆盖登记窗口里发生的状态变化
            match check_children().transpose() {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        }
    }
    ```
  ],
  caption: [父进程等待子进程状态，登记后复查防止通知丢失],
) <lst:child-wait-register>

第一次检查没结果，把 Waker 登记上去，然后立刻再查一次。如果第二次查到了结果，说明状态变化恰好落在那个窗口里，直接返回不需要等待。如果仍然没有，才放心地进入阻塞。这套双重检查的通用模式在第三章阻塞与唤醒机制里有更完整的说明。

== 进程身份与信号权限校验

POSIX 规定，除 root 外，发送者只能向与自己 UID 或 SUID 匹配的目标发信号。`SIGCONT` 有个例外：同一会话内可以跨 UID 发，让 Shell 能对已切换身份的子进程发出恢复命令。

StarryOS 的 `ProcessData` 里没有 UID 相关字段，检查根本没有基础。Moss 在进程与线程管理一章（@sec:uid-gid）引入了 real/effective/saved 三元凭据，信号权限校验直接使用这些字段：

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

`sys_kill` 支持四种目标语义：指定进程、当前进程组、广播、指定进程组。批量操作的结果有三种情况：至少有一个目标有权限就算成功，全部因权限不足失败才返回 `EPERM`，完全没有匹配目标则返回 `ESRCH`。

== 小结

StarryOS 已经完成了信号发送、入队、屏蔽与用户处理函数递送的完整链路。Moss 在此基础上补了三个环节：

#figure(
  table(
    columns: (1fr, 1.6fr, 1.6fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([补充内容], [核心结构], [支撑的使用场景]),
    [进程可暂停], [`ProcessWaitStatus`、`stop_event`], [Shell 作业控制、GDB 调试暂停],
    [信号驱动 wait 语义], [`WUNTRACED`、`WCONTINUED` 选项生效], [父进程按需感知子进程停止与恢复],
    [状态变化通知], [`child_exit_event` 复用，登记后复查], [`waitid` 停止与继续事件正常到达父进程],
    [信号权限], [UID/EUID/SUID 三元凭据校验], [POSIX 信号权限模型，跨 UID 隔离],
  ),
  caption: [Moss 对信号机制的补充],
) <tab:signal-deltas>
