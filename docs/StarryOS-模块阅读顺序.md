# StarryOS 模块阅读顺序（按 8 章）

本文档按你给出的 8 个章节组织，给出建议的源码阅读顺序（从上到下依次打开）。

## 第一章：应用程序与基本执行环境
1. `/Users/wuxun/os/StarryOS/src/main.rs`
2. `/Users/wuxun/os/StarryOS/kernel/src/lib.rs`
3. `/Users/wuxun/os/StarryOS/kernel/src/entry.rs`
4. `/Users/wuxun/os/StarryOS/kernel/src/task/user.rs`
5. `/Users/wuxun/os/StarryOS/vendor/axruntime/src/lib.rs`
6. `/Users/wuxun/os/StarryOS/vendor/axplat-riscv64-qemu-virt/src/boot.rs`
7. `/Users/wuxun/os/StarryOS/vendor/axhal/linker.lds.S`

## 第二章：批处理系统
1. `/Users/wuxun/os/StarryOS/src/init.sh`
2. `/Users/wuxun/os/StarryOS/src/ltp.txt`
3. `/Users/wuxun/os/StarryOS/kernel/src/entry.rs`
4. `/Users/wuxun/os/StarryOS/kernel/src/mm/loader.rs`
5. `/Users/wuxun/os/StarryOS/kernel/src/syscall/task/execve.rs`
6. `/Users/wuxun/os/StarryOS/kernel/src/syscall/task/wait.rs`

## 第三章：多道程序与分时多任务
1. `/Users/wuxun/os/StarryOS/vendor/axtask/src/lib.rs`
2. `/Users/wuxun/os/StarryOS/vendor/axtask/src/task.rs`
3. `/Users/wuxun/os/StarryOS/vendor/axtask/src/run_queue.rs`
4. `/Users/wuxun/os/StarryOS/vendor/axtask/src/timers.rs`
5. `/Users/wuxun/os/StarryOS/vendor/axtask/src/wait_queue.rs`
6. `/Users/wuxun/os/StarryOS/kernel/src/syscall/task/schedule.rs`
7. `/Users/wuxun/os/StarryOS/kernel/src/task/timer.rs`

## 第四章：地址空间
1. `/Users/wuxun/os/StarryOS/kernel/src/config/riscv64.rs`
2. `/Users/wuxun/os/StarryOS/kernel/src/mm/mod.rs`
3. `/Users/wuxun/os/StarryOS/kernel/src/mm/aspace/mod.rs`
4. `/Users/wuxun/os/StarryOS/kernel/src/mm/loader.rs`
5. `/Users/wuxun/os/StarryOS/kernel/src/mm/access.rs`
6. `/Users/wuxun/os/StarryOS/kernel/src/syscall/mm/mmap.rs`
7. `/Users/wuxun/os/StarryOS/kernel/src/syscall/mm/brk.rs`
8. `/Users/wuxun/os/StarryOS/vendor/axhal/src/paging.rs`
9. `/Users/wuxun/os/StarryOS/vendor/axcpu/src/riscv/uspace.rs`

## 第五章：进程及进程管理
1. `/Users/wuxun/os/StarryOS/vendor/starry-process/src/lib.rs`
2. `/Users/wuxun/os/StarryOS/vendor/starry-process/src/process.rs`
3. `/Users/wuxun/os/StarryOS/kernel/src/task/mod.rs`
4. `/Users/wuxun/os/StarryOS/kernel/src/task/ops.rs`
5. `/Users/wuxun/os/StarryOS/kernel/src/task/resources.rs`
6. `/Users/wuxun/os/StarryOS/kernel/src/syscall/task/mod.rs`
7. `/Users/wuxun/os/StarryOS/kernel/src/syscall/task/clone.rs`
8. `/Users/wuxun/os/StarryOS/kernel/src/syscall/task/execve.rs`
9. `/Users/wuxun/os/StarryOS/kernel/src/syscall/task/exit.rs`
10. `/Users/wuxun/os/StarryOS/kernel/src/syscall/task/wait.rs`

## 第六章：文件系统与 I/O 重定向
1. `/Users/wuxun/os/StarryOS/kernel/src/file/mod.rs`
2. `/Users/wuxun/os/StarryOS/kernel/src/file/fs.rs`
3. `/Users/wuxun/os/StarryOS/kernel/src/file/pipe.rs`
4. `/Users/wuxun/os/StarryOS/kernel/src/pseudofs/mod.rs`
5. `/Users/wuxun/os/StarryOS/kernel/src/pseudofs/fs.rs`
6. `/Users/wuxun/os/StarryOS/kernel/src/pseudofs/dev/tty/mod.rs`
7. `/Users/wuxun/os/StarryOS/kernel/src/syscall/fs/mod.rs`
8. `/Users/wuxun/os/StarryOS/kernel/src/syscall/fs/fd_ops.rs`
9. `/Users/wuxun/os/StarryOS/kernel/src/syscall/fs/io.rs`
10. `/Users/wuxun/os/StarryOS/kernel/src/syscall/fs/ctl.rs`
11. `/Users/wuxun/os/StarryOS/kernel/src/syscall/io_mpx/epoll.rs`
12. `/Users/wuxun/os/StarryOS/vendor/axfs/src/lib.rs`
13. `/Users/wuxun/os/StarryOS/vendor/axfs/src/highlevel/fs.rs`

## 第七章：进程间通信
1. `/Users/wuxun/os/StarryOS/kernel/src/syscall/ipc/mod.rs`
2. `/Users/wuxun/os/StarryOS/kernel/src/syscall/ipc/msg.rs`
3. `/Users/wuxun/os/StarryOS/kernel/src/syscall/ipc/shm.rs`
4. `/Users/wuxun/os/StarryOS/kernel/src/task/signal.rs`
5. `/Users/wuxun/os/StarryOS/kernel/src/syscall/signal.rs`
6. `/Users/wuxun/os/StarryOS/vendor/starry-signal/src/lib.rs`
7. `/Users/wuxun/os/StarryOS/vendor/starry-signal/src/api/process.rs`
8. `/Users/wuxun/os/StarryOS/vendor/starry-signal/src/api/thread.rs`
9. `/Users/wuxun/os/StarryOS/kernel/src/task/futex.rs`
10. `/Users/wuxun/os/StarryOS/kernel/src/syscall/sync/futex.rs`

## 第八章：并发
1. `/Users/wuxun/os/StarryOS/vendor/axsync/src/lib.rs`
2. `/Users/wuxun/os/StarryOS/vendor/axsync/src/mutex.rs`
3. `/Users/wuxun/os/StarryOS/vendor/axtask/src/run_queue.rs`
4. `/Users/wuxun/os/StarryOS/vendor/axtask/src/wait_queue.rs`
5. `/Users/wuxun/os/StarryOS/kernel/src/syscall/sync/mod.rs`
6. `/Users/wuxun/os/StarryOS/kernel/src/syscall/sync/futex.rs`
7. `/Users/wuxun/os/StarryOS/kernel/src/syscall/sync/membarrier.rs`
8. `/Users/wuxun/os/StarryOS/kernel/src/task/ops.rs`
9. `/Users/wuxun/os/StarryOS/vendor/axcpu/src/riscv/context.rs`

## 备注
- 先看 `kernel/src` 的语义实现，再下潜 `vendor/*` 的底层机制，理解会更顺。
- 当前顺序以 `riscv64-qemu-virt` 为主线；后续如果你切 LoongArch，可将 `riscv` 对应文件替换为 `loongarch64` 对应实现。
