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

这些修改共同作用于任务离开 CPU 和重新获得 CPU 的过程。其中最难处理的是通知与状态转换可能同时发生：任务检查完条件、尚未登记 Waker 时，事件可能已经到达；任务准备进入阻塞时，通知也可能先一步发生。下面的叙述会先简要说明调度基础，再重点讨论 moss 在阻塞唤醒机制上的增量改进。

== 任务调度机制

StarryOS 的任务调度围绕四种状态展开：`Running`（正在 CPU 上执行）、`Ready`（可运行，在队列中等待）、`Blocked`（等待事件）、`Exited`（已结束）。新任务以 `Ready` 进入就绪队列，被调度器选中后变为 `Running`。主动让出 CPU 的任务回到 `Ready`；等待资源的任务进入 `Blocked`，由 `unblock_task` 恢复为 `Ready`。任务执行结束后进入 `Exited`。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3.2.1.svg", width: 38%)
    ]
  ],
  caption: [TaskState 的状态转换关系],
) <fig:task-state>

任务按 RR 策略轮转使用 CPU，默认时间片为 5 个时钟滴答。时钟中断到来时，`scheduler_timer_tick` 更新当前任务时间片。计数归零时内核先设置待抢占标志，等代码离开禁止抢占的临界区后再执行重新调度。延迟抢占把上下文切换放到状态一致的位置，避免其他 CPU 看到中间状态。

每个任务保存一份 CPU 亲和性掩码 `cpumask`，限制它可以在哪些 CPU 上运行。moss 修正了用户位图解析过程：按实际配置的 CPU 数量读取内容，把置为 1 的位写入任务掩码。新掩码保存后还要检查任务当前所在 CPU，如果已被排除，moss 会将任务迁移到另一个允许的运行队列。

== 阻塞与唤醒机制的增量

=== PollSet 的优化

StarryOS 的 `PollSet` 管理一组 Waker，但唤醒时可能在持锁情况下调用 `wake()`，导致调度路径被锁保护。moss 增加 `has_waiters` 原子标志，没有等待者时直接返回；真正唤醒时，先在锁内取出并清空本轮 Waker，再到锁外逐个调用。

#figure(
  [
    ```rust
    // vendor/axpoll/src/lib.rs

    pub struct PollSet {
        inner: Lazy<Mutex<Inner>>,
        has_waiters: AtomicBool,  // moss 新增：快速路径优化
    }

    pub fn wake(&self) -> usize {
        if !self.has_waiters.load(Ordering::Acquire) {
            return 0;  // 快速返回
        }
        let mut guard = self.inner.lock();
        let (wakers, len) = guard.take_wakers();  // 取出并清空
        self.has_waiters.store(false, Ordering::Release);
        drop(guard);  // 锁外唤醒
        for waker in wakers.iter().take(len) {
            unsafe { waker.assume_init_read() }.wake();
        }
        len
    }
    ```
  ],
  caption: [PollSet 的锁外唤醒优化],
) <lst:pollset-wake>

=== block_on 的竞争窗口处理

Future 返回 `Pending` 与通知到达可能发生在相邻时刻。moss 用 `woke` 标记协调这两条路径：Waker 先把 `woke` 设为真再尝试唤醒任务；阻塞路径在改变任务状态前检查该标记。通知先到时，任务不再进入 `Blocked`，只让出一次 CPU 后重新轮询。

#figure(
  [
    ```rust
    // vendor/axtask/src/future/mod.rs

    struct AxWaker {
        task: WeakAxTaskRef,
        woke: SpinNoIrq<bool>,  // moss 关键：记录通知是否已到达
    }

    impl Wake for AxWaker {
        fn wake_by_ref(self: &Arc<Self>) {
            if let Some(task) = self.task.upgrade() {
                *self.woke.lock() = true;  // 先标记
                select_run_queue(&task).unblock_task(task, false);  // 再唤醒
            }
        }
    }

    // block_on 中检查 woke 标记
    match fut.as_mut().poll(&mut cx) {
        Poll::Pending => {
            let woke = axwaker.woke.lock();
            if !*woke {
                rq.blocked_resched(woke);  // 安全阻塞
            } else {
                drop(woke);
                crate::yield_now();  // 通知先到，只让出 CPU
            }
        }
        Poll::Ready(output) => break output,
    }
    ```
  ],
  caption: [woke 标记补上 Future 状态与任务状态之间的空隙],
) <lst:block-on-waker>

@fig:block-on-race 对比了正常阻塞路径与竞争路径的时序差异。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-block-on-race.svg", width: 90%)
    ]
  ],
  caption: [block_on 的两条路径时序对比],
) <fig:block-on-race>

=== 定时器队列的去重

StarryOS 的定时事件可以按截止时间触发，但定时器重新设置后，旧事件可能仍留在队列中。moss 改用 `BTreeMap` 保存定时器，并用截止时间 `deadline` 和递增序号 `seq` 组成唯一的 `TimerKey`。

#figure(
  [
    ```rust
    struct TimerKey {
        deadline: Duration,  // 触发时刻
        seq: u64,           // 区分同一时刻的多个事件
    }

    struct AlarmRuntime {
        next_seq: u64,                      // 生成不重复序号
        timers: BTreeMap<TimerKey, Entry>,  // 按到期顺序保存
    }
    ```
  ],
  caption: [TimerKey 使每个定时事件都有独立键],
) <lst:timer-key>

重新设置或取消定时器时，内核删除旧键；后台任务取出事件后，还要确认它与定时器当前保存的键一致。只有匹配的事件才会触发信号，重新设置前留下的旧条目会被忽略。

=== 子进程等待的登记后复查

父进程调用 wait 时先筛选目标子进程，再检查是否已有可报告状态。如果暂时没有结果，它把 Waker 登记到 `child_exit_event` 并进入等待。moss 的关键修改是登记后复查：

#figure(
  [
    ```rust
    match check_children().transpose() {
        Some(result) => Poll::Ready(result),
        None => {
            child_exit_event.register(cx.waker());  // 登记
            match check_children().transpose() {     // 立即复查
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        }
    }
    ```
  ],
  caption: [登记后复查覆盖"首次检查结束、Waker 尚未登记"的窗口],
) <lst:child-wait-register>

这一模式用来覆盖"首次检查结束、Waker 尚未登记"期间发生的子进程状态变化。

== Futex 同步机制的增量

=== 独立通知状态

StarryOS 的 Futex 等待项只保存 Waker 和位掩码，无法分清正常唤醒、超时和信号中断。moss 将等待项组织为三元组：`(Arc<AtomicBool>, u32, Arc<PollSet>)`，其中 `AtomicBool` 记录该等待项是否已被选中唤醒。

#figure(
  [
    ```rust
    // kernel/src/task/futex.rs

    pub struct WaitQueue {
        // moss 关键改动：每个等待项包含独立的通知状态
        queue: SpinNoIrq<VecDeque<(Arc<AtomicBool>, u32, Arc<PollSet>)>>,
        //                         ^^^^^^^^^^^^^^^^  ^^  ^^^^^^^^^^^^
        //                         notified         bitset  唤醒入口
    }

    pub fn wake(&self, count: usize, mask: u32) -> usize {
        let mut woke = 0;
        self.queue.lock().retain(|(waiter, bitset, poll)| {
            if woke >= count || (bitset & mask) == 0 {
                true  // 保留
            } else {
                waiter.store(true, Ordering::Release);  // 标记已通知
                poll.wake();
                woke += 1;
                false  // 移除
            }
        });
        woke
    }
    ```
  ],
  caption: [Futex 等待项的独立通知状态],
) <lst:futex-wait-queue>

@fig:futex-queue 展示了 FutexKey 如何定位 WaitQueue，以及每个等待项三个字段的分工。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-futex-queue.svg", width: 92%)
    ]
  ],
  caption: [FutexKey、WaitQueue 与等待项三元组的结构关系],
) <fig:futex-queue>

=== 超时与中断的精确清理

moss 用 `interruptible(timeout(...))` 组合三条路径。超时或信号中断时，按照 `notified` 的对象地址精确删除当前等待项：

#figure(
  [
    ```rust
    let result = block_on(interruptible(timeout(
        timeout_dur,
        poll_fn(|cx| {
            if notified.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            poll.register(cx.waker());
            if notified.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }),
    )));

    match result {
        Ok(Ok(())) => Ok(true),  // 正常唤醒
        Ok(Err(_)) | Err(_) => {
            // 超时或信号中断：精确删除此等待项
            let mut queue = self.queue.lock();
            queue.retain(|(waiter, ..)| !Arc::ptr_eq(waiter, &notified));
            Err(if result.is_ok() { AxError::TimedOut } else { AxError::Interrupted })
        }
    }
    ```
  ],
  caption: [超时与中断后按对象地址精确删除等待项],
) <lst:futex-timeout-cleanup>

=== 重新排队时状态的保留

`FUTEX_REQUEUE` 先唤醒一部分任务，再把其余等待项移动到另一个 Futex 队列。moss 在移动过程中保留 `notified`、bitset 和 PollSet，不重新登记 Waker。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-3-futex-requeue.svg", width: 88%)
    ]
  ],
  caption: [FUTEX_REQUEUE 转移时保留等待项的所有状态],
) <fig:futex-requeue>

=== 修正的超时语义

原有实现没有把 `FUTEX_WAIT` 和 `FUTEX_WAIT_BITSET` 的时间语义分开。moss 按 Linux 规则处理：前者接收相对时长，后者接收绝对截止时间，并根据 `FUTEX_CLOCK_REALTIME` 选择实时时钟或单调时钟。

== 本章小结

StarryOS 已经具备 RR 调度、任务状态转换、上下文切换和基础事件等待，moss 没有另建一套调度器。本章的修改集中在调度器之外、重新进入调度器之前的那段路径：

- PollSet 缩短了持锁唤醒路径，`has_waiters` 提供快速返回
- `block_on` 用 `woke` 标记协调通知与阻塞的竞争窗口
- 定时器用 `TimerKey` 识别仍然有效的事件
- 子进程等待在登记 Waker 后复查状态
- Futex 通过独立 `notified` 状态、双重检查和精确删除处理竞争

这些改进修复了 LoongArch64 上 `futex_wait`、pthread cancellation 和部分休眠测试卡住的问题。它们最终回到同一条流程：条件不满足时阻塞，条件可能变化时唤醒，复查成功后转为就绪，再由调度器安排运行。
