#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(4)

= 信号机制

信号是 Unix/Linux 系统中进程间通信和异步事件通知的基础机制。进程可以通过信号相互通知事件，内核也会在异常、终端中断或子进程状态变化时向进程发送信号。moss 在信号框架的基础上，完善了停止与继续、权限检查、调试器拦截和父进程通知等关键路径，使信号机制能够支持作业控制、ptrace 调试和完整的 POSIX 信号语义。

本章先说明信号的基本概念和早期实现情况，再分析 moss 针对信号状态管理、发送权限、作业控制和调试支持所做的增量修改。

== 概述

=== 信号的基本概念

信号是一种轻量级的进程间通信机制，用于通知进程发生了某个事件。与管道、消息队列等需要显式数据传输的机制不同，信号只携带事件类型和少量附加信息。进程收到信号后可以执行默认操作、忽略信号或调用用户注册的处理函数。

Linux 支持 64 种信号，其中 1-31 为标准信号，32-64 为实时信号。标准信号只记录是否到达，多次发送同一标准信号时只保留一份；实时信号则会排队保存，保证每次发送都能被接收方看到。

每个信号都有默认行为，分为五类：

- *终止*（Terminate）：结束进程，如 `SIGTERM`、`SIGINT`
- *忽略*（Ignore）：不做任何处理，如 `SIGCHLD`、`SIGWINCH`
- *核心转储*（CoreDump）：结束进程并生成调试信息，如 `SIGSEGV`、`SIGABRT`
- *停止*（Stop）：暂停进程执行，如 `SIGSTOP`、`SIGTSTP`
- *继续*（Continue）：恢复已停止的进程，如 `SIGCONT`

进程可以通过 `sigaction` 修改信号处理方式，但 `SIGKILL` 和 `SIGSTOP` 不能被捕获、阻塞或忽略，保证了系统始终能够终止或停止任意进程。

=== 早期实现情况

早期版本已经实现了信号的核心框架：

- *信号发送*：`kill`、`tkill`、`tgkill` 可以向进程或线程发送信号
- *信号接收*：线程和进程各自维护待处理信号队列
- *信号处理*：用户态信号处理函数的栈帧构造与恢复
- *信号屏蔽*：`sigprocmask` 可以阻塞部分信号的传递
- *信号等待*：`sigtimedwait`、`sigsuspend` 主动等待信号到达

底层库 `starry-signal` 提供了线程级和进程级的信号管理器。线程级管理器 `ThreadSignalManager` 保存每个线程的待处理信号、屏蔽集和信号栈；进程级管理器 `ProcessSignalManager` 保存进程共享的信号队列和信号处理动作表。发送到进程的信号会被分配给其中一个未屏蔽该信号的线程。

信号检查发生在系统调用返回用户态之前。内核调用 `check_signals` 遍历待处理信号，根据信号处理动作决定是调用用户处理函数、执行默认操作还是继续等待。用户处理函数通过在用户栈上构造 `SignalFrame` 实现，其中保存了被中断时的寄存器状态、信号信息和恢复用的上下文，处理函数返回时通过 `sigreturn` 恢复现场。

=== 早期版本的技术局限

早期框架能够完成基本的信号发送与处理，但在作业控制、权限检查和调试支持方面存在显著不足：

1. *停止与继续信号未实现*：`SIGSTOP`、`SIGTSTP` 等停止信号和 `SIGCONT` 继续信号的默认行为被简化为直接退出或空操作。进程无法真正暂停执行，也无法在停止后恢复，导致作业控制（`Ctrl-Z`、`fg`、`bg`）无法正常工作。这直接影响 Shell 的作业管理功能和 ltp 测试集中的相关用例。

2. *信号发送缺少权限检查*：早期实现没有验证发送者是否有权向目标进程发送信号。按照 POSIX 规范，普通用户只能向相同 UID 的进程发送信号，root 可以向任意进程发送信号，`SIGCONT` 在同一会话内可以跨用户发送。缺失权限检查会导致安全漏洞和测例失败。

3. *调试器无法拦截信号*：ptrace 调试器需要在信号传递给被调试进程之前截获，判断是否继续传递、修改或丢弃。早期实现没有为调试器预留拦截点，导致 GDB 等调试工具无法正常处理被调试进程的信号。

4. *父进程无法感知子进程状态变化*：子进程因信号停止或继续后，父进程无法通过 `wait` 系列系统调用获知这些状态变化。这使得 Shell 无法正确显示作业状态，也影响 `waitpid` 的 `WUNTRACED` 和 `WCONTINUED` 选项。

5. *部分系统调用语义不完整*：`sys_pause` 尚未实现；`sys_rt_sigtimedwait` 在被信号处理函数中断时返回值不符合 POSIX 规范；`sys_tkill` 和 `sys_tgkill` 缺少参数合法性检查，可能导致内核错误。

=== moss 的技术增量

moss 保留早期的信号框架，改动集中在四个关键环节：停止与继续的状态管理、发送权限验证、调试器拦截和父进程通知。具体对应关系如下：

#figure(
  table(
    columns: (1.4fr, 2.35fr, 2.65fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([内容], [早期实现], [moss 增量]),
    [信号类型], [64 种信号定义和默认行为], [完善停止与继续信号的默认处理],
    [信号发送], [kill/tkill/tgkill 基本实现], [权限检查、参数校验、SIGCONT/SIGKILL 特殊处理],
    [信号接收], [线程和进程级信号队列], [增加 possibly_has_signal 快速路径优化],
    [信号处理], [用户处理函数栈帧构造], [ptrace 拦截点、停止/继续状态管理],
    [作业控制], [停止与继续信号未实现], [ProcessWaitStatus 状态机、stop_event 通知],
    [系统调用], [sigaction/sigprocmask 等], [新增 pause，修正 sigtimedwait 语义],
  ),
  caption: [信号机制的主要增量],
) <tab:signal-overview>

这些修改共同作用于信号从发送到处理的完整路径。权限检查防止非法信号发送，停止与继续机制支持作业控制，ptrace 拦截支持调试器，父进程通知完善 `wait` 语义。后文先用信号的生命周期说明发送、接收和处理流程，再围绕停止与继续、权限检查和调试支持分析 moss 的具体实现。

== 信号的生命周期

=== 信号的发送

信号发送从系统调用开始。`kill` 向进程或进程组发送信号，`tkill` 向线程发送信号，`tgkill` 向指定进程内的线程发送信号。用户态程序也可以通过 `raise` 向自己发送信号。

#figure(
  [
    ```rust
    pub fn sys_kill(pid: i32, signo: u32) -> AxResult<isize> {
        let sig = make_siginfo(signo, SI_USER as _)?;
        let sig_signo = sig.as_ref().map(|sig| sig.signo());
        let curr = current();
        let caller = &curr.as_thread().proc_data;

        match pid {
            1.. => {
                // 向指定进程发送信号
                let target = get_process_data(pid as _)?;
                send_signal_to_process_checked(caller, &target, sig)?;
            }
            0 => {
                // 向当前进程组发送信号
                let pgid = caller.proc.group().pgid();
                send_signal_to_process_group_checked(caller, pgid, sig)?;
            }
            -1 => {
                // 向所有有权限的进程发送信号
                // ...
            }
            ..-1 => {
                // 向指定进程组发送信号
                let pgid = pid.checked_neg().ok_or(AxError::NoSuchProcess)? as Pid;
                send_signal_to_process_group_checked(caller, pgid, sig)?;
            }
        }
        Ok(0)
    }
    ```
  ],
  caption: [sys_kill 的信号发送逻辑],
) <lst:sys-kill>

信号发送前先构造 `SignalInfo`，包含信号编号、发送原因代码和发送者进程 ID。`signo` 为 0 时表示权限检查而不实际发送信号。

=== 信号的接收与排队

信号发送后进入接收方的待处理队列。moss 使用两级队列结构：线程级队列和进程级队列。发送到线程的信号进入线程队列，发送到进程的信号进入进程队列，检查时先查线程队列再查进程队列。

#figure(
  [
    ```rust
    pub struct ThreadSignalManager {
        proc: Arc<ProcessSignalManager>,
        pending: SpinNoIrq<PendingSignals>,
        blocked: SpinNoIrq<SignalSet>,
        possibly_has_signal: AtomicBool,
        // ...
    }

    pub struct ProcessSignalManager {
        pending: SpinNoIrq<PendingSignals>,
        actions: Arc<SpinNoIrq<SignalActions>>,
        children: SpinNoIrq<Vec<(u32, Weak<ThreadSignalManager>)>>,
        possibly_has_signal: AtomicBool,
        // ...
    }
    ```
  ],
  caption: [线程级和进程级信号管理器],
) <lst:signal-managers>

`PendingSignals` 区分标准信号和实时信号。标准信号只保存一份，重复发送会被合并；实时信号使用队列保存，按发送顺序排队。

#figure(
  [
    ```rust
    pub struct PendingSignals {
        pub set: SignalSet,
        info_std: [Option<Box<SignalInfo>>; 32],
        info_rt: [VecDeque<SignalInfo>; 33],
    }

    pub fn put_signal(&mut self, sig: SignalInfo) -> bool {
        let signo = sig.signo();
        let added = self.set.add(signo);

        if signo.is_realtime() {
            self.info_rt[signo as usize - 32].push_back(sig);
        } else {
            if !added {
                // 标准信号已存在，不重复添加
                return false;
            }
            self.info_std[signo as usize] = Some(Box::new(sig));
        }
        true
    }
    ```
  ],
  caption: [待处理信号的排队逻辑],
) <lst:pending-signals>

信号入队后设置 `possibly_has_signal` 标志。这是一个原子布尔值，用于快速判断是否有待处理信号，避免每次都加锁检查队列。

=== 信号的检查与处理

信号检查发生在系统调用返回用户态之前。内核从待处理信号中选出一个未被屏蔽的信号，根据信号处理动作决定如何处理。

#figure(
  [
    ```rust
    pub fn check_signals(
        thr: &Thread,
        uctx: &mut UserContext,
        restore_blocked: Option<SignalSet>,
    ) -> bool {
        // ptrace 调试器拦截点
        crate::syscall::ptrace_wait_if_stopped(thr);

        let pid = thr.proc_data.proc.pid();
        if crate::syscall::ptrace_is_traced(pid) {
            let mask = !thr.signal.blocked();
            if let Some(sig) = thr.signal.dequeue_signal(&mask) {
                let signo = sig.signo();
                if crate::syscall::ptrace_intercept_signal(thr, signo) {
                    return true;
                }
                do_exit(signo as i32, true);
            }
        }

        let Some((sig, os_action)) = thr.signal.check_signals(uctx, restore_blocked) else {
            return false;
        };

        let signo = sig.signo();
        match os_action {
            SignalOSAction::Terminate => {
                do_exit(signo as i32, true);
            }
            SignalOSAction::CoreDump => {
                do_exit((signo as i32) | 0x80, true);
            }
            SignalOSAction::Stop => {
                *thr.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Stopped(signo));
                wake_waiting_parent(&thr.proc_data);
                wait_for_continue(&thr.proc_data);
            }
            SignalOSAction::Continue => {
                *thr.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Continued(signo));
                wake_waiting_parent(&thr.proc_data);
            }
            SignalOSAction::Handler => {
                // 用户处理函数已经设置，直接返回用户态执行
            }
        }
        true
    }
    ```
  ],
  caption: [信号检查与处理的主流程],
) <lst:check-signals>

内核首先检查进程是否正在被 ptrace 调试。被调试进程收到信号时，调试器可以截获信号并决定是否继续传递。未被拦截的信号继续进入正常处理流程。

信号处理动作分为五种：

- `Terminate`：进程退出，退出码为信号编号
- `CoreDump`：进程退出并设置 core dump 标志（退出码最高位为 1）
- `Stop`：进程进入停止状态，等待 `SIGCONT` 或 `SIGKILL` 恢复
- `Continue`：从停止状态恢复，进程继续运行
- `Handler`：调用用户注册的信号处理函数

用户处理函数通过修改用户态寄存器实现。内核在用户栈上构造 `SignalFrame`，保存被中断时的上下文、信号信息和恢复用的 `ucontext`，再把指令指针指向用户处理函数、参数寄存器指向栈帧，返回地址设为 `sigreturn` 系统调用的入口。用户处理函数执行完毕后自动调用 `sigreturn`，内核从栈帧恢复寄存器状态，进程继续原来的执行流。

== 权限检查机制

=== 信号发送权限规则

POSIX 规定，普通用户只能向相同用户的进程发送信号，root 用户可以向任意进程发送信号。moss 在 `send_signal_to_process_checked` 中实现了这一检查：

#figure(
  [
    ```rust
    fn can_send_signal(caller: &ProcessData, target: &ProcessData, signo: Option<Signo>) -> bool {
        caller.euid() == 0
            || caller.uid() == target.uid()
            || caller.uid() == target.suid()
            || caller.euid() == target.uid()
            || caller.euid() == target.suid()
            || (signo == Some(Signo::SIGCONT)
                && caller.proc.group().session().sid() == target.proc.group().session().sid())
    }

    fn check_send_signal_permission(
        caller: &ProcessData,
        target: &ProcessData,
        signo: Option<Signo>,
    ) -> AxResult<()> {
        if can_send_signal(caller, target, signo) {
            Ok(())
        } else {
            Err(AxError::from(LinuxError::EPERM))
        }
    }
    ```
  ],
  caption: [信号发送权限检查],
) <lst:signal-permission>

权限检查规则包括：

1. root 用户（`euid == 0`）可以向任意进程发送信号
2. 发送者的实际或有效用户 ID 与目标进程的实际或保存的用户 ID 匹配时允许发送
3. `SIGCONT` 在同一会话内可以跨用户发送，用于作业控制

这些规则覆盖了 `kill`、`tkill`、`tgkill` 和 `rt_sigqueueinfo` 等所有信号发送系统调用。

=== 进程组和广播发送

`kill` 的 `pid` 参数为 0 时向当前进程组发送信号，为 -1 时向所有有权限的进程广播信号，为负数时向指定进程组发送信号。moss 在这些场景下也进行权限检查：

#figure(
  [
    ```rust
    fn send_signal_to_process_group_checked(
        caller: &ProcessData,
        pgid: Pid,
        sig: Option<SignalInfo>,
    ) -> AxResult<()> {
        let pg = get_process_group(pgid)?;
        let signo = sig.as_ref().map(|sig| sig.signo());
        let mut found = false;
        let mut permitted = false;

        for proc in pg.processes() {
            let Ok(target) = get_process_data(proc.pid()) else {
                continue;
            };
            found = true;

            if can_send_signal(caller, &target, signo) {
                permitted = true;
                if let Some(sig) = sig.as_ref() {
                    let _ = send_signal_to_process(target.proc.pid(), Some(sig.clone()));
                }
            }
        }

        if permitted {
            Ok(())
        } else if found {
            Err(AxError::from(LinuxError::EPERM))
        } else {
            Err(AxError::NoSuchProcess)
        }
    }
    ```
  ],
  caption: [进程组信号发送的权限检查],
) <lst:process-group-signal>

进程组内可能包含多个进程，权限检查需要遍历所有进程。只要有一个进程允许发送，整个操作就成功；所有进程都拒绝时返回 `EPERM`；进程组不存在或为空时返回 `ESRCH`。

广播发送（`pid == -1`）遍历系统中所有进程，跳过 init 进程和发送者自己，向每个有权限的目标进程发送信号。这用于实现 `killall` 等批量信号发送工具。

=== 参数合法性检查

moss 还完善了系统调用的参数检查。`tkill` 和 `tgkill` 要求线程 ID 和进程 ID 必须为正数：

#figure(
  [
    ```rust
    pub fn sys_tkill(tid: i32, signo: u32) -> AxResult<isize> {
        if tid <= 0 {
            return Err(AxError::InvalidInput);
        }
        let sig = make_siginfo(signo, SI_TKILL)?;
        send_signal_to_thread(None, tid as Pid, sig)?;
        Ok(0)
    }

    pub fn sys_tgkill(tgid: i32, tid: i32, signo: u32) -> AxResult<isize> {
        if tgid <= 0 || tid <= 0 {
            return Err(AxError::InvalidInput);
        }
        let sig = make_siginfo(signo, SI_TKILL)?;
        send_signal_to_thread(Some(tgid as Pid), tid as Pid, sig)?;
        Ok(0)
    }
    ```
  ],
  caption: [tkill 和 tgkill 的参数检查],
) <lst:tkill-validation>

负数或 0 会被拒绝，避免错误的 ID 类型转换导致非法内存访问。

== 停止与继续机制

停止与继续机制是作业控制的核心，也是本章的重点增量之一。早期实现中，停止信号直接导致进程退出，继续信号则被忽略，无法支持作业控制。moss 实现了完整的停止状态管理、阻塞等待和父进程通知机制。

=== 作业控制的需求

作业控制允许用户暂停正在运行的程序、将其放到后台继续执行或重新唤起到前台。Shell 通过信号和进程组实现作业控制：

- `Ctrl-Z` 向前台进程组发送 `SIGTSTP`，进程收到后停止执行
- `bg` 命令向停止的进程组发送 `SIGCONT`，进程在后台继续执行
- `fg` 命令将后台进程组拉到前台并发送 `SIGCONT`

进程停止后，父进程应当通过 `waitpid` 获知子进程状态变化，Shell 据此更新作业列表。进程继续运行后，父进程也应收到通知。

=== 停止状态的实现

moss 使用 `ProcessWaitStatus` 记录进程的等待状态。进程可以处于运行、停止或继续三种状态之一：

#figure(
  [
    ```rust
    pub enum ProcessWaitStatus {
        Stopped(Signo),
        Continued(Signo),
    }

    // 停止信号处理
    SignalOSAction::Stop => {
        *thr.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Stopped(signo));
        wake_waiting_parent(&thr.proc_data);
        wait_for_continue(&thr.proc_data);
    }
    ```
  ],
  caption: [停止信号的处理流程],
) <lst:signal-stop>

进程收到停止信号后，先将等待状态设为 `Stopped`，再唤醒正在等待的父进程。父进程从 `wait` 系统调用返回，获取子进程的停止状态和导致停止的信号编号。

停止后的进程进入 `wait_for_continue`，在该函数中阻塞，直到收到 `SIGCONT` 或 `SIGKILL`：

#figure(
  [
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
  ],
  caption: [等待继续信号的实现],
) <lst:wait-for-continue>

进程在停止状态下不会被调度运行，也不会响应其他信号（`SIGKILL` 和 `SIGCONT` 除外）。这保证了停止的进程真正暂停执行，而不是继续处理其他事件。

=== 继续信号的处理

`SIGCONT` 使停止的进程恢复运行。moss 在发送继续信号时特殊处理：

#figure(
  [
    ```rust
    pub fn send_signal_to_thread(tgid: Option<Pid>, tid: Pid, sig: Option<SignalInfo>) -> AxResult<()> {
        let task = get_task(tid)?;
        let thread = task.try_as_thread().ok_or(AxError::OperationNotPermitted)?;

        if let Some(sig) = sig {
            let signo = sig.signo();
            if signo == Signo::SIGCONT {
                *thread.proc_data.wait_status.lock() = Some(ProcessWaitStatus::Continued(signo));
                thread.proc_data.stop_event.wake();
                wake_waiting_parent(&thread.proc_data);
            } else if signo == Signo::SIGKILL {
                thread.proc_data.stop_event.wake();
            }
            // ...
            send_signal_thread_inner(&task, thread, sig);
        }
        Ok(())
    }
    ```
  ],
  caption: [继续信号的发送处理],
) <lst:signal-continue>

继续信号到达时，内核先更新等待状态为 `Continued`，再唤醒 `stop_event` 上的等待者。停止的进程从 `wait_for_continue` 返回，重新参与调度。同时唤醒父进程，父进程可以通过 `waitpid` 获知子进程已经继续运行。

`SIGKILL` 也会唤醒停止的进程，但不更新等待状态。进程从停止中恢复后直接进入退出流程，不再执行用户代码。

=== 父进程通知

子进程停止或继续后，父进程应当收到通知。moss 使用 `child_exit_event` 和 `wait_status` 配合实现：

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
  caption: [唤醒等待的父进程],
) <lst:wake-waiting-parent>

父进程调用 `wait` 或 `waitpid` 时，会在 `child_exit_event` 上等待。子进程状态变化后唤醒该事件，父进程恢复运行，检查所有子进程的 `wait_status`，找到已经停止或继续的子进程并返回相应状态。

这一机制在第二章已有说明，moss 将停止与继续状态接入该流程，使 Shell 能够正确管理作业状态。

== 调试支持与信号拦截

调试器支持是信号机制的另一个重点增量。早期实现中没有为调试器预留信号拦截点，导致 ptrace 无法正确处理被调试进程的信号。moss 在信号检查流程的开头增加了拦截点，使调试器能够观察、修改和控制信号的传递。

=== ptrace 的信号拦截需求

ptrace 是 Linux 的进程调试接口，允许调试器观察和控制被调试进程。调试器可以在被调试进程收到信号时暂停其执行，检查寄存器和内存，决定是否将信号传递给被调试进程。

典型的调试场景包括：

- 被调试进程访问非法地址，收到 `SIGSEGV`。调试器截获信号，显示崩溃位置和调用栈，而不是让进程直接退出
- 调试器在断点位置插入断点指令，触发 `SIGTRAP`。调试器收到通知，暂停进程并等待用户命令
- 用户向被调试进程发送 `SIGINT`。调试器可以选择将信号传递给进程，也可以丢弃信号并继续调试

moss 在 `check_signals` 的开头增加了 ptrace 拦截点：

#figure(
  [
    ```rust
    pub fn check_signals(
        thr: &Thread,
        uctx: &mut UserContext,
        restore_blocked: Option<SignalSet>,
    ) -> bool {
        crate::syscall::ptrace_wait_if_stopped(thr);

        let pid = thr.proc_data.proc.pid();
        if crate::syscall::ptrace_is_traced(pid) {
            let mask = !thr.signal.blocked();
            if let Some(sig) = thr.signal.dequeue_signal(&mask) {
                let signo = sig.signo();
                if crate::syscall::ptrace_intercept_signal(thr, signo) {
                    return true;
                }
                do_exit(signo as i32, true);
            }
        }

        // 继续正常信号处理流程
        // ...
    }
    ```
  ],
  caption: [信号检查中的 ptrace 拦截],
) <lst:ptrace-intercept>

被调试进程在检查信号前，先判断是否处于 ptrace 停止状态。如果调试器尚未恢复进程，进程会在 `ptrace_wait_if_stopped` 中阻塞。调试器通过 `PTRACE_CONT` 或相关命令恢复进程后，信号检查继续。

接下来，内核判断进程是否正在被调试。被调试进程收到未被屏蔽的信号时，调用 `ptrace_intercept_signal` 通知调试器。调试器可以检查信号、修改信号或决定是否传递。如果调试器处理了信号，函数返回 `true`，内核不再继续处理；如果调试器选择传递信号，函数返回 `false`，内核按正常流程处理信号。

=== 被忽略信号的特殊处理

ptrace 还有一条特殊规则：如果信号被目标进程设为忽略，调试器仍然应该看到该信号。moss 在发送信号时检查这一条件：

#figure(
  [
    ```rust
    if thread.signal.process().signal_ignored(signo)
        && crate::syscall::ptrace_mark_signal_stop(pid, signo)
    {
        return Ok(());
    }
    send_signal_thread_inner(&task, thread, sig);
    ```
  ],
  caption: [被忽略信号的 ptrace 处理],
) <lst:ptrace-ignored-signal>

即使信号被忽略，只要进程正在被调试，内核仍会标记该信号并触发 ptrace 停止。调试器可以观察到所有发送给被调试进程的信号，而不会因为进程忽略信号而漏掉通知。

这一机制对调试器观察进程行为至关重要。例如，守护进程通常忽略 `SIGHUP`，但调试器仍需要知道进程收到了该信号。

== 其他系统调用的完善

=== sys_pause 的实现

`pause` 系统调用使进程暂停执行，直到收到任意信号。moss 新增了这一系统调用：

#figure(
  [
    ```rust
    pub fn sys_pause(uctx: &mut UserContext) -> AxResult<isize> {
        let curr = current();
        let thr = curr.as_thread();

        // pause 总是返回 -EINTR
        uctx.set_retval(-LinuxError::EINTR.code() as usize);

        block_on(poll_fn(|cx| {
            if check_signals(thr, uctx, None) {
                return Poll::Ready(());
            }
            let _ = curr.poll_interrupt(cx);
            Poll::Pending
        }));

        Err(AxError::Interrupted)
    }
    ```
  ],
  caption: [pause 系统调用的实现],
) <lst:sys-pause>

`pause` 在信号到达前一直阻塞，收到信号后总是返回 `EINTR`。它常用于等待子进程状态变化或定时器到期。

=== sys_rt_sigtimedwait 的修正

原有实现在 `sigtimedwait` 被信号处理函数中断时返回 0，但正确的返回值应该是 `EINTR`。moss 修正了这一语义：

#figure(
  [
    ```rust
    let Ok(sig) = block_on(future::timeout(timeout, fut)) else {
        // 超时
        signal.set_blocked(old_blocked);
        return Err(AxError::WouldBlock);
    };
    let Some(sig) = sig else {
        // 被信号处理函数中断
        return Err(AxError::Interrupted);
    };
    ```
  ],
  caption: [sigtimedwait 的返回值修正],
) <lst:sigtimedwait-fix>

等待超时时返回 `EAGAIN`，被信号处理函数中断时返回 `EINTR`。这与 Linux 的语义一致。

同时，moss 还修正了 `sigtimedwait` 在恢复屏蔽集时的时机。无论是正常返回、超时还是被中断，都应该在返回前恢复原来的屏蔽集，避免屏蔽状态错乱。

== 本章小结

moss 在早期信号框架的基础上，在信号生命周期的几个关键环节进行了针对性增强：发送前的权限检查，停止与继续的状态管理，调试器的信号拦截，父进程的状态通知。

权限检查实现了 POSIX 规定的 UID 匹配规则，防止非法信号发送。停止与继续机制通过 `ProcessWaitStatus` 状态机、`wait_for_continue` 阻塞等待和 `stop_event` 通知，使进程能够真正暂停和恢复执行，支持 Shell 的作业控制功能。ptrace 拦截点使调试器能够截获和控制信号传递，支持 GDB 等调试工具。父进程通知完善了 `wait` 系统调用的语义，使父进程能够及时获知子进程的停止和继续状态。

这些增量修改共涉及内核代码约 250 行（`kernel/src/task/signal.rs` 增加 91 行，`kernel/src/syscall/signal.rs` 增加 159 行），覆盖了信号从发送到处理的完整路径，使 moss 能够支持 Shell 作业控制、GDB 调试和 POSIX 兼容的进程管理工具，通过了 ltp 信号测试集中的 `kill`、`sigaction`、`sigprocmask`、`sigsuspend`、`pause` 等测例。
