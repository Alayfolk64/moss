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

StarryOS 已经提供了可运行的任务调度骨架：`axtask` 管理任务状态和运行队列，`axsched` 用 RR 调度器按时间片轮转，多核下每个 CPU 有独立就绪队列，Futex 的 WAIT/WAKE、位掩码唤醒、重新排队和 robust list 也已实现，定时器与子进程等待接口也在。这部分 moss 主要是在 StarryOS 的基础上进行补充的。

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

这些修改共同作用于任务离开 CPU 和重新获得 CPU 的过程。其中最难处理的是通知与状态转换可能同时发生：任务检查完条件、尚未登记 Waker 时，事件可能已经到达；任务准备进入阻塞时，通知也可能先一步发生。下面的叙述会先用任务状态、抢占和上下文切换说明调度基础，再围绕这一竞争窗口讨论通用阻塞唤醒机制，最后分析定时等待、子进程等待和 Futex 的具体处理。

== 任务调度机制

=== 任务状态与状态转换

任务调度围绕四种状态展开。调度器不需要理解任务正在等待哪一种资源，只需判断任务当前属于以下四种中的哪种状态：

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
        // 调度器通过该字段判断任务当前能否运行
        state: AtomicU8,
        // ...
    }
    ```
  ],
  caption: [任务状态的数据结构],
) <lst:task-state>

`state` 使用原子整数保存任务状态。定时等待、Futex 和子进程等待各自维护不同的业务条件，但要重新获得 CPU，都必须让任务从 `Blocked` 回到 `Ready`。调度器只处理这一状态转换，并不判断具体等待条件是否已经满足。四种状态之间的转换关系如图所示。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3.2.1.svg", width: 38%)
    ]
  ],
  caption: [TaskState 的状态转换关系],
) <fig:task-state>

新任务先以 `Ready` 状态进入就绪队列，被调度器选中后变为 `Running`。主动让出 CPU 的任务仍具备运行条件，因此会回到 `Ready`；等待资源的任务则进入 `Blocked`，由 `unblock_task` 恢复为 `Ready`。任务执行结束后进入 `Exited`，并在资源回收后离开状态机。moss 的定时等待、Futex 和子进程等待都通过这组状态转换重新接入调度器。

状态修改通过原子状态和运行队列中的唤醒检查配合完成。多个通知同时到达时，唤醒路径应该只允许仍处于 `Blocked` 的任务重新进入就绪队列。

=== 时间片轮转与抢占

任务按照 RR 策略轮流使用 CPU。多核环境下，每个 CPU 都有自己的就绪队列，用来保存处于 `Ready` 状态的任务。调度器从队头选择下一个任务，任务的默认时间片是 5 个时钟滴答；时间片用完后，任务回到队尾，CPU 转交给后面的任务。

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

时间片归零后，内核先记录抢占请求，等执行离开禁止抢占的临界区再调度。任务如果在修改运行队列、处理中断或持有关键锁时被换出，其他 CPU 可能看到尚未完成的中间状态。延迟抢占把上下文切换放到状态一致的位置，运行队列和锁的处理也能先行结束。

如果当前没有普通任务可运行，CPU 会进入 idle 任务（没有普通任务时运行的空闲任务），等待新的任务或中断到来。

=== 任务选择与上下文切换

前两节已经说明了任务状态如何变化，以及时间片耗尽后如何发起抢占。本节从重新调度真正开始的位置继续：当前任务的状态处理完毕后，`resched` 从本 CPU 的就绪队列中选出下一个任务；如果队列为空，就让 idle 任务暂时接管 CPU。

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

选出任务后，`switch_to` 先将它标记为 `Running`，再切换任务现场。这里的“现场”是任务暂停前的运行位置。每个任务都有一个 `TaskContext`，其中保存内核栈指针、需要保留的寄存器和返回位置。切换时，当前 CPU 的这些值被保存到旧任务中，再从新任务的 `TaskContext` 恢复。旧任务以后再次被选中时，就能接着原来的位置运行。

如 @fig:context-switch-layout 所示，两个任务的 `TaskContext` 分别位于各自的内核栈中，`switch_to` 把 prev 的寄存器值写出，再从 next 读入；其中 `satp` 让切换完成的瞬间就生效了新的地址空间。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-context-switch.svg", width: 90%)
    ]
  ],
  caption: [TaskContext 布局与 switch_to 的现场传递方向],
) <fig:context-switch-layout>

对于用户任务，moss 在创建任务时把该进程的页表根写入 `TaskContext`，保证切换后使用正确的地址空间。用户态寄存器则保存在 `UserContext`（用户程序暂停时的寄存器记录）中。至此，从 3.2.2 的抢占请求到完成切换，可以连成下面这条流程：

#align(center)[
  设置待抢占标志 → 到达安全调度位置 → 选择下一任务 → 保存旧现场 → 恢复新现场
]

=== CPU 亲和性与调度接口

每个任务都保存一份 CPU 亲和性掩码 `cpumask`，用来限制它可以在哪些 CPU 上运行。例如，掩码只包含 CPU 0 和 CPU 2 时，任务就不会进入其他 CPU 的就绪队列。

新任务默认可以使用所有已经启用的 CPU。任务创建或被唤醒时，调度器在 `cpumask` 允许的范围内轮转选择一个 CPU，再把任务加入该 CPU 的就绪队列。

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
  caption: [在亲和性范围内选择 CPU],
) <lst:select-run-queue>

这段逻辑按 CPU 编号轮转，并跳过亲和性掩码不允许的 CPU。它沿用原有的多运行队列设计，moss 在这一部分的实际修改主要位于用户位图解析和设置后的任务迁移。

用户传入的亲和性范围是一张位图，每一位对应一个 CPU。moss 修正了这张位图的解析过程：内核按照实际配置的 CPU 数量读取内容，再把其中被置为 1 的位置写入任务掩码。用户程序通过 Linux 的 `sched_getaffinity` 查询范围，通过 `sched_setaffinity` 修改范围。

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

新的掩码保存后还要检查任务当前所在的 CPU。如果该 CPU 仍被允许，任务可以继续运行；如果已经被排除，moss 会把任务迁移到另一个允许的运行队列。新的设置因此会立即改变任务实际运行的位置，而不只是留下一个供查询的数值。

用户调用 `sched_yield` 主动让出 CPU 后，moss 将当前任务放回就绪队列，再由调度器选择其他任务运行。这个接口适合任务暂时没有紧急工作、希望让其他就绪任务先执行的情况。

== 阻塞与唤醒机制

moss 延续 StarryOS 的 Future/Waker 模型。这套机制的实际好处在于：timerfd、pidfd、Futex 等来源各不相同的等待，都能挂到同一套唤醒路径上，`block_on` 也因此能以统一方式暂停和恢复任务，不需要为每类等待单独实现阻塞逻辑。代价是每个等待路径都得正确维护通知状态和双重检查，否则竞争窗口就会漏掉通知。这正是本节要讨论的。

=== 事件等待的基本流程

调度器只知道任务处于 `Blocked`，并不知道任务在等什么。事件对象和等待者之间还需要一套通知协议。moss 沿用 Future 表示”尚未完成、之后还要继续检查的操作”，用 Waker 记录”条件可能变化时如何通知当前任务”。

以读取管道为例，任务先检查缓冲区。已有数据就直接读取；没有数据则把自己的 Waker 登记到管道中，再由 Future 返回“暂时未完成”。另一个任务写入数据后，管道调用保存的 Waker，通知读任务继续检查。

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

收到通知后还要重新检查条件。Waker 只表示“情况可能有变化”，并不保证资源一定属于当前任务。例如多个读任务同时被通知时，管道中的数据可能已经被其中一个任务取走，其他任务仍需继续等待。

=== Waker 与 PollSet

Waker 是 StarryOS 留给事件对象的任务唤醒入口。事件对象不必直接操作运行队列，只需保存等待者提供的 Waker，并在状态发生变化时调用 `wake()`。moss 继续使用这一接口，把新增的 timerfd、pidfd 和 Futex 等等待路径接到同一套任务唤醒流程上。

`PollSet` 管理一组 Waker，管道、eventfd、timerfd、pidfd 和终端等对象都通过它登记等待者。moss 增加 `has_waiters`，没有等待者时直接返回；真正唤醒时，先在锁内取出并清空本轮 Waker，再到锁外逐个调用。内部锁只保护集合操作，不覆盖可能继续触发调度的 `wake()`。

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

`take_wakers` 在取出 Waker 的同时清空集合，因此后续事件不会再次使用这一批旧记录。

PollSet 只负责“有人需要重新检查了”，不会替业务对象判断条件是否真的满足。管道仍要检查缓冲区，子进程等待仍要检查状态，Futex 也要检查通知标记。

=== block_on 与通知竞争处理

Future 通过反复轮询判断操作是否完成，而同步系统调用需要在结果尚未产生时暂停当前任务。`block_on` 连接这两种执行方式：Future 返回 `Ready` 时，系统调用取得结果；返回 `Pending` 时，当前任务准备阻塞，把 CPU 交给其他任务。任务选择和现场切换仍由 3.2.3 的调度路径完成。

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

Future 返回 `Pending` 与通知到达可能发生在相邻时刻。Waker 先把 `woke` 设为真，再尝试唤醒任务；阻塞路径则在改变任务状态前检查这个标记。通知先到时，任务不再进入 `Blocked`，只让出一次 CPU 后重新轮询。阻塞先完成时，`unblock_task` 再把任务放回就绪队列。`woke` 记录“本轮轮询后是否已经收到通知”，补上了 Future 状态与任务状态之间的空隙。

@fig:block-on-race 对比了这两条路径的时序差异。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-block-on-race.svg", width: 90%)
    ]
  ],
  caption: [block_on 正常阻塞路径与 woke 标记覆盖竞争路径的时序对比],
) <fig:block-on-race>

正常事件之外，信号也可以结束等待。`interruptible` 同时检查原来的 Future 和任务的信号状态。收到未屏蔽信号后，等待提前结束，上层系统调用再决定返回 `EINTR`、保存剩余时间或按信号规则重启。

等待因超时或信号提前结束后，还必须删除本轮登记。否则后续事件仍可能找到旧 Waker，既产生无效唤醒，也会干扰唤醒数量统计。PollSet 会清空已经取出的 Waker，Futex 则按等待项精确清理，具体过程将在 3.5 节说明。

== 通用阻塞机制的应用

3.3 节建立了“检查条件、登记 Waker、阻塞、收到通知后复查”的通用流程。本节选择两类应用说明这套流程如何落地。定时等待关注截止时间是否到达，子进程等待关注是否出现可报告的状态。二者的业务条件不同，但都不能把一次唤醒直接当作最终结果，任务恢复后仍要重新检查条件。

=== 定时等待与截止时间

定时等待先根据系统调用参数选择时间来源，再把等待目标统一换算为截止时间。单调时钟只向前推进，适合表示“再等待多久”；实时时钟对应系统日历时间，适合指定一个实际时刻。

时间参数分为相对时长和绝对时间。相对时长需要加到当前时间上，绝对时间本身就是截止时间；如果截止时间已经到达，等待立即结束。统一转换后，后续逻辑只需比较“当前时间是否达到截止时间”。同样的区分也用于修正后文 `FUTEX_WAIT_BITSET` 的超时语义。

`nanosleep` 使用单调时钟完成相对休眠。`clock_nanosleep` 还允许选择时钟，并通过 `TIMER_ABSTIME` 表示绝对时间。休眠期间如果收到未屏蔽信号，moss 会结束本次等待，向用户返回 `EINTR`，并写回尚未用完的时间。

=== 定时器队列与到期处理

StarryOS 的定时事件可以按截止时间触发，但定时器重新设置后，旧事件可能仍留在队列中。仅凭截止时间无法判断取出的条目是否仍然有效。moss 改用 `BTreeMap` 保存定时器，并用截止时间 `deadline` 和递增序号 `seq` 组成唯一的 `TimerKey`。后台定时任务每次取出最早到期的条目，到期后更新定时器状态并发送相应信号。

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
        // 按到期顺序保存全部定时事件
        timers: BTreeMap<TimerKey, Entry>,
    }
    ```
  ],
  caption: [定时器队列的关键数据结构],
) <lst:timer-key>

同一时刻可能有多个定时器到期，`seq` 使每个事件都有独立的键。重新设置或取消定时器时，内核删除旧键；后台任务取出事件后，还要确认它与定时器当前保存的键一致。只有匹配的事件才会触发信号，重新设置前留下的旧条目会被忽略。新增更早的定时器时，系统还会通知后台任务重新计算下一次等待时间。

每个线程还有一个时间管理对象 `TimeManager`，用来分别累计用户态和内核态运行时间。实际时间定时器 `ITIMER_REAL` 按经过时间发送 `SIGALRM`；用户时间定时器 `ITIMER_VIRTUAL` 只统计用户态时间；性能分析定时器 `ITIMER_PROF` 同时统计用户态与内核态时间。周期定时器到期后重新装载，一次性定时器到期后停止。

=== 子进程等待的通知路径

第二章已经介绍了子进程状态、等待目标和接口语义。本节只讨论等待过程如何接入任务阻塞与唤醒。父进程暂时找不到可报告状态时，需要登记通知入口并暂停运行；子进程状态变化后，再将父进程唤醒。

父进程调用 wait 时先筛选目标子进程，再检查是否已有可报告状态。如果暂时没有结果，它就把 Waker 登记到 `child_exit_event` 并进入等待。子进程退出或状态发生变化时会触发这个事件，父进程恢复运行后重新读取状态。

#figure(
  [
    ```rust
    // 先检查是否已有子进程状态可以报告
    match check_children().transpose() {
        Some(result) => Poll::Ready(result),
        None => {
            // 暂无结果，登记父进程的 Waker
            child_exit_event.register(cx.waker());
            // 登记后复查，覆盖同时发生的状态变化
            match check_children().transpose() {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        }
    }
    ```
  ],
  caption: [父进程等待子进程状态的实现],
) <lst:child-wait-register>

父进程因此不必循环查询子进程。状态变化触发 Waker，父进程恢复后重新执行 `check_children`，再根据第二章所述的目标和选项判断是否已有结果。代码中的登记后复查与 3.3.1 的通用流程一致，用来覆盖“首次检查结束、Waker 尚未登记”期间发生的子进程状态变化。

== Futex 同步机制

=== 基本思想与数据结构

Futex 把无竞争的同步留在用户态。线程先通过原子变量尝试获得锁，只有发现资源被占用时才进入内核等待。因此，大多数没有竞争的加锁和解锁不需要系统调用。

进入内核后，moss 使用 `FutexKey` 定位等待队列。进程私有 Futex 以用户地址为键；共享匿名映射和文件映射还要结合共享后端与区域偏移，保证不同进程即使把同一页映射到不同虚拟地址，也能找到同一个 Futex。

每个 Futex 表项保存等待队列和 `owner_dead` 状态。为了分清正常唤醒、超时和信号中断，moss 将单个等待项组织为：

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

元组中的 `AtomicBool` 是 `notified`，记录等待项是否已经被选中唤醒；`u32` 保存 bitset；`PollSet` 保存任务的唤醒入口。业务状态与唤醒动作由此分开：`notified` 防止通知丢失或重复处理，PollSet 负责让任务重新参与调度。

@fig:futex-queue 展示了 FutexKey 如何定位 WaitQueue，以及每个等待项三个字段的分工——`notified` 记录通知结果，bitset 过滤唤醒目标，PollSet 持有唤醒入口。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-futex-queue.svg", width: 92%)
    ]
  ],
  caption: [FutexKey、WaitQueue 与等待项三元组的结构关系],
) <fig:futex-queue>

=== WAIT 与 WAKE

等待者只有在用户地址仍等于期望值时才应睡眠。系统调用入口先做一次快速检查，不相等就返回 `EAGAIN`；准备入队时，内核在持有等待队列锁的情况下再次读取用户值。第二次检查把用户值判断与入队操作放进同一个受保护区间，条件已经改变时，本次调用不再阻塞。

条件仍成立时，内核创建等待项并进入可中断、可超时的 Future。`FUTEX_WAKE` 从队列中选择指定数量的等待者，先把 `notified` 设为真，再调用 PollSet；被正常唤醒的等待项同时从队列删除，不会继续占用下一次 wake 的名额。

与 3.3.3 不同，这里额外处理 Futex 等待项入队后的竞争：`notified` 由 `FUTEX_WAKE` 写入，并在登记 PollSet 前后复查。它记录的是单个 Futex 等待项的通知结果，后续位掩码唤醒、超时清理和重新排队都会保留或处理该状态。

=== 位掩码唤醒与重新排队

位掩码唤醒与重新排队也使用新增的 `notified` 状态和退出清理。`FUTEX_WAIT_BITSET` 为等待者附带位掩码，`FUTEX_WAKE_BITSET` 只唤醒 bitset 与目标 mask 有交集的任务，从而避免叫醒无关线程。

`FUTEX_REQUEUE` 先唤醒一部分任务，再把其余等待项移动到另一个 Futex 队列。`FUTEX_CMP_REQUEUE` 在此之前还会比较原地址的用户值，不匹配时返回 `EAGAIN`。移动过程中保留 `notified`、bitset 和 PollSet，不重新登记 Waker。

这类操作常用于 pthread 条件变量。广播时不必把所有线程同时叫醒争抢互斥量，可以先唤醒少量线程，再把其他等待者转移到互斥量对应的队列。如 @fig:futex-requeue 所示，操作结束后原队列只剩余未被处理的等待者，其余项带着原有的 `notified`、bitset 和 PollSet 整体迁移到目标 Futex 队列。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-futex-requeue.svg", width: 88%)
    ]
  ],
  caption: [FUTEX_REQUEUE 将部分等待者唤醒、其余等待者转移至另一 Futex 队列],
) <fig:futex-requeue>

=== 超时、中断与等待修正

Futex 等待可能正常收到通知，也可能先超时或被信号打断。moss 用 `interruptible(timeout(...))` 组合这三条路径。超时返回 `ETIMEDOUT`，信号中断返回 `EINTR`；两种提前结束都会按照 `notified` 的对象地址精确删除当前等待项。

原有实现没有把 `FUTEX_WAIT` 和 `FUTEX_WAIT_BITSET` 的时间语义分开。moss 按 Linux 规则处理：前者接收相对时长，后者接收绝对截止时间，并根据 `FUTEX_CLOCK_REALTIME` 选择实时时钟或单调时钟。

独立通知状态解决了“通知是否已经到达”的判断问题，前后复查覆盖登记窗口，退出清理则删除已经失效的等待项。三者分别处理通知丢失、重复处理和队列残留。相关修改修复了 LoongArch64 上 `futex_wait`、pthread cancellation 和部分休眠测试卡住的问题。

=== robust list 与线程退出清理

线程持有互斥量时如果直接退出，其他线程可能永远等不到解锁，从而造成死锁。针对这种情况，moss 完善了退出时的 Futex 清理。用户态线程库通过 `set_robust_list` 把 robust list 头指针交给内核，链表记录该线程可能持有的 Futex。

线程退出时，moss 按 `futex_offset` 找到对应 Futex，将 `owner_dead` 设为真，并唤醒一个等待者。被唤醒线程收到 `EOWNERDEAD` 后，由用户态线程库检查共享数据并恢复锁的一致性。链表遍历受 `ROBUST_LIST_LIMIT` 限制，损坏的用户链表不会让内核无限循环。

线程退出还会清零 `clear_child_tid`，并唤醒这个地址对应的 Futex。这条路径支持 `pthread_join` 等线程回收操作。

== 本章小结

StarryOS 已经具备 RR 调度、任务状态转换、上下文切换和基础事件等待，moss 没有另建一套调度器。本章的修改集中在调度器之外、重新进入调度器之前的那段路径：等待者登记状态，事件到来后留下通知，任务恢复时复查条件，等待提前结束时清理旧记录。

PollSet 缩短了持锁唤醒路径，`block_on` 用 `woke` 协调通知与阻塞，定时器用 `TimerKey` 识别仍然有效的事件，子进程等待在登记 Waker 后复查状态，Futex 则通过 `notified`、双重检查和精确删除处理竞争。它们最终回到同一条流程：条件不满足时阻塞，条件可能变化时唤醒，复查成功后转为就绪，再由调度器安排运行。
