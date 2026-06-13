#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(3)

= 调度与同步机制

调度负责安排任务使用 CPU，同步负责处理任务之间的等待与通知。两者通过任务状态连接起来：任务等待资源时离开运行队列，条件满足后再由调度器恢复运行。本章先介绍任务调度，再沿着阻塞、定时和唤醒这条主线说明 Futex 与子进程等待。

== 概述

StarryOS 已经提供了完整的任务调度骨架。底层 `axtask` 管理任务状态和运行队列，`axsched` 提供轮转调度器，Future、Waker 和等待队列负责阻塞与唤醒。在这套基础上，系统可以运行多个任务，也能处理主动让出、时间片抢占和事件等待。

moss 没有重新设计调度算法，主要工作集中在调度器上层。原有代码在复杂等待场景中容易出现通知丢失、失效等待项残留和超时含义不一致等问题；子进程等待也主要覆盖退出事件，对停止、继续和 `waitid` 支持不足。

因此，本章的重点不是介绍一套新的 CPU 调度器，而是说明 moss 如何把已有调度框架与 Linux 等待语义接得更稳。主要增量包括 PollSet 唤醒路径调整、可中断等待、定时器失效项过滤、Futex 等待队列修正，以及子进程状态等待的补充。

== 任务调度机制

=== 任务状态与运行队列

调度器不需要理解任务正在等待哪一种资源，只需要维护四种基本状态：

- `Running`：正在某个 CPU 上运行。
- `Ready`：可以运行，正在就绪队列中等待。
- `Blocked`：条件尚未满足，暂时不能运行。
- `Exited`：执行已经结束，等待最后的资源回收。

#figure(
  [
    ```rust
    pub enum TaskState {
        Running = 1, // 正在 CPU 上执行
        Ready   = 2, // 可以运行，正在队列中等待
        Blocked = 3, // 正在等待事件，暂时不能运行
        Exited  = 4, // 已经结束，等待资源回收
    }

    pub struct TaskInner {
        // 状态决定任务是否能进入运行队列
        state: AtomicU8,
        // 位图中的 1 表示允许任务使用对应 CPU
        cpumask: SpinNoIrq<AxCpuMask>,
        // 记录任务当前或即将运行的 CPU
        cpu_id: AtomicU32,
        // ...
    }
    ```
  ],
  caption: [任务状态与 CPU 亲和性字段],
) <lst:task-state-cpumask>

`state` 使用原子整数保存任务状态，`cpumask` 记录任务允许使用的 CPU，`cpu_id` 则记录任务当前或即将运行的位置。它们构成了调度器判断“任务能否运行、应当放到哪里”的基本信息。

#figure(
  [
    #align(center)[
      #box(
        inset: 9pt,
        stroke: 0.6pt + gray,
        radius: 3pt,
      )[
        创建或唤醒 → `Ready` → 调度选中 → `Running`

        #v(0.7em)
        时间片耗尽或主动让出：`Running` → `Ready`

        #v(0.7em)
        等待事件：`Running` → `Blocked` → `Ready`

        #v(0.7em)
        执行结束：`Running` → `Exited`
      ]
    ]
  ],
  caption: [任务的主要状态转换],
) <fig:task-state>

新任务先以 `Ready` 状态进入就绪队列，被调度器选中后变为 `Running`。主动让出 CPU 的任务仍具备运行条件，因此会回到 `Ready`；等待资源的任务则进入 `Blocked`，在收到通知前不再参与调度。

多核环境下，每个 CPU 有自己的运行队列。任务内部保存一份 CPU 亲和性掩码，用来表示它允许在哪些 CPU 上运行。例如，掩码只包含 CPU 0 和 CPU 2 时，任务就不会被放到其他 CPU 的运行队列。

新任务默认允许在所有已经启用的 CPU 上运行。任务创建或被唤醒时，调度器从亲和性掩码允许的 CPU 中轮转选择一个，再把任务加入对应的运行队列。状态修改使用原子操作，同一个阻塞任务即使同时收到多个通知，也只会被成功加入运行队列一次。

#figure(
  [
    ```rust
    fn select_run_queue_index(cpumask: AxCpuMask) -> usize {
        loop {
            // 在所有 CPU 编号之间轮转
            let index = RUN_QUEUE_INDEX.fetch_add(1, Ordering::SeqCst)
                % axconfig::plat::CPU_NUM;
            // 只选择亲和性掩码允许的 CPU
            if cpumask.get(index) {
                return index;
            }
        }
    }
    ```
  ],
  caption: [从亲和性允许的 CPU 中选择运行队列],
) <lst:select-run-queue>

代码先轮转取得一个 CPU 编号，再用 `cpumask.get(index)` 判断该 CPU 是否允许当前任务运行。不允许就继续寻找，直到选中可用的运行队列。

=== 运行队列与轮转调度

当前工程使用 RR，也就是时间片轮转调度。可运行任务依次排在就绪队列中，调度器从队头选择下一个任务。任务的默认时间片是 5 个时钟滴答，时间片用完后回到队尾，CPU 转交给后面的任务。

时钟中断每到来一次，`scheduler_timer_tick`（调度器的时钟节拍处理函数）就更新当前任务的时间片。计数归零时，内核先设置待抢占标志，而不是立刻在中断处理或持锁位置切换任务。等代码离开禁止抢占的临界区后，再执行真正的重新调度。

#align(center)[
  #box(
    inset: 8pt,
    stroke: 0.6pt + gray,
    radius: 3pt,
  )[
    时钟中断 → 时间片减一 → 设置待抢占标志
    #h(0.7em)
    → 离开临界区 → 重新调度
  ]
]

这种延后切换的做法避免了任务在修改运行队列、处理中断或持有关键锁时被强行换出。如果当前没有普通任务可运行，CPU 会进入 idle 任务（没有普通任务时运行的空闲任务），等待新的任务或中断到来。

=== 调度的触发与上下文切换

一次调度通常由时间片耗尽、任务主动让出 CPU 或任务进入阻塞触发。触发后，内核先处理当前任务的状态，再从运行队列中选择下一个任务，最后完成上下文切换。

#figure(
  table(
    columns: (1.35fr, 2fr, 2.8fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([触发方式], [状态变化], [含义]),
    [时间片耗尽], [`Running → Ready`], [任务仍可运行，但本轮时间片已经用完],
    [`yield_now()`], [`Running → Ready`], [任务主动把 CPU 交给其他就绪任务],
    [阻塞等待], [`Running → Blocked`], [继续运行也无法推进，需要等待事件],
  ),
  caption: [三种常见调度触发方式],
) <tab:schedule-trigger>

时间片耗尽和主动让出时，当前任务仍能继续运行，因此会回到就绪队列。任务阻塞时则变为 `Blocked`，不会重新入队。处理完当前任务后，`resched` 从运行队列中取出一个 `Ready` 任务；如果队列为空，就选择 idle 任务。

#figure(
  [
    ```rust
    fn resched(&mut self) {
        // 从就绪队列选择任务；队列为空时运行 idle 任务
        let next = self.scheduler.lock().pick_next_task()
            .unwrap_or_else(|| unsafe {
                IDLE_TASK.current_ref_raw().get_unchecked().clone()
            });

        self.switch_to(crate::current(), next);
    }

    fn switch_to(&mut self, prev: CurrentTask, next: AxTaskRef) {
        // 将选中的任务设为正在运行
        next.set_state(TaskState::Running);

        // 保存当前任务现场，恢复下一个任务现场
        let prev_ctx = prev.ctx_mut_ptr();
        let next_ctx = next.ctx_mut_ptr();
        unsafe { (*prev_ctx).switch_to(&*next_ctx) };
    }
    ```
  ],
  caption: [从选择任务到切换现场的主要流程],
) <lst:task-context-switch>

这里的“现场”可以理解为任务暂停前的运行位置。每个任务都有一个 `TaskContext`，其中保存内核栈指针、需要保留的寄存器和返回位置。`switch_to` 先把当前 CPU 上的这些值写入旧任务，再从新任务的 `TaskContext` 中恢复。旧任务以后再次被选中时，就能接着上次暂停的位置运行。

对于用户任务，moss 在创建任务时还会把该进程的页表根写入 `TaskContext`，保证切换后使用正确的地址空间。用户态寄存器由 `UserContext`（用户程序暂停时的寄存器记录）保存。这样，一次完整调度就形成了清楚的顺序：

#align(center)[
  触发调度 → 处理当前任务状态 → 选择下一个任务 → 保存旧现场 → 恢复新现场
]

=== CPU 亲和性与调度接口

为了让用户程序限制任务可以使用哪些 CPU，StarryOS 已经在任务中保存了 3.2.1 介绍的亲和性掩码，并提供基础的查询和设置入口。moss 沿用这套设计，修正了用户位图的解析过程：内核按照平台实际配置的 CPU 数量读取位图，再把其中被置为 1 的位置写入任务掩码。查询和设置对应 Linux 的 `sched_getaffinity` 与 `sched_setaffinity` 接口。

#figure(
  [
    ```rust
    // 用户位图中的每一位对应一个 CPU
    for cpu_id in 0..axconfig::plat::CPU_NUM {
        if user_mask[cpu_id / 8] & (1 << (cpu_id % 8)) != 0 {
            cpu_mask.set(cpu_id, true);
        }
    }

    // 保存新的亲和性范围
    let curr = current().clone();
    curr.set_cpumask(cpu_mask);

    // 当前 CPU 已不在范围内时，将任务迁移到允许的 CPU
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
  caption: [CPU 亲和性掩码的设置与任务迁移],
) <lst:set-affinity>

修改掩码后，如果当前 CPU 仍被允许，任务可以继续运行；如果当前 CPU 已被排除，内核会把任务迁移到另一个允许的运行队列。这样，用户程序可以把任务固定在指定 CPU 上，减少不必要的跨核迁移，也满足 Linux 程序对 CPU 绑定接口的需要。

对于主动让出 CPU，moss 沿用 StarryOS 已有的调度流程。用户调用 `sched_yield` 后，当前任务回到就绪队列，调度器转而选择其他任务运行。这个接口适合任务暂时没有紧急工作、希望让其他就绪任务先执行的情况。

== 阻塞与唤醒机制

=== 事件等待的基本流程

moss 使用事件等待来处理“当前条件尚未满足”的任务。以读取管道为例，任务先检查缓冲区中是否有数据；有数据就直接读取，没有数据就登记自己的唤醒信息，并进入 `Blocked` 状态。此时调度器会选择其他就绪任务运行。

当另一个任务向管道写入数据后，管道会通知等待者。被通知的任务从 `Blocked` 变回 `Ready`，重新进入运行队列。它再次获得 CPU 后重新检查缓冲区，确认数据已经可读，再继续执行读取操作。

#figure(
  [
    #align(center)[
      #box(
        inset: 9pt,
        stroke: 0.6pt + gray,
        radius: 3pt,
      )[
        检查条件 → 登记等待者 → 再次检查 → 进入 `Blocked`

        #v(0.75em)
        事件发生 → 发出通知 → 任务变为 `Ready` → 重新检查条件
      ]
    ]
  ],
  caption: [事件等待与唤醒的基本流程],
) <fig:event-wait-flow>

流程中安排了两次条件检查。第一次检查可以让已经满足条件的任务直接继续；登记唤醒信息后再检查一次，可以覆盖事件恰好在登记期间发生的情况。任务被唤醒后还会继续检查条件，因为一次通知只表示状态发生了变化，最终能否继续执行仍由当前条件决定。

=== Waker 与 PollSet

Waker 保存了唤醒任务所需的信息。事件对象不必直接操作运行队列，只需保存等待者提供的 Waker，并在状态发生变化时调用 `wake()`。Waker 会把对应任务从 `Blocked` 改为 `Ready`，再交给合适的运行队列。

`PollSet` 用于管理一组 Waker。管道、eventfd、timerfd、pidfd 和终端等对象都可以通过同一组接口登记和通知等待者。moss 对这条路径做了两处调整：增加 `has_waiters` 快速判断，没有等待者时不再进入锁；唤醒时先把 Waker 从集合中取出并清空记录，再在锁外逐个调用。这样既缩短了持锁时间，也避免同一批等待者被后续事件重复唤醒。

#figure(
  [
    ```rust
    pub struct PollSet {
        inner: Lazy<Mutex<Inner>>,
        // 没有等待者时，wake 可以直接返回
        has_waiters: AtomicBool,
    }

    pub fn wake(&self) -> usize {
        if !self.has_waiters.load(Ordering::Acquire) {
            return 0;
        }
        // 取出并清空本轮登记的 Waker
        let (wakers, len) = self.inner.lock().take_wakers();
        self.has_waiters.store(false, Ordering::Release);
        // 真正唤醒时已经不再持有内部锁
        for waker in wakers.iter().take(len) {
            unsafe { waker.assume_init_read() }.wake();
        }
        len
    }
    ```
  ],
  caption: [PollSet 取出并唤醒等待者],
) <lst:pollset-wake>

这里的 `take_wakers` 会清空内部集合。后续事件不会再次使用旧 Waker，真正调用 `wake()` 时也不再持有 PollSet 的内部锁。

PollSet 只负责“有人需要重新检查了”，不会替业务对象判断条件是否真的满足。管道仍要检查缓冲区，子进程等待仍要检查状态，Futex 也要检查通知标记。

=== 从任务阻塞到重新调度

同步代码通过 `block_on` 等待 Future。Future 返回 `Pending` 后，`block_on` 把当前任务设为 `Blocked` 并调用运行队列重新调度；Waker 被触发后，任务恢复为 `Ready`，下一次获得 CPU 时继续轮询 Future。

#figure(
  [
    ```rust
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(task) = self.task.upgrade() {
            // 先记录通知已经到达
            *self.woke.lock() = true;
            // 再把阻塞任务放回合适的运行队列
            select_run_queue(&task).unblock_task(task, false);
        }
    }

    match future.poll(&mut context) {
        // 没收到通知，当前任务可以安全进入 Blocked
        Poll::Pending if !*woke => run_queue.blocked_resched(woke),
        // 通知先到，不再阻塞，只让其他任务先运行一次
        Poll::Pending => yield_now(),
        Poll::Ready(output) => return output,
    }
    ```
  ],
  caption: [Waker 与 block_on 的阻塞唤醒配合],
) <lst:block-on-waker>

`blocked_resched` 负责把当前任务变为 `Blocked`，`unblock_task` 再把收到通知的任务恢复为 `Ready`。如果通知已经先到，代码不会继续阻塞，而是让出 CPU 后重新检查 Future。

阻塞与通知可能几乎同时发生。为此，任务 Waker 内部保存 `woke` 标记：如果通知先到，任务不会再真正睡下，而是让出一次 CPU 后继续检查；如果阻塞先完成，通知路径再把它放回运行队列。这个标记处理的是任务状态切换窗口，业务条件本身仍由等待对象复查。

正常通知之外，等待还可能被信号中断。任务对象保存中断状态和对应 Waker，`interruptible` 将其与原来的 Future 一起轮询。收到未屏蔽信号后，等待提前结束，上层系统调用再决定返回 `EINTR`、保存剩余时间或按信号规则重启。

=== 条件复查与等待项清理

可靠的等待需要同时处理两件事：不漏掉通知，也不留下已经失效的等待者。常用顺序是先检查条件，登记 Waker 后再检查一次；任务醒来时仍要重新检查，因为一次通知可能同时唤醒多个任务，而资源只够其中一个使用。

如果等待因超时或信号结束，原先登记的记录也必须删除。否则后续事件可能把一次已经返回用户态的等待算入唤醒数量，甚至唤醒正在执行其他工作的旧 Waker。moss 在通用 PollSet 和具体等待队列中分别清理这类状态，Futex 的处理会在 3.5 节进一步说明。

== 定时与超时机制

=== 时间来源与截止时间

定时等待首先要区分时间来源。单调时钟只向前推进，适合表示“再等多久”；实时时钟对应系统日历时间，修改系统时间后可能跳变。`clock_nanosleep` 和 Futex 的部分操作允许用户选择时钟，内核需要按接口规则解释。

时间参数还分相对时长和绝对截止时间。相对时间可以直接用于等待；绝对时间则要先减去当前时刻，得到剩余时长。截止时间已经过去时，等待应立即结束，不能把过去的时间再次当成一段新的时长。

=== 定时器队列与到期唤醒

moss 的进程定时器使用按 `(deadline, seq)` 排序的 `BTreeMap`。`deadline` 决定触发顺序，`seq` 用来区分截止时间相同的多个事件。后台 alarm task 总是读取最早到期的条目，没有定时器时则等待新事件通知。

#figure(
  [
    ```rust
    struct TimerKey {
        // 定时事件应当触发的时刻
        deadline: Duration,
        // 区分截止时间相同的多个事件
        seq: u64,
    }

    struct AlarmRuntime {
        // 为新事件生成不重复的序号
        next_seq: u64,
        // BTreeMap 按 TimerKey 自动排列触发顺序
        timers: BTreeMap<TimerKey, Entry>,
    }
    ```
  ],
  caption: [定时器队列的关键数据结构],
) <lst:timer-key>

同一截止时间可能对应多个定时器，因此不能只用 `deadline` 作为键。递增的 `seq` 让每个定时事件都有独立身份，也方便取消和过滤旧配置。

每个正在使用的 interval timer 保存自己的 `TimerKey`。重新设置或取消定时器时，旧 Key 会从队列删除；后台任务取出条目后还会检查它是否仍是当前生效的 Key。即使旧事件因并发时序留在队列中，也会在触发前被过滤，不会产生重复信号。

新增了更早的定时器后，`EVENT_NEW_TIMER` 会叫醒 alarm task，让它重新计算下一次截止时间。这样，后台任务不会一直睡到原来的较晚时刻。

=== 任务休眠与可中断超时

`nanosleep` 使用单调时钟，`clock_nanosleep` 还支持实时时钟和 `TIMER_ABSTIME`。moss 调整了休眠期间的信号检查：循环中发现未屏蔽信号后立即返回 `EINTR`，并把剩余时间写回 `rem`。这项修改用于解决 LoongArch64 上休眠和线程取消长时间不返回的问题。

当前休眠路径通过检查时钟并反复 `yield_now()` 等待截止时间。它能够让其他任务运行，也能及时响应信号，但没有像普通事件等待那样长期保持 `Blocked`。Futex 等异步等待则使用 `timeout` Future，把事件完成和截止时间放在同一次等待中。

=== 运行时间统计与周期定时器

每个线程的 `TimeManager` 分别记录用户态时间和内核态时间。进入内核、返回用户态时会切换计时状态，定期轮询再把经过时间累加到对应字段。这些数据供 `times`、资源统计和进程定时器使用。

三类 interval timer 的计时范围不同：`ITIMER_REAL` 按实际经过时间触发 `SIGALRM`，`ITIMER_VIRTUAL` 只累计用户态时间并触发 `SIGVTALRM`，`ITIMER_PROF` 同时累计用户态和内核态时间并触发 `SIGPROF`。周期定时器到期后重新装载 interval，一次性定时器则停止。

== Futex 同步机制

=== 基本思想与数据结构

Futex 把无竞争的同步留在用户态。线程先通过原子变量尝试获得锁，只有发现资源被占用时才进入内核等待。这样，大多数没有竞争的加锁和解锁不需要系统调用。

进入内核后，`FutexKey` 用来定位等待队列。进程私有 Futex 以用户地址为键；共享匿名映射和文件映射还要结合共享后端与区域偏移，保证不同进程即使把同一页映射到不同虚拟地址，也能找到同一个 Futex。

每个 Futex 表项保存等待队列和 `owner_dead` 状态。moss 将单个等待项组织为：

#figure(
  [
    ```rust
    pub struct WaitQueue {
        // 每个等待项依次保存：
        // 通知状态、唤醒位掩码和任务唤醒入口
        queue: SpinNoIrq<
            VecDeque<(Arc<AtomicBool>, u32, Arc<PollSet>)>
        >,
    }
    ```
  ],
  caption: [Futex 等待队列的数据结构],
) <lst:futex-wait-queue>

元组中的 `AtomicBool` 是 `notified`，表示是否已经收到通知；`u32` 保存 bitset；`PollSet` 保存任务的唤醒入口。三项分别回答“是否醒了、由哪类 wake 唤醒、怎样叫醒任务”。

=== WAIT 与 WAKE

`FUTEX_WAIT` 的含义是：只有用户地址仍等于期望值时才睡眠。系统调用入口先做一次快速检查，不相等就返回 `EAGAIN`。准备入队时，内核在持有等待队列锁的情况下再次读取用户值；如果条件已经改变，本次调用不再阻塞。

条件仍成立时，内核创建等待项并进入可中断、可超时的 Future。`FUTEX_WAKE` 从队列中选择指定数量的等待者，先把 `notified` 设为真，再调用 PollSet。正常唤醒的等待项会同时从队列删除，不会继续占用下一次 wake 的名额。

#figure(
  [
    ```rust
    // 第一次检查：通知可能已经先于登记到达
    if notified.load(Ordering::Acquire) {
        return Poll::Ready(());
    }

    // 条件未满足，登记当前任务的 Waker
    poll.register(cx.waker());

    // 第二次检查：覆盖“检查与登记之间”的竞争窗口
    if notified.load(Ordering::Acquire) {
        Poll::Ready(())
    } else {
        Poll::Pending
    }
    ```
  ],
  caption: [Futex 登记 Waker 前后检查通知状态],
) <lst:futex-double-check>

第一次检查处理已经到达的通知，登记后的第二次检查覆盖两步之间的竞争窗口。只要唤醒者先写入 `notified`，等待者就不会因为错过一次 Waker 调用而一直阻塞。

等待者在登记 Waker 前后各检查一次 `notified`。通知无论发生在登记前还是登记后，至少有一次检查能看到它。这是 moss 对原有单纯保存 Waker 方案的重要修正。

=== 位掩码唤醒与重新排队

`FUTEX_WAIT_BITSET` 为等待者附带位掩码，`FUTEX_WAKE_BITSET` 只唤醒 bitset 与目标 mask 有交集的任务。它让多个逻辑等待条件可以共用同一个 Futex 地址，同时避免叫醒无关线程。

`FUTEX_REQUEUE` 先唤醒一部分任务，再把其余等待项移动到另一个 Futex 队列。`FUTEX_CMP_REQUEUE` 在此之前还会比较原地址的用户值，不匹配时返回 `EAGAIN`。移动过程中保留 `notified`、bitset 和 PollSet，不重新登记 Waker。

这类操作常用于 pthread 条件变量。广播时不必把所有线程同时叫醒争抢互斥量，可以先唤醒少量线程，再把其他等待者转移到互斥量对应的队列。

=== 超时、中断与等待修正

Futex 等待可能正常收到通知，也可能先超时或被信号打断。moss 用 `interruptible(timeout(...))` 组合这三条路径。超时返回 `ETIMEDOUT`，信号中断返回 `EINTR`；两种提前结束都会按照 `notified` 的对象地址精确删除当前等待项。

原有实现没有把 `FUTEX_WAIT` 和 `FUTEX_WAIT_BITSET` 的时间语义分开。moss 按 Linux 规则处理：前者接收相对时长，后者接收绝对截止时间，并根据 `FUTEX_CLOCK_REALTIME` 选择实时时钟或单调时钟。

独立通知状态、前后两次检查和退出时清理共同解决了几个问题：通知不会轻易落在登记空隙中，失效等待项不会消耗后续唤醒数量，正常唤醒和超时也不会重复处理同一个任务。这些修改同时修复了 LoongArch64 上 `futex_wait`、pthread cancellation 和部分休眠测试卡住的问题。

=== robust list 与线程退出清理

线程持有互斥量时如果直接退出，其他线程可能永远等不到解锁。用户态线程库通过 `set_robust_list` 把 robust list 头指针交给内核，链表记录该线程可能持有的 Futex。

线程退出时，moss 按 `futex_offset` 找到对应 Futex，将 `owner_dead` 设为真，并唤醒一个等待者。被唤醒线程收到 `EOWNERDEAD` 后，由用户态线程库检查共享数据并恢复锁的一致性。链表遍历受 `ROBUST_LIST_LIMIT` 限制，损坏的用户链表不会让内核无限循环。

线程退出还会清零 `clear_child_tid`，并唤醒这个地址对应的 Futex。这条路径支持 `pthread_join` 等线程回收操作。

== 子进程状态与等待

=== 子进程状态记录

进程退出后会成为僵尸进程，退出码暂时保存在进程对象中，等待父进程读取。StarryOS 原有 `waitpid` 主要处理这一类事件。moss 进一步在 `ProcessData` 中记录停止和继续状态，使父进程可以区分子进程已经退出、被作业控制信号暂停，以及收到 `SIGCONT` 后恢复运行。

退出、停止和继续都属于状态事件，但消费方式不同。退出状态通常在父进程读取后回收子进程；停止和继续不会结束子进程，只需要清除本次事件记录。未被当前等待选项选中的状态会继续保留。

=== 父进程的阻塞与唤醒

父进程调用 wait 时先筛选目标子进程，再检查是否已有可报告状态。没有结果并且不要求非阻塞返回时，父进程把 Waker 登记到 `child_exit_event`，随后进入阻塞。

#figure(
  [
    ```rust
    // 先检查是否已有子进程状态可以报告
    match check_children().transpose() {
        Some(result) => Poll::Ready(result),
        None => {
            // 暂无结果，登记父进程的 Waker
            child_exit_event.register(cx.waker());
            // 登记完成后再查一次，避免错过并发状态变化
            match check_children().transpose() {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        }
    }
    ```
  ],
  caption: [父进程登记等待前后的状态检查],
) <lst:child-wait-register>

这段代码与通用事件等待的规则一致：先检查状态，再登记 Waker，最后复查一次。子进程即使恰好在登记期间退出，父进程也能在第二次检查中看到结果。

子进程退出或状态变化后会调用对应事件的 `wake()`。父进程恢复运行后再次检查子进程状态，而不是把一次唤醒直接当成等待成功。这与 3.3 节的通用流程一致，也能处理多个子进程同时变化或一次通知对应多个等待者的情况。

等待过程由 `interruptible` 包装。收到未屏蔽信号后，系统调用可以返回 `EINTR` 或按重启规则重新执行，不会让父进程一直留在等待队列中。

=== wait 系列接口与等待选项

`waitpid` 的 pid 参数可以选择一个子进程、当前进程组、指定进程组或任意直接子进程。moss 将它统一转换为 `Pid`、`Pgid` 和 `Any` 三种内部目标，再对父进程的子进程集合进行筛选。没有符合条件的直接子进程时返回 `ECHILD`。

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

`wait4` 在等待结果之外返回资源使用信息。`waitid` 则使用 `siginfo_t` 描述状态，分别编码为 `CLD_EXITED`、`CLD_KILLED`、`CLD_STOPPED` 或 `CLD_CONTINUED`，比传统整数状态码更直接。

=== waitid 与 pidfd

moss 增加 `waitid`，支持 `P_ALL`、`P_PID`、`P_PGID` 和 `P_PIDFD`。前几种目标按全部子进程、PID 或进程组筛选；`P_PIDFD` 先通过文件描述符取得稳定的进程引用，再确认它属于调用进程的直接子进程。

pidfd 本身来自 StarryOS，moss 的增量是把它接入等待接口并补充 `/proc` fdinfo。普通 PID 以后可能被复用，pidfd 则持续引用原来的进程对象。非阻塞 pidfd 在状态尚未就绪时返回 `EAGAIN`，阻塞 pidfd 则沿用父进程事件等待流程。

`WNOWAIT` 允许父进程只查看状态而不立即回收目标。这样，多个管理组件可以先观察同一状态，再由负责回收的一方完成最后处理。

== 本章小结

StarryOS 已经具备 RR 调度、任务状态转换、上下文切换和基础事件等待。moss 沿用这套调度骨架，把工作重点放在阻塞后的处理：PollSet 缩短唤醒持锁路径，等待流程补上条件复查、信号中断和失效记录清理，定时器通过 `TimerKey` 过滤过期事件。

在这些通用机制之上，moss 修正了 Futex 的通知状态、队列清理、绝对超时和 robust list 退出处理，并扩展子进程停止、继续、`waitid` 与 pidfd 等待。各功能使用的是同一条主线：任务不满足条件时进入等待，事件或时间到达后被唤醒，再由调度器安排它继续执行。
