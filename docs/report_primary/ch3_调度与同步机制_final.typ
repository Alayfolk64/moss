#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(3)

= 调度与同步机制

任务等待事件的过程看起来直接：检查条件、登记唤醒入口、进入阻塞，事件到了再恢复。但如果通知恰好在"条件检查完、Waker 还没登记"这个窗口里到达时，等待方错过了这次通知，之后就没有人再来唤醒它了。本章大多数改动都对准这个竞争窗口，以及由此派生出的几个具体问题：超时后旧等待项还留在队列里，子进程状态变化和父进程登记 Waker 之间有空隙，Futex 通知与入队之间存在同样的竞争。

moss 沿用 StarryOS 的 RR 调度框架，改动集中在进入阻塞和重新唤醒之间的那段路径。任务在就绪、运行和阻塞之间循环，执行完毕后退出调度流程，整体关系如图所示。

#figure(
  [
    #align(center)[
      #image("./pictrues/ch3-总图.svg", width: 95%)
    ]
  ],
  caption: [调度、阻塞与唤醒的总体循环],
) <fig:scheduling-overall-flow>


== 概述

StarryOS 已经提供了可运行的任务调度骨架：`axtask` 管理任务状态和运行队列，`axsched` 用 RR 调度器按时间片轮转，多核下每个 CPU 有独立就绪队列，Futex 的 WAIT/WAKE、位掩码唤醒、重新排队和 robust list 也已实现，定时器与子进程等待接口也在。

问题集中在事件交错时的状态处理上。Futex 等待项只保存了 Waker 和位掩码，没有独立的通知状态，超时或被信号中断后旧记录还留在队列里——内核说不清通知究竟有没有到达，失效等待项也会占用后续唤醒名额。`FUTEX_WAIT_BITSET` 的绝对截止时间被当相对时长用，定时器重新设置后旧事件照样留在全局队列。子进程状态变化与父进程登记 Waker 之间，也同样有一段通知可能悄悄溜走的窗口。

moss 保留 RR 调度框架，改动集中在三个环节：等待者如何登记，通知如何与阻塞状态配合，等待结束后如何清理记录。对应关系如下：

#figure(
  table(
    columns: (0.9fr, 2.35fr, 2.65fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([内容], [StarryOS 基线], [moss 的增量]),
    [任务调度], [任务状态、RR 时间片和 CPU 亲和性], [沿用调度框架，修正亲和性位图解析],
    [阻塞唤醒], [Future、Waker 和等待队列], [调整 PollSet 唤醒过程，补充条件复查和等待项清理],
    [定时处理], [基础休眠和进程定时器], [区分相对与绝对时间，使用 TimerKey 过滤旧事件],
    [Futex], [WAIT、WAKE、位掩码和重新排队], [加入独立通知状态，修正超时语义和退出清理],
    [子进程等待], [退出事件与基础等待], [将状态变化接入 Waker，并在登记后复查条件],
  ),
  caption: [调度与同步机制的主要增量],
) <tab:scheduling-overview>

这些修改共同作用于任务离开 CPU 和重新获得 CPU 的过程。其中最难处理的是通知与状态转换可能同时发生：任务检查完条件、尚未登记 Waker 时，事件可能已经到达；任务准备进入阻塞时，通知也可能先一步发生。下面先简要说明调度基础，再重点讨论 moss 在阻塞唤醒机制上的改进。

== 任务调度机制

=== 任务状态与调度流程

StarryOS 的任务调度围绕四种状态展开：`Running`（正在 CPU 上执行）、`Ready`（可运行，在队列中等待）、`Blocked`（等待事件）、`Exited`（已结束）。新任务以 `Ready` 进入就绪队列，被调度器选中后变为 `Running`。主动让出 CPU 的任务回到 `Ready`；等待资源的任务进入 `Blocked`，由 `unblock_task` 恢复为 `Ready`。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3.2.1.svg", width: 38%)
    ]
  ],
  caption: [TaskState 的状态转换关系],
) <fig:task-state>

任务按 RR 策略轮转使用 CPU，默认时间片为 5 个时钟滴答。时钟中断到来时更新时间片，计数归零时先设置待抢占标志，等代码离开禁止抢占的临界区后再执行重新调度。这样可以避免在持锁或中断处理时切换任务。

#align(center)[
  #box(
    inset: 8pt,
    stroke: 0.6pt + gray,
    radius: 3pt,
  )[
    时钟中断 → 时间片减一 → 设置待抢占标志 → 离开临界区 → 重新调度
  ]
]

多核环境下，每个 CPU 都有自己的就绪队列。调度器从队头选择下一个任务，时间片用完后任务回到队尾，CPU 转交给后面的任务。如果队列为空，就让 idle 任务接管 CPU。上下文切换时，当前 CPU 的寄存器值被保存到旧任务的 `TaskContext` 中，再从新任务的 `TaskContext` 恢复。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-context-switch.svg", width: 90%)
    ]
  ],
  caption: [TaskContext 布局与 switch_to 的现场传递方向],
) <fig:context-switch-layout>

对于用户任务，moss 在创建任务时把该进程的页表根写入 `TaskContext`，保证切换后使用正确的地址空间。用户态寄存器则保存在 `UserContext` 中。整个流程可以串成：

#align(center)[
  设置待抢占标志 → 到达安全调度位置 → 选择下一任务 → 保存旧现场 → 恢复新现场
]

=== CPU 亲和性修正

每个任务保存一份 CPU 亲和性掩码，限制它可以在哪些 CPU 上运行。moss 修正了用户位图解析过程：按实际配置的 CPU 数量读取内容，把置为 1 的位写入任务掩码。

#figure(
  [
    ```rust
    // 解析用户传入的亲和性位图
    for cpu_id in 0..axconfig::plat::CPU_NUM {
        if user_mask[cpu_id / 8] & (1 << (cpu_id % 8)) != 0 {
            cpu_mask.set(cpu_id, true);
        }
    }

    // 保存新掩码
    let curr = current().clone();
    curr.set_cpumask(cpu_mask);

    // 当前 CPU 已不在范围内时，迁移任务
    if !cpu_mask.get(axhal::percpu::this_cpu_id()) {
        let migration_task = TaskInner::new(
            move || migrate_entry(curr),
            "migration-task".into(),
            4096,
        ).into_arc();
        current_run_queue().migrate_current(migration_task);
    }
    ```
  ],
  caption: [CPU 亲和性掩码的解析与迁移],
) <lst:set-affinity>

新掩码保存后还要检查任务当前所在 CPU，如果已被排除，会将任务迁移到另一个允许的运行队列。用户程序通过 `sched_getaffinity` 查询范围，通过 `sched_setaffinity` 修改范围。

== 阻塞与唤醒机制的核心改进

这部分是 moss 的重点改动。StarryOS 已经有了基本的 Future/Waker 模型，但在通知与阻塞交错时容易出问题。moss 的改进围绕一个核心思路展开：#strong[在关键路径上加标记，覆盖竞争窗口]。

=== 事件等待的基本流程

调度器只知道任务处于 `Blocked`，并不知道任务在等什么。事件对象和等待者之间需要一套通知协议。moss 沿用 Future 表示"尚未完成、之后还要继续检查的操作"，用 Waker 记录"条件可能变化时如何通知当前任务"。

以读取管道为例，任务先检查缓冲区。已有数据就直接读取；没有数据则把自己的 Waker 登记到管道中，再由 Future 返回"暂时未完成"。另一个任务写入数据后，管道调用保存的 Waker，通知读任务继续检查。

#figure(
  [
    #align(center)[
      #box(
        inset: 9pt,
        stroke: 0.6pt + gray,
        radius: 3pt,
      )[
        检查条件 → 保存 Waker → 再次检查 → 等待通知

        #v(0.75em)
        事件发生 → 调用 Waker → Future 再次执行 → 重新检查条件
      ]
    ]
  ],
  caption: [事件登记与通知的基本流程],
) <fig:event-wait-flow>

关键步骤是登记 Waker 后的第二次条件检查。假如数据恰好在第一次检查结束后到达，此时写入方还找不到等待者，无法调用 Waker。登记完成后立刻复查缓冲区，读任务仍能看到已经到达的数据，不会因为漏掉那次通知而一直等待。

收到通知后还要重新检查条件。Waker 只表示"情况可能有变化"，并不保证资源一定属于当前任务。例如多个读任务同时被通知时，管道中的数据可能已经被其中一个任务取走，其他任务仍需继续等待。

=== PollSet：锁外唤醒

StarryOS 的 `PollSet` 管理一组 Waker，但唤醒时可能在持锁情况下调用 `wake()`。moss 的改进很直接：加个原子标志快速判断有没有等待者，然后把 Waker 取出来到锁外再调用。

#figure(
  [
    ```rust
    // vendor/axpoll/src/lib.rs
    pub struct PollSet {
        inner: Lazy<Mutex<Inner>>,
        has_waiters: AtomicBool,  // 快速判断有没有等待者
    }

    pub fn wake(&self) -> usize {
        if !self.has_waiters.load(Ordering::Acquire) {
            return 0;  // 没人等，直接返回
        }
        let (wakers, len) = self.inner.lock().take_wakers();  // 取出
        self.has_waiters.store(false, Ordering::Release);
        drop(guard);  // 释放锁
        for waker in wakers.iter().take(len) {
            unsafe { waker.assume_init_read() }.wake();  // 锁外唤醒
        }
        len
    }
    ```
  ],
  caption: [PollSet 的锁外唤醒],
) <lst:pollset-wake>

这个改动看起来简单，但解决了调度路径被锁保护的问题。

=== block_on：用标记补窗口

这是最关键的改动。问题在于：Future 返回 `Pending` 和通知到达可能几乎同时发生。如果通知先到，但任务还没进入 `Blocked` 状态，通知就白费了。

moss 的解决办法是加一个 `woke` 标记：

#figure(
  [
    ```rust
    // vendor/axtask/src/future/mod.rs
    struct AxWaker {
        task: WeakAxTaskRef,
        woke: SpinNoIrq<bool>,  // 关键：记录通知有没有到
    }

    // 通知来的时候：
    impl Wake for AxWaker {
        fn wake_by_ref(self: &Arc<Self>) {
            if let Some(task) = self.task.upgrade() {
                *self.woke.lock() = true;  // 先标记
                select_run_queue(&task).unblock_task(task, false);  // 再唤醒
            }
        }
    }

    // 准备阻塞的时候：
    match fut.as_mut().poll(&mut cx) {
        Poll::Pending => {
            let woke = axwaker.woke.lock();
            if !*woke {
                rq.blocked_resched(woke);  // 没通知，安全阻塞
            } else {
                drop(woke);
                crate::yield_now();  // 通知先到了，只让出 CPU
            }
        }
        Poll::Ready(output) => break output,
    }
    ```
  ],
  caption: [woke 标记补上竞争窗口],
) <lst:block-on-waker>

逻辑很清楚：通知先标记再唤醒，阻塞先检查标记再改状态。这样通知无论什么时候到，都能被正确处理。@fig:block-on-race 展示了两条路径的时序对比。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-block-on-race.svg", width: 90%)
    ]
  ],
  caption: [两条路径的时序对比：正常阻塞 vs 通知先到],
) <fig:block-on-race>

=== 定时器：用序号去重

StarryOS 的定时器用截止时间做键，但定时器重新设置后，旧事件还留在队列里。moss 的办法是给每个事件加个递增的序号：

#figure(
  [
    ```rust
    struct TimerKey {
        deadline: Duration,  // 什么时候触发
        seq: u64,           // 第几个事件
    }

    impl Ord for TimerKey {
        fn cmp(&self, other: &Self) -> Ordering {
            // 先按截止时间排序，再按序号排序
            self.deadline.cmp(&other.deadline)
                .then_with(|| self.seq.cmp(&other.seq))
        }
    }

    struct AlarmRuntime {
        next_seq: u64,                      // 生成序号
        timers: BTreeMap<TimerKey, Entry>,  // 按时间顺序存
    }
    ```
  ],
  caption: [TimerKey = 截止时间 + 序号],
) <lst:timer-key>

重新设置定时器时删除旧键，后台任务取出事件后确认序号匹配才触发。这样旧事件就被自动过滤掉了。同一时刻可能有多个定时器到期，`seq` 使每个事件都有独立的键。新增更早的定时器时，系统还会通知后台任务重新计算下一次等待时间。

每个线程还有一个时间管理对象 `TimeManager`，用来分别累计用户态和内核态运行时间。实际时间定时器 `ITIMER_REAL` 按经过时间发送 `SIGALRM`；用户时间定时器 `ITIMER_VIRTUAL` 只统计用户态时间；性能分析定时器 `ITIMER_PROF` 同时统计用户态与内核态时间。周期定时器到期后重新装载，一次性定时器到期后停止。

时间参数分为相对时长和绝对时间。相对时长需要加到当前时间上，绝对时间本身就是截止时间；如果截止时间已经到达，等待立即结束。`nanosleep` 使用单调时钟完成相对休眠。`clock_nanosleep` 还允许选择时钟，并通过 `TIMER_ABSTIME` 表示绝对时间。休眠期间如果收到未屏蔽信号，moss 会结束本次等待，向用户返回 `EINTR`，并写回尚未用完的时间。

=== 子进程等待：登记后复查

父进程等待子进程状态时，可能在检查完"没有可报告状态"和登记 Waker 之间，子进程正好退出了。moss 的解决办法是登记后立即复查：

#figure(
  [
    ```rust
    match check_children().transpose() {
        Some(result) => Poll::Ready(result),  // 有状态，直接返回
        None => {
            child_exit_event.register(cx.waker());  // 登记 Waker
            match check_children().transpose() {     // 立即复查
                Some(result) => Poll::Ready(result),  // 补上窗口期的变化
                None => Poll::Pending,               // 确实没有，可以等了
            }
        }
    }
    ```
  ],
  caption: [登记后复查，覆盖中间窗口],
) <lst:child-wait-register>

这个模式和 `block_on` 的思路一样：关键操作前后都检查一遍，确保不漏掉通知。

第二章已经介绍了子进程状态、等待目标和接口语义。子进程调用 `exit` 退出后，父进程通过 wait 系列系统调用回收。父进程需要筛选目标子进程（通过 PID、进程组或特定条件），再检查是否有可报告的状态变化（退出、停止、继续）。如果暂时没有结果，父进程需要等待子进程状态发生变化。

子进程状态变化后，内核会触发 `child_exit_event`，唤醒父进程。父进程恢复后重新执行 `check_children`，根据等待选项判断是否已有结果。如果设置了 `WNOHANG`，即使没有结果也立即返回；否则继续等待直到有可报告的状态。

== Futex 同步机制的改进

Futex 的改动最复杂，因为它要处理正常唤醒、超时、信号中断、位掩码过滤、重新排队等多种情况。StarryOS 的等待项只保存了 `(Waker, bitset)`，分不清这些情况。

=== 独立通知状态

moss 把等待项扩展成三元组：`(Arc<AtomicBool>, u32, Arc<PollSet>)`。第一个字段 `notified` 记录这个等待项有没有被选中唤醒。

#figure(
  [
    ```rust
    // kernel/src/task/futex.rs
    pub struct WaitQueue {
        // 每个等待项：(通知状态, 位掩码, 唤醒入口)
        queue: SpinNoIrq<VecDeque<(Arc<AtomicBool>, u32, Arc<PollSet>)>>,
    }

    pub fn wake(&self, count: usize, mask: u32) -> usize {
        let mut woke = 0;
        self.queue.lock().retain(|(waiter, bitset, poll)| {
            if woke >= count || (bitset & mask) == 0 {
                true  // 不唤醒，留在队列
            } else {
                waiter.store(true, Ordering::Release);  // 标记已通知
                poll.wake();                            // 唤醒任务
                woke += 1;
                false  // 从队列移除
            }
        });
        woke
    }
    ```
  ],
  caption: [Futex 等待项的三元组结构],
) <lst:futex-wait-queue>

有了独立的 `notified` 状态，就能准确判断：超时是因为没人唤醒，还是被唤醒后又超时了。@fig:futex-queue 展示了这个结构。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-futex-queue.svg", width: 92%)
    ]
  ],
  caption: [FutexKey 定位队列，三元组记录状态],
) <fig:futex-queue>

等待者只有在用户地址仍等于期望值时才应睡眠。系统调用入口先做一次快速检查，不相等就返回 `EAGAIN`；准备入队时，内核在持有等待队列锁的情况下再次读取用户值。第二次检查把用户值判断与入队操作放进同一个受保护区间，条件已经改变时，本次调用不再阻塞。

条件仍成立时，内核创建等待项并进入可中断、可超时的 Future。等待过程中会反复检查 `notified` 标记，登记 Waker 前后都要复查。这和 `block_on` 的处理思路一致：通知可能在任何时刻到达，用标记把窗口补上。

=== 精确清理

超时或信号中断时，要把这个等待项从队列里删掉。moss 按 `notified` 的对象地址精确匹配：

#figure(
  [
    ```rust
    match result {
        Ok(Ok(())) => Ok(true),  // 正常唤醒
        Ok(Err(_)) | Err(_) => {
            // 超时或信号中断：删掉这一项
            let mut queue = self.queue.lock();
            queue.retain(|(waiter, ..)| !Arc::ptr_eq(waiter, &notified));
            Err(if result.is_ok() { AxError::TimedOut }
                else { AxError::Interrupted })
        }
    }
    ```
  ],
  caption: [按对象地址精确删除],
) <lst:futex-timeout-cleanup>

这样不会误删其他等待者，也不会留下失效的记录。

正常事件之外，信号也可以结束等待。`interruptible` 同时检查原来的 Future 和任务的信号状态。收到未屏蔽信号后，等待提前结束，上层系统调用再决定返回 `EINTR`、保存剩余时间或按信号规则重启。

等待因超时或信号提前结束后，还必须删除本轮登记。否则后续事件仍可能找到旧 Waker，既产生无效唤醒，也会干扰唤醒数量统计。moss 用 `Arc::ptr_eq` 按对象地址精确匹配，确保只删除当前这个等待项。

=== 位掩码唤醒

位掩码唤醒用来优化条件变量等场景。`FUTEX_WAIT_BITSET` 为等待者附带位掩码，`FUTEX_WAKE_BITSET` 只唤醒 bitset 与目标 mask 有交集的任务，从而避免叫醒无关线程。

例如一个 Futex 上可能有多个不同的等待者：有的在等读事件（bitset = 0x01），有的在等写事件（bitset = 0x02）。读事件发生时，只需用 mask = 0x01 唤醒读等待者，写等待者继续睡眠。这样可以减少无效的上下文切换。

`wake` 函数遍历队列时会检查每个等待项的 bitset：`(bitset & mask) == 0` 的跳过，有交集的才唤醒。被唤醒的等待项从队列移除，不会被重复处理。

=== 重新排队时保留状态

`FUTEX_REQUEUE` 把一些等待者从一个 Futex 移到另一个。moss 在移动时保留完整的三元组，不重新登记 Waker。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-futex-requeue.svg", width: 88%)
    ]
  ],
  caption: [重新排队时整体迁移，保留所有状态],
) <fig:futex-requeue>

这样被移动的等待者不会丢失通知，也不会被重复唤醒。

这类操作常用于 pthread 条件变量。广播时不必把所有线程同时叫醒争抢互斥量，可以先唤醒少量线程，再把其他等待者转移到互斥量对应的队列。假设有 10 个线程在条件变量上等待，广播时只唤醒 1 个，其余 9 个转移到互斥量队列。第一个线程获得互斥量后处理完毕释放锁，互斥量队列中的下一个线程再被唤醒。这样避免了 10 个线程同时被唤醒后争抢同一个锁的情况。

`FUTEX_CMP_REQUEUE` 在移动前还会比较原地址的用户值，不匹配时返回 `EAGAIN`。这是为了防止条件变量和互斥量状态不一致时的错误移动。

=== 时间语义修正

原来的实现没分清 `FUTEX_WAIT` 和 `FUTEX_WAIT_BITSET` 的时间参数。moss 按 Linux 规则处理：前者用相对时长，后者用绝对截止时间，还可以通过 `FUTEX_CLOCK_REALTIME` 选择时钟类型。

相对时长需要加到当前时间上得到截止时间，绝对时间本身就是截止时间。如果截止时间已经过了，等待立即结束返回 `ETIMEDOUT`。单调时钟只向前推进，不受系统时间调整影响，适合表示"再等待多久"；实时时钟对应系统日历时间，适合指定一个实际时刻。

这个修正看起来简单，但对于依赖精确超时语义的程序很重要。例如 `pthread_cond_timedwait` 使用绝对时间，如果被当成相对时长处理，超时时间就会偏差很大。

=== robust list 处理

线程持有互斥量时退出，其他线程可能永远等不到解锁。moss 完善了退出清理：遍历线程的 robust list，找到对应的 Futex，标记 `owner_dead` 并唤醒一个等待者。被唤醒的线程收到 `EOWNERDEAD` 后，由用户态线程库检查并恢复锁的一致性。

#figure(
  [
    ```rust
    // 线程退出时清理 robust list
    for entry in robust_list.iter() {
        let futex_addr = entry.futex_offset;
        let futex = find_futex(futex_addr);

        // 标记持有者已死亡
        futex.owner_dead.store(true, Ordering::Release);

        // 唤醒一个等待者，让它知道锁的持有者已死
        futex.wq.wake(1, u32::MAX);
    }

    // 清零 clear_child_tid，通知等待该线程的其他线程
    if let Some(tid_addr) = clear_child_tid {
        let futex = find_futex(tid_addr);
        futex.wq.wake(1, u32::MAX);
    }
    ```
  ],
  caption: [线程退出时的 Futex 清理],
) <lst:robust-list-cleanup>

链表遍历受 `ROBUST_LIST_LIMIT` 限制，损坏的用户链表不会让内核无限循环。线程退出还会清零 `clear_child_tid`，并唤醒这个地址对应的 Futex。这条路径支持 `pthread_join` 等线程回收操作。

== 本章小结

StarryOS 已经有了基本的调度和事件等待，moss 的改进集中在通知与阻塞交错时的状态处理：

- *PollSet*：锁外唤醒，避免调度路径被锁保护
- *block_on*：`woke` 标记协调通知与阻塞的竞争窗口
- *定时器*：`(deadline, seq)` 双键去重，过滤旧事件
- *子进程等待*：登记后复查，覆盖中间窗口
- *Futex*：独立 `notified` 状态 + 精确删除 + 状态保留

这些改进的核心思路是：#strong[在关键路径加标记，操作前后都检查]。通知先标记再唤醒，阻塞先检查再改状态，退出时按标记精确清理。它们修复了 LoongArch64 上 `futex_wait`、pthread cancellation 和部分休眠测试卡住的问题。
