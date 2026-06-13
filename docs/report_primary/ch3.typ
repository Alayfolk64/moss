#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(3)

= 调度与同步机制

调度负责安排任务使用 CPU，同步负责处理任务之间的等待与通知。moss 没有重新设计 StarryOS 的调度器，而是在原有任务状态和运行队列之上，重点补强阻塞、唤醒、定时和 Futex 等路径。任务等待资源时离开运行队列，条件满足后再由调度器恢复运行，这条状态变化也是本章各项增量的共同基础。

== 概述

=== StarryOS 的已有基础

StarryOS 已经提供了完整的任务调度骨架。底层 `axtask` 管理任务状态和运行队列，`axsched` 使用 RR 调度器按时间片轮流运行任务。任务可以主动让出 CPU，也可以在时间片耗尽后被抢占；多核环境下，每个 CPU 有独立的运行队列和亲和性掩码。

等待方面，StarryOS 使用 Future、Waker 和等待队列连接任务与事件。任务等待 I/O、定时器或其他状态时可以进入阻塞，事件发生后再回到运行队列。系统也已经实现 Futex 的 WAIT、WAKE、位掩码唤醒、重新排队和 robust list，并提供基础的定时器与子进程等待接口。

=== base os 实现的主要不足

这套框架能够完成基本调度和等待，但在多种事件交错发生时，部分状态处理还不够严密。原有 Futex 等待项主要保存 Waker 和位掩码，等待超时或被信号中断后，旧记录可能继续留在队列中；条件检查与 Waker 登记之间也缺少独立通知状态，较难判断一次通知是否已经被等待者收到。

时间处理也有两处需要补充。`FUTEX_WAIT_BITSET` 的绝对截止时间曾被当作相对时长使用；定时器被重新设置后，旧事件也可能继续留在全局队列中。这样一来，后续到期处理无法只凭截止时间判断事件是否仍然有效。

子进程等待主要覆盖退出和僵尸回收。父进程还不能完整取得停止、继续等状态，也缺少 `waitid` 的目标选择和状态编码。pidfd 已经存在，但尚未接入这套等待流程。

=== moss 的完善范围

moss 保留 StarryOS 的 RR 调度框架，改动主要落在阻塞任务如何登记、收到通知和结束等待这些环节。对应关系如下：

#figure(
  table(
    columns: (1.4fr, 2.35fr, 2.65fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([内容], [StarryOS 基线], [moss 的增量]),
    [任务调度], [任务状态、RR 时间片和 CPU 亲和性], [沿用调度框架，修正亲和性位图解析],
    [阻塞唤醒], [Future、Waker 和等待队列], [调整 PollSet 唤醒过程，补充条件复查和等待项清理],
    [定时处理], [基础休眠和进程定时器], [区分相对与绝对时间，使用 TimerKey 过滤旧事件],
    [Futex], [WAIT、WAKE、位掩码和重新排队], [加入独立通知状态，修正超时语义和退出清理],
    [子进程等待], [`waitpid` 主要处理退出状态], [补充停止、继续、`waitid` 和 pidfd 等待],
  ),
  caption: [调度与同步机制的主要增量],
) <tab:scheduling-overview>

后文先介绍 moss 的任务调度骨架，再说明阻塞唤醒的通用路径，最后展开定时等待、子进程等待和 Futex 中的具体增量。

== 任务调度机制

=== 任务状态与状态转换

任务调度围绕四种状态展开。调度器不需要理解任务正在等待哪一种资源，只需判断任务当前属于以下哪种状态：

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
        // 调度器通过该字段判断任务当前能否运行
        state: AtomicU8,
        // ...
    }
    ```
  ],
  caption: [任务状态的数据结构],
) <lst:task-state>

`state` 使用原子整数保存任务状态。后续增加的定时等待、Futex 清理和子进程状态通知，最终都会落到这个字段上：继续等待时保持 `Blocked`，收到有效通知后改回 `Ready`。

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

新任务先以 `Ready` 状态进入就绪队列，被调度器选中后变为 `Running`。主动让出 CPU 的任务仍具备运行条件，因此会回到 `Ready`；等待资源的任务则进入 `Blocked`，在收到通知前不再参与调度。moss 的定时等待、Futex 和子进程等待都通过这组状态转换重新接入调度器。

状态修改使用原子操作。同一个阻塞任务即使同时收到多个通知，也只有第一次能把状态从 `Blocked` 改为 `Ready`，避免任务被重复加入就绪队列。这一点使 moss 后面补充的超时、信号和正常唤醒可以同时工作，而不会各自把同一任务放回队列。

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

这里没有在时间片归零时立刻切换任务，而是先留下待抢占标志，等内核离开临界位置后再调度。这样可以避免任务在修改运行队列、处理中断或持有关键锁时被强行换出。若在这些位置直接切换，运行队列可能只修改了一半，关键锁也无法及时释放；其他 CPU 或任务再次访问时，就可能出现任务重复入队、队列状态错乱或长时间等待。

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

选出任务后，`switch_to` 先将它标记为 `Running`，再切换任务现场。这里的“现场”可以理解为任务暂停前的运行位置。每个任务都有一个 `TaskContext`，其中保存内核栈指针、需要保留的寄存器和返回位置。切换时，当前 CPU 的这些值被保存到旧任务中，再从新任务的 `TaskContext` 恢复。旧任务以后再次被选中时，就能接着原来的位置运行。

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

这段代码先轮转取得一个 CPU 编号，再检查该 CPU 是否允许当前任务运行。不允许就继续寻找，直到找到合适的 CPU。

用户传入的亲和性范围是一张位图，每一位对应一个 CPU。moss 修正了这张位图的解析过程：内核按照平台实际配置的 CPU 数量读取内容，再把其中被置为 1 的位置写入任务掩码。用户程序通过 Linux 的 `sched_getaffinity` 查询范围，通过 `sched_setaffinity` 修改范围。

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

新的掩码保存后还要检查任务当前所在的 CPU。如果该 CPU 仍被允许，任务可以继续运行；如果已经被排除，moss 会把任务迁移到另一个允许的运行队列。这样，新的设置会立即影响任务实际运行的位置，而不只是留下一个供查询的数值。

用户调用 `sched_yield` 主动让出 CPU 后，moss 将当前任务放回就绪队列，再由调度器选择其他任务运行。这个接口适合任务暂时没有紧急工作、希望让其他就绪任务先执行的情况。

== 阻塞与唤醒机制

=== 事件等待的基本流程

当前条件尚未满足时，任务通过 Future 和 Waker 进入等待。moss 在这条路径上补上登记前后复查与失效等待项清理。以读取管道为例，任务先检查缓冲区中是否有数据；有数据就直接读取，没有数据就登记自己的唤醒信息，并进入 `Blocked` 状态。此时调度器会选择其他就绪任务运行。

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

这里特意安排了两次条件检查。第一次检查可以让已经满足条件的任务直接继续；登记唤醒信息后再检查一次，可以覆盖事件恰好在登记期间发生的情况。任务被唤醒后还会继续检查条件，因为一次通知只表示状态发生了变化，最终能否继续执行仍由当前条件决定。

=== Waker 与 PollSet

Waker 是 StarryOS 留给事件对象的任务唤醒入口。事件对象不必直接操作运行队列，只需保存等待者提供的 Waker，并在状态发生变化时调用 `wake()`。moss 继续使用这一接口，把新增的 timerfd、pidfd 和 Futex 等等待路径接到同一套任务唤醒流程上。

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

同步系统调用通过 `block_on` 等待 Future。Future 返回 `Pending` 后，`block_on` 把当前任务设为 `Blocked` 并调用运行队列重新调度；Waker 被触发后，任务恢复为 `Ready`，下一次获得 CPU 时继续轮询 Future。

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

阻塞与通知可能几乎同时发生。moss 保留任务 Waker 中的 `woke` 标记，并让新增等待代码遵守这项检查：如果通知先到，任务不会再真正睡下，而是让出一次 CPU 后继续检查；如果阻塞先完成，通知路径再把它放回运行队列。这个标记处理的是任务状态切换窗口，业务条件本身仍由等待对象复查。

正常通知之外，moss 还把信号中断接入等待过程。任务对象保存中断状态和对应 Waker，`interruptible` 将其与原来的 Future 一起轮询。收到未屏蔽信号后，等待提前结束，上层系统调用再决定返回 `EINTR`、保存剩余时间或按信号规则重启。

=== 条件复查与等待项清理

这部分改动主要解决两个问题：不能漏掉通知，也不能留下已经失效的等待者。具体顺序是先检查条件，登记 Waker 后再检查一次；任务醒来时仍要重新检查，因为一次通知可能同时唤醒多个任务，而资源只够其中一个使用。

如果等待因超时或信号结束，原先登记的记录也必须删除。否则后续事件可能把一次已经返回用户态的等待算入唤醒数量，甚至唤醒正在执行其他工作的旧 Waker。moss 在通用 PollSet 和具体等待队列中分别清理这类状态，Futex 的处理会在 3.5 节进一步说明。

== 阻塞与唤醒的具体应用

3.3 节介绍了通用流程，本节说明它在两类场景中的实际用法。定时等待以“到达指定时间”为继续条件，子进程等待以“子进程出现可报告状态”为继续条件。两者都会先判断条件；条件暂未满足时记录等待信息，时间到达或状态变化后再让任务继续执行。

=== 定时等待与截止时间

定时等待先根据系统调用参数选择时间来源，再把等待目标统一换算为截止时间。单调时钟只向前推进，适合表示“再等待多久”；实时时钟对应系统日历时间，适合指定一个实际时刻。

时间参数还分为相对时长和绝对时间。moss 对两者分别处理：相对时长需要加到当前时间上得到截止时间，绝对时间则可以直接作为截止时间使用。截止时间已经到达时，等待立即结束。这项区分也用于修正后文 `FUTEX_WAIT_BITSET` 的超时语义。

`nanosleep` 使用单调时钟完成相对休眠。`clock_nanosleep` 还允许选择时钟，并通过 `TIMER_ABSTIME` 表示绝对时间。休眠期间如果收到未屏蔽信号，moss 会结束本次等待，向用户返回 `EINTR`，并写回尚未用完的时间。

=== 定时器队列与到期处理

StarryOS 的定时事件可以按截止时间触发，但定时器重新设置后，旧事件可能仍留在队列中。moss 改用 `BTreeMap` 保存定时器，并用截止时间 `deadline` 和递增序号 `seq` 组成唯一的 `TimerKey`。后台定时任务（alarm task）每次取出最早到期的条目，到期后更新定时器状态并发送相应信号。

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

同一时刻可能有多个定时器到期，moss 增加的 `seq` 使每个事件都有独立的 Key。重新设置或取消定时器时，内核删除旧 Key；后台任务取出事件后还会确认它仍是当前生效的 Key，再执行到期处理。这样，已经作废的旧事件不会误触发信号。新增更早的定时器时，系统还会通知 alarm task 重新计算下一次等待时间。

每个线程还有一个时间管理对象 `TimeManager`，用来分别累计用户态和内核态运行时间。实际时间定时器 `ITIMER_REAL` 按经过时间发送 `SIGALRM`；用户时间定时器 `ITIMER_VIRTUAL` 只统计用户态时间；性能分析定时器 `ITIMER_PROF` 同时统计用户态与内核态时间。周期定时器到期后重新装载，一次性定时器到期后停止。

=== 子进程状态与等待条件

为了让父进程看到更多状态，`ProcessData` 中增加了停止和继续事件。父进程除了等待子进程退出，还能区分子进程被信号暂停和恢复运行。

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

退出状态被父进程读取后，子进程可以完成回收；停止和继续不会结束子进程，只消费本次状态记录。未被当前等待选项选中的状态会继续保留，供后续调用读取。

=== wait 系列接口与 pidfd

父进程可以等待一个指定子进程、某个进程组或任意直接子进程。moss 先把这三类目标转换成统一的内部表示（`Pid`、`Pgid` 和 `Any`），再从子进程集合中筛选。没有符合条件的直接子进程时返回 `ECHILD`。

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

传统的 `waitpid` 和 `wait4` 返回整数形式的状态，`waitid` 则通过 `siginfo_t` 说明子进程是正常退出、被信号终止、停止还是继续运行。moss 增加了 `waitid`，并支持按全部子进程、PID、进程组和 pidfd 选择目标。

pidfd 是指向进程对象的文件描述符。moss 将 pidfd 接入 `waitid`，父进程可以通过稳定的文件描述符等待目标，不会因为 PID 数值被复用而指向另一个进程。`WNOWAIT` 还允许调用者只读取状态，暂时不回收目标。

== Futex 同步机制

=== 基本思想与数据结构

Futex 把无竞争的同步留在用户态。线程先通过原子变量尝试获得锁，只有发现资源被占用时才进入内核等待。这样，大多数没有竞争的加锁和解锁不需要系统调用。

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

元组中的 `AtomicBool` 是 `notified`，表示是否已经收到通知；`u32` 保存 bitset；`PollSet` 保存任务的唤醒入口。三项分别回答“是否醒了、由哪类 wake 唤醒、怎样叫醒任务”。

=== WAIT 与 WAKE

等待者只有在用户地址仍等于期望值时才会睡眠。为此，moss 补严了 `FUTEX_WAIT` 和 `FUTEX_WAKE` 的检查与清理：系统调用入口先做一次快速检查，不相等就返回 `EAGAIN`；准备入队时，内核在持有等待队列锁的情况下再次读取用户值，如果条件已经改变，本次调用不再阻塞。

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

位掩码唤醒与重新排队也使用新增的 `notified` 状态和退出清理。`FUTEX_WAIT_BITSET` 为等待者附带位掩码，`FUTEX_WAKE_BITSET` 只唤醒 bitset 与目标 mask 有交集的任务，从而避免叫醒无关线程。

`FUTEX_REQUEUE` 先唤醒一部分任务，再把其余等待项移动到另一个 Futex 队列。`FUTEX_CMP_REQUEUE` 在此之前还会比较原地址的用户值，不匹配时返回 `EAGAIN`。移动过程中保留 `notified`、bitset 和 PollSet，不重新登记 Waker。

这类操作常用于 pthread 条件变量。广播时不必把所有线程同时叫醒争抢互斥量，可以先唤醒少量线程，再把其他等待者转移到互斥量对应的队列。

=== 超时、中断与等待修正

Futex 等待可能正常收到通知，也可能先超时或被信号打断。moss 用 `interruptible(timeout(...))` 组合这三条路径。超时返回 `ETIMEDOUT`，信号中断返回 `EINTR`；两种提前结束都会按照 `notified` 的对象地址精确删除当前等待项。

原有实现没有把 `FUTEX_WAIT` 和 `FUTEX_WAIT_BITSET` 的时间语义分开。moss 按 Linux 规则处理：前者接收相对时长，后者接收绝对截止时间，并根据 `FUTEX_CLOCK_REALTIME` 选择实时时钟或单调时钟。

独立通知状态、前后两次检查和退出时清理共同解决了几个问题：通知不会轻易落在登记空隙中，失效等待项不会消耗后续唤醒数量，正常唤醒和超时也不会重复处理同一个任务。这些修改同时修复了 LoongArch64 上 `futex_wait`、pthread cancellation 和部分休眠测试卡住的问题。

=== robust list 与线程退出清理

线程持有互斥量时如果直接退出，其他线程可能永远等不到解锁。针对这种情况，moss 完善了退出时的 Futex 清理。用户态线程库通过 `set_robust_list` 把 robust list 头指针交给内核，链表记录该线程可能持有的 Futex。

线程退出时，moss 按 `futex_offset` 找到对应 Futex，将 `owner_dead` 设为真，并唤醒一个等待者。被唤醒线程收到 `EOWNERDEAD` 后，由用户态线程库检查共享数据并恢复锁的一致性。链表遍历受 `ROBUST_LIST_LIMIT` 限制，损坏的用户链表不会让内核无限循环。

线程退出还会清零 `clear_child_tid`，并唤醒这个地址对应的 Futex。这条路径支持 `pthread_join` 等线程回收操作。

== 本章小结

StarryOS 已经具备 RR 调度、任务状态转换、上下文切换和基础事件等待。moss 沿用这套调度骨架，把工作重点放在阻塞后的处理：PollSet 缩短唤醒持锁路径，等待流程补上条件复查、信号中断和失效记录清理，定时器通过 `TimerKey` 过滤过期事件。

在这些通用机制之上，moss 修正了 Futex 的通知状态、队列清理、绝对超时和 robust list 退出处理，并扩展子进程停止、继续、`waitid` 与 pidfd 等待。各功能使用的是同一条主线：任务不满足条件时进入等待，事件或时间到达后被唤醒，再由调度器安排它继续执行。
