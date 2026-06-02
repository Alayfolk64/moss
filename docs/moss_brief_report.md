# MOSS 操作系统内核简要报告

## 第 1 章 摘要

### 1.1 MOSS 内核介绍

MOSS 是基于 `Starry-OS/StarryOS.git` 开发的 Rust 宏内核项目，底层继承 StarryOS/ArceOS 的组件化基础设施，并在此基础上面向 OS 比赛评测、Linux 用户态兼容和 LTP syscall 测试进行了系统性增强。MOSS 支持 RISC-V64 与 LoongArch64 两个主要比赛架构，能够构建 `kernel-rv` 与 `kernel-la` 双架构产物，并通过自动化脚本完成镜像准备、运行库注入、QEMU 启动、测试执行和结果汇总。

### 1.2 MOSS 整体设计目标

MOSS 的整体设计目标包括三点。第一，补齐 Linux/POSIX 风格系统调用语义，使 musl/glibc 用户态程序能够稳定运行。第二，围绕 LTP 与比赛测例补齐文件系统、进程、内存、时间、信号、I/O、网络等模块的边界行为。第三，通过固定工具链、vendor 依赖和双架构运行脚本提升评测环境下的可复现性。

### 1.3 基线与增量范围

| 项目 | 数量 |
|---|---:|
| 基线仓库 | `Starry-OS/StarryOS.git` |
| 基线提交 | `2e075acc` |
| MOSS 提交 | `6d54abf5` |
| 基线 syscall 分发项 | 211 |
| MOSS syscall 分发项 | 264 |
| 新增 syscall 分发项 | 54 |
| 新增 `sys_*` 实现函数 | 61 |
| 增强已有 `sys_*` 实现函数 | 82 |

上述统计采用两个口径：分发项口径统计 `kernel/src/syscall/mod.rs` 中的 `Sysno::...`；实现函数口径统计 `kernel/src/syscall/` 下的 `pub fn sys_*`。两者数量不同，是因为部分 syscall 在 StarryOS 基线中已有分发项但仅为 dummy 或最小 stub，MOSS 将其替换为真实实现。

### 1.4 MOSS 模块完成情况

| 模块 | MOSS 实现功能概述 |
|---|---|
| 进程管理 | 支持进程/线程创建、退出与资源回收，完善 `fork`、`clone`、`execve`、`wait`、`waitid`、`pidfd`、`ptrace` 等 Linux 进程语义，支持 UID/GID、capability、umask、进程组及部分 namespace 行为 |
| 内存管理 | 支持用户地址空间管理、ELF 加载、用户栈/堆映射、Copy-on-Write、lazy allocation、file-backed mmap、`mremap`、`mlock`、`MAP_GROWSDOWN` 等机制，补齐 page fault、SIGBUS/SIGSEGV 等异常处理语义 |
| 硬件抽象层 | 基于 StarryOS/ArceOS 的 HAL 体系，重点适配 RISC-V64 与 LoongArch64，支持 QEMU virt 平台启动、页表管理、用户态上下文切换、trap/exception 处理，并对 LoongArch64 用户态异常和非对齐访问做了适配 |
| 任务调度 | 基于底层任务运行时支持多任务运行、阻塞/唤醒、等待队列、定时器唤醒和线程退出通知，配合 `sleep`、`poll`、`epoll`、`futex`、timerfd 等 syscall 实现可验证的任务调度行为 |
| 文件系统 | 支持 Ext4、tmpfs/ramfs、部分兼容型文件系统后端，完善 VFS、路径解析、FD 表、权限检查、`stat/statx`、xattr、mount/umount、loop device、新 mount API；同时扩展 `/proc`、`/sys`、`/dev` 等 Linux 用户态探测接口 |
| 信号机制 | 支持进程/线程间信号传递、信号屏蔽、`sigreturn`、`sigaltstack`、`sigsuspend`、`sigtimedwait` 等行为，能够将用户态异常转换为 `SIGSEGV`、`SIGBUS`、`SIGILL`、`SIGTRAP` |
| I/O 与事件通知 | 完善 `read/write`、`readv/writev`、`pread/pwrite`、`sendfile`、`splice`、`copy_file_range`、AIO 等文件 I/O 语义，支持 pipe、fcntl、record lock、flock、poll/epoll，并实现 inotify 与 fanotify |
| 时间与定时器 | 支持 `clock_gettime`、`clock_settime`、`gettimeofday`、`times`、`nanosleep`、`setitimer/getitimer`、POSIX timer、timerfd、time namespace 等功能 |
| 网络支持 | 支持本地回环网络、TCP/UDP、Unix socket、部分 socket option、netperf 相关场景，并提供最小 BPF map/program 兼容实现 |
| 系统调用 | 面向 Linux/POSIX 兼容补齐大量 syscall，覆盖进程、内存、文件、时间、信号、同步、网络、IPC、mount、xattr、fanotify/inotify 等类别 |
| 工程化与评测框架 | 固定 Rust nightly 和依赖版本，支持离线 vendor 构建，生成双架构产物，提供镜像准备、运行库注入、QEMU 启动、LTP 执行和结果汇总脚本 |

## 第 2 章 系统概述

### 2.1 整体架构

MOSS 的整体运行链路可以概括为：用户态 busybox、musl/glibc 程序和 LTP 测例通过系统调用进入内核 syscall 分发层，再根据 syscall 类别进入 task、mm、fs、io、time、net、signal、sync、ipc 等模块；这些模块向下复用 StarryOS/ArceOS 的任务运行时、文件系统、硬件抽象层和平台组件，最终运行于 RISC-V64 或 LoongArch64 的 QEMU virt 平台。

### 2.2 代码结构

MOSS 的主要代码结构如下。`src/` 负责用户态入口、init 脚本和 LTP case 注入；`kernel/src/` 是核心内核逻辑；`kernel/src/syscall/` 负责系统调用分发与实现；`kernel/src/task/` 负责进程、线程、信号、定时器和 futex；`kernel/src/mm/` 负责地址空间、ELF loader、mmap 和用户内存访问；`kernel/src/file/` 负责文件对象、FD、pipe、eventfd、signalfd、timerfd、epoll 等抽象；`kernel/src/pseudofs/` 负责 procfs、sysfs、devfs 等伪文件系统；`vendor/` 和 `third_party/` 用于固定 ArceOS 组件、本地依赖、syscall number 表和 ext4 支撑库。

### 2.3 设计理念

MOSS 的设计理念是以 StarryOS 为基础，围绕比赛评测补齐 Linux 兼容语义。项目并不追求一次性实现完整 Linux 内核，而是优先打通真实用户态程序和 LTP 测例所依赖的关键路径。Rust 的类型系统和所有权机制用于降低内核模块实现中的资源管理风险，vendor、本地工具链和自动化脚本则用于保证隔离评测环境中的可复现运行。

### 2.4 增量统计口径

报告中的 syscall 数量采用两个统计口径。第一个是 syscall 分发表口径，统计 `kernel/src/syscall/mod.rs` 中能够进入 match 分发的 `Sysno::...` 项。第二个是 syscall 实现函数口径，统计 `kernel/src/syscall/` 下的 `pub fn sys_*`。由于一个实现函数可能服务多个 syscall，且部分基线 syscall 原本只接入 dummy fd 或简单 stub，所以两个口径的数量不完全一致。

### 2.5 系统运行流程

MOSS 的运行流程从 `make all` 开始，构建 RISC-V64 与 LoongArch64 两套内核并生成 `kernel-rv`、`kernel-la`。运行脚本准备 sdcard 镜像，注入运行库，启动 QEMU。内核启动后挂载 `/proc`、`/sys`、`/dev`、`/tmp` 等伪文件系统，加载 busybox，执行内嵌 init 脚本，准备 musl/glibc 环境，随后运行比赛测试和 LTP 测例。测试结束后，init 退出，内核统一卸载并 flush 文件系统，最后调用关机流程，使 QEMU 可以被脚本稳定收集结果。

## 第 3 章 系统调用与 Linux 兼容接口设计

### 3.1 系统调用分发结构

MOSS 的 syscall 层按功能模块组织，主要包括文件系统、I/O 多路复用、IPC、内存、网络、资源管理、信号、同步、系统信息、任务管理和时间等部分。用户态通过架构相关 syscall 入口进入内核后，`handle_syscall` 根据 `Sysno` 将请求分发给对应模块。

### 3.2 StarryOS 基线 syscall 支持情况

基线 StarryOS 在 `2e075acc` 提交下共有 211 个 syscall 分发项。基线已经具备基本 Linux 用户态运行能力，但不少 syscall 仍停留在最小实现、dummy fd 或弱语义阶段，面对 LTP 中的边界检查、错误码校验、阻塞唤醒、权限判断和路径解析时仍然不足。

### 3.3 MOSS syscall 增量统计

MOSS 在 `6d54abf5` 提交下共有 264 个 syscall 分发项，相比基线新增 54 个分发项。从实现函数看，MOSS 新增 61 个 `sys_*` 函数，增强 82 个已有 `sys_*` 函数。新增部分主要集中在 xattr、fanotify、inotify、AIO、timerfd、POSIX timer、新 mount API、ptrace、execveat、waitid、BPF 等方向；增强部分主要集中在文件系统、I/O、进程、时间、网络和内存语义。

### 3.4 按模块划分的 syscall 增量表

| 模块 | 新增 `sys_*` 实现函数 | 增强已有 `sys_*` 实现函数 |
|---|---:|---:|
| 进程管理 | 7 | 17 |
| 任务调度 | 2 | 4 |
| 内存管理 | 3 | 4 |
| 文件系统 | 20 | 21 |
| I/O 与事件通知 | 12 | 23 |
| 信号机制 | 1 | 1 |
| 时间与定时器 | 11 | 4 |
| 网络与 BPF | 1 | 7 |
| 同步机制 | 0 | 1 |
| 系统管理类 | 4 | 0 |
| 合计 | 61 | 82 |

### 3.5 面向 LTP 的验证方式

MOSS 的 syscall 增量并不是简单增加分发项，而是围绕 LTP 可验证语义补齐行为。例如文件系统类 syscall 需要满足路径解析、权限检查和错误码约定；进程类 syscall 需要满足等待、退出、ptrace 和信号交互；时间类 syscall 需要满足阻塞、唤醒和超时行为；I/O 类 syscall 需要满足 nonblock、poll/epoll、offset 和 partial transfer 语义。

## 第 4 章 进程管理模块

### 4.1 核心数据结构

MOSS 的进程管理围绕进程、线程、`ProcessData`、FD 表、UID/GID、进程组、会话、退出状态和等待事件展开。相较基线，MOSS 更强调 Linux 进程语义中的资源状态、身份属性和等待关系。

### 4.2 进程创建与执行

MOSS 支持 `fork`、`clone`、`clone3`、`execve`、`execveat` 等进程创建与执行接口。`execveat` 是新增 syscall，用于补齐 dirfd、`AT_EMPTY_PATH` 等路径相关执行语义。ELF loader 和脚本解释器 fallback 也服务于这一流程，使 busybox、shell 脚本和 LTP 二进制能够稳定启动。

### 4.3 进程等待与退出

MOSS 增强了 `wait4/waitpid`，并新增 `waitid`。等待语义支持更多 wait option、进程组过滤、ptrace wait target、子进程退出状态和 stop/continue 状态观察。进程退出后，内核会负责资源回收，并配合 init 退出后的统一卸载、flush 和关机流程完成评测闭环。

### 4.4 权限与进程属性

MOSS 补齐 real/effective/saved UID/GID 相关状态，新增 `getresuid`、`getresgid`、`setregid`，并增强 `setreuid`、`setresuid`、`setresgid`、`capget`、`capset`、`umask` 等接口。该部分主要服务权限类 LTP 测试和用户态工具对身份状态的探测。

### 4.5 ptrace 与特殊进程语义

MOSS 新增 `ptrace` 实现，支持 trace 状态、stop event、SIGTRAP 和 wait 交互。ptrace 与信号、wait、异常处理紧密相关，是调试类和进程控制类 LTP 用例能够正确计分的关键。

### 4.6 本模块 syscall 增量

本模块新增 7 个 syscall 实现：`getresuid`、`getresgid`、`setregid`、`execveat`、`ptrace`、`unshare`、`waitid`。本模块增强 17 个已有 syscall 实现，涉及 `clone3`、`execve`、`waitpid`、`setpgid`、`setsid`、UID/GID、capability、资源状态等。

## 第 5 章 内存管理模块

### 5.1 地址空间管理

MOSS 负责维护用户地址空间、内核地址空间、用户栈、堆、auxv 和 mmap 区间。地址空间管理不仅服务普通 ELF 程序加载，也服务 fork、exec、mmap、信号栈和 `/proc/[pid]/maps` 等用户态探测接口。

### 5.2 ELF 加载与动态链接

MOSS 的 ELF loader 支持动态链接器 fallback。当程序的 `PT_INTERP` 指向 `/lib/...` 而镜像中实际运行库位于 `/musl` 或 `/glibc` 时，内核会尝试兼容路径。MOSS 同时支持 shell 脚本和 shebang 解析，使 LTP 脚本、busybox 工具和动态链接用户程序更容易启动。

### 5.3 mmap 与文件映射

MOSS 增强了 `mmap`、`munmap`、`mremap`、`mlock2` 等接口，支持 file-backed mmap、`MAP_GROWSDOWN`、`MAP_FIXED_NOREPLACE`、shared/private 映射标记、文件尾 SIGBUS 和 mmap 区间记录。这些语义直接影响 mmap、fork、exec、proc maps 等测试。

### 5.4 COW 与懒分配

MOSS 对 Copy-on-Write 和 lazy allocation 进行了优化。COW 引用计数扩展避免 fork 风暴下溢出，未触碰的 lazy COW 区域在 fork 时可以减少不必要的 page table 复制，文件映射和 page cache 行为也围绕 LTP 场景进行了修复。

### 5.5 缺页异常与信号

MOSS 在 page fault 处理中区分普通缺页、growdown stack、文件尾访问和非法访问等场景，并将用户态异常转换为 `SIGSEGV`、`SIGBUS` 等信号。LoongArch64 平台还补充了用户态非对齐访问处理。

### 5.6 本模块 syscall 增量

本模块新增 3 个 syscall 实现：`mlockall`、`munlock`、`munlockall`。本模块增强 4 个已有 syscall 实现：`mmap`、`munmap`、`mremap`、`mlock2`。

## 第 6 章 文件系统模块

### 6.1 VFS 与文件对象抽象

MOSS 完善了 VFS、文件对象、FD 表和路径解析。文件系统模块统一处理 dirfd、`AT_EMPTY_PATH`、`AT_SYMLINK_NOFOLLOW`、路径权限、目录项遍历、文件描述符状态和 proc fdinfo 等行为。

### 6.2 文件系统与挂载

MOSS 支持 Ext4、tmpfs/ramfs 以及部分兼容型文件系统后端，增强 `mount`、`umount2`、bind mount、remount、只读挂载、loop device 和新 mount API。新增的新 mount API 包括 `fsopen`、`fsconfig`、`fsmount`、`move_mount`。

### 6.3 文件权限与元数据

MOSS 增强 `open/openat`、`stat/statx`、`chmod/chown`、`access/faccessat2`、`utimensat`、xattr、inode flags 等接口。xattr 新增 12 个 syscall 实现，覆盖路径、符号链接和 fd 三种访问形式。

### 6.4 procfs/sysfs/devfs

MOSS 将 `/proc`、`/sys`、`/dev` 等伪文件系统作为文件系统模块的重要组成部分。项目扩展了 `/proc/[pid]/maps`、`fdinfo`、`mounts`、`status`、`timens_offsets`，补充 `/proc/sys/fs`、`/proc/sys/kernel`、`/proc/sys/vm` 等探测接口，并提供 random、tty、loop、block device 等设备节点。

### 6.5 文件系统对评测的支撑

大量 glibc、busybox、mkfs/mount 工具和 LTP harness 会主动读取 `/proc`、`/sys`、`/dev` 或检查文件权限、挂载信息、xattr 和 inode metadata。MOSS 对这些接口的补齐，使测试用例不再因环境探测失败而提前退出。

### 6.6 本模块 syscall 增量

本模块新增 20 个 syscall 实现。其中 xattr 12 个：`setxattr`、`getxattr`、`listxattr`、`removexattr`、`lsetxattr`、`lgetxattr`、`llistxattr`、`lremovexattr`、`fsetxattr`、`fgetxattr`、`flistxattr`、`fremovexattr`；新 mount API 4 个：`fsopen`、`fsconfig`、`fsmount`、`move_mount`；节点与文件句柄相关 4 个：`mknod`、`mknodat`、`name_to_handle_at`、`open_by_handle_at`。本模块增强 21 个已有 syscall 实现，覆盖 `openat`、`mkdirat`、`linkat`、`unlinkat`、`renameat2`、`statx`、`mount`、`umount2` 等。

## 第 7 章 I/O 与事件通知模块

### 7.1 文件 I/O 语义

MOSS 对 `read/write`、`readv/writev`、`pread/pwrite`、`sendfile`、`splice`、`copy_file_range`、AIO 等 I/O syscall 进行了增强，补齐参数校验、错误码、offset、append、partial transfer、pipe/socket/file endpoint 等行为。

### 7.2 文件描述符控制

MOSS 增强 dup、close、close_range、fcntl、flock、record lock、pipe size 等文件描述符控制语义。FD 状态也与 `/proc/[pid]/fdinfo`、poll/epoll、nonblock 和 close-on-exec 行为联动。

### 7.3 poll/epoll

MOSS 的 poll/epoll 支持 regular file、pipe、socket、timerfd 等对象的就绪状态，补齐 ready queue、level/edge/oneshot、兴趣集修改、零超时和阻塞唤醒行为。epoll 与 timerfd、pipe、signal 等模块共同支撑 LTP 中的等待类测试。

### 7.4 inotify 与 fanotify

MOSS 新增 inotify 和 fanotify 子系统。inotify 支持 init、add watch、remove watch、事件队列、oneshot、overflow 等行为。fanotify 支持 init、mark、FID、permission event、mount/filesystem mark、ignore mask 和事件上报，是 MOSS 文件系统兼容性中的重点增量。

### 7.5 本模块 syscall 增量

本模块新增 12 个 syscall 实现：AIO 5 个 `io_setup`、`io_destroy`、`io_submit`、`io_cancel`、`io_getevents`；inotify 3 个 `inotify_init1`、`inotify_add_watch`、`inotify_rm_watch`；fanotify 2 个 `fanotify_init`、`fanotify_mark`；文件预读和同步 2 个 `readahead`、`sync_file_range`。本模块增强 23 个已有 syscall 实现，覆盖 `read/write/readv/writev`、`pread/pwrite`、`sendfile`、`splice`、`fcntl`、`close_range`、`epoll_ctl` 等。

## 第 8 章 信号机制模块

### 8.1 信号发送与屏蔽

MOSS 支持进程/线程信号发送、signal mask、pending signal 和实时信号相关行为。信号机制与进程等待、ptrace、异常处理、阻塞 syscall 和用户态返回路径紧密结合。

### 8.2 信号处理流程

MOSS 在进入用户态前检查待处理信号，必要时构造 signal frame 并切换到用户态处理函数。`rt_sigreturn` 用于从用户态信号处理函数恢复原上下文。

### 8.3 异常转信号

MOSS 能够将用户态异常转换为信号，包括 `SIGSEGV`、`SIGBUS`、`SIGILL`、`SIGTRAP`。其中 mmap 文件尾访问、非法地址、非法指令、ptrace trap 等场景会进入不同信号路径。

### 8.4 信号与 wait/ptrace 协作

信号模块与 wait/ptrace 共同处理 stop/continue、job control、ptrace stop 和 syscall restart。阻塞 wait 被信号打断时，MOSS 会尽量保持 Linux 风格的返回和重启语义。

### 8.5 本模块 syscall 增量

本模块新增 1 个 syscall 实现：`pause`。本模块增强 1 个已有 syscall 实现：`rt_sigtimedwait`。此外，异常转信号、ptrace stop、job control stop/continue 和 syscall restart 虽不都表现为新增 syscall，但属于信号模块的重要语义增量。

## 第 9 章 时间与定时器模块

### 9.1 时间系统调用

MOSS 的时间模块支持 `clock_gettime`、`clock_settime`、`gettimeofday`、`times`、`clock_getres`、`adjtimex`、`clock_adjtime` 等接口，覆盖 wall clock、monotonic clock、boottime、TAI 和 process/thread CPU time 等场景。

### 9.2 定时器机制

MOSS 支持 `nanosleep`、`clock_nanosleep`、`setitimer/getitimer`、POSIX timer 和 timerfd。全局 alarm task、timer worker 和 timerfd 文件对象共同负责定时器注册、取消、触发和唤醒。

### 9.3 time namespace

MOSS 支持 time namespace 的最小兼容语义，通过 `/proc/[pid]/timens_offsets` 暴露 child monotonic/boottime offset，使相关 LTP 探测能够进入预期路径。

### 9.4 与 poll/epoll 的协作

timerfd 与 poll/epoll 协作是时间模块的重要部分。MOSS 支持 nonblock、absolute timer、周期 timer 和过期次数读取，并修复 epoll timeout 与 timerfd 组合场景中的卡死或误判问题。

### 9.5 本模块 syscall 增量

本模块新增 11 个 syscall 实现：`clock_settime`、`adjtimex`、`clock_adjtime`、`timer_create`、`timer_delete`、`timer_getoverrun`、`timer_gettime`、`timer_settime`、`timerfd_create`、`timerfd_gettime`、`timerfd_settime`。本模块增强 4 个已有 syscall 实现：`clock_gettime`、`gettimeofday`、`times`、`setitimer`。

## 第 10 章 网络支持模块

### 10.1 网络模块概述

MOSS 支持本地回环 TCP/UDP、Unix socket 和基本 socket option，能够运行部分 netperf 场景。网络模块并不等价于完整 Linux 网络栈，而是围绕比赛和 LTP 网络类测例提供最小成功路径或正确错误路径。

### 10.2 socket 与 socket option

MOSS 增强了 `socket`、`bind`、`sendto`、`recvfrom`、`sendmsg`、`recvmsg`、`setsockopt` 等 syscall。实现中补齐了低端口权限、IPv6 domain 到 IPv4 后端兼容、Unix socket、部分 multicast 和 BPF 相关 socket option。

### 10.3 netperf 支持

MOSS 的 init 脚本和网络实现支持部分 netperf 测试组，覆盖 UDP/TCP stream、request/response 等本地回环场景。该部分主要用于证明网络路径可运行，并为比赛测试提供基础支撑。

### 10.4 BPF 最小兼容实现

MOSS 新增 `bpf` syscall，实现最小 BPF map/program 建模，支持 hash、array、ringbuf 等 map 类型的基本操作，并为 `SO_ATTACH_BPF` 等路径提供测试所需的最小行为。

### 10.5 本模块 syscall 增量

本模块新增 1 个 syscall 实现：`bpf`。本模块增强 7 个网络相关 syscall，包括 `socket`、`bind`、`sendto`、`recvfrom`、`sendmsg`、`recvmsg`、`setsockopt`。

## 第 11 章 任务调度与同步机制

### 11.1 多任务运行与阻塞唤醒

MOSS 复用底层任务运行时完成多任务运行、阻塞/唤醒、等待队列和线程退出通知。调度行为与时间、epoll、futex、信号等待等模块相互配合，是阻塞类测试能够稳定运行的基础。

### 11.2 调度相关 syscall

MOSS 支持 `sched_yield`、`sched_getaffinity`、`sched_setaffinity`、`sched_getscheduler`、`sched_setscheduler`、`sched_getparam`、`nanosleep`、`clock_nanosleep` 等调度相关接口。部分接口以最小兼容形式返回 Linux 风格结果。

### 11.3 futex 与同步

MOSS 增强 `futex`，补充 robust list、membarrier 等同步相关行为。futex 与线程阻塞、唤醒、超时、信号打断相关，是多线程用户态程序和 LTP 同步类用例的重要基础。

### 11.4 本模块 syscall 增量

任务调度新增 2 个 syscall 实现：`ioprio_get`、`ioprio_set`。任务调度增强 4 个已有 syscall 实现：`sched_getaffinity`、`sched_setaffinity`、`nanosleep`、`clock_nanosleep`。同步机制增强 1 个已有 syscall 实现：`futex`。

## 第 12 章 硬件抽象层与双架构适配

### 12.1 RISC-V64 与 LoongArch64 支持

MOSS 继承 StarryOS/ArceOS 的硬件抽象层，主要支持 RISC-V64 与 LoongArch64 两个架构。项目通过平台配置、构建目标、QEMU virt 启动和 wrapper ELF 产物生成，使同一套上层内核逻辑能够在双架构上运行。

### 12.2 trap 与上下文切换

MOSS 依赖 HAL 提供的 trap、exception 和用户态上下文切换能力实现 syscall 入口、异常处理和用户态返回。信号处理、ptrace、page fault 和调度切换都依赖这一层提供的上下文维护能力。

### 12.3 页表与地址转换

MOSS 在地址空间管理中使用底层页表抽象完成虚拟地址到物理地址的转换，支持用户空间访问检查、mmap 映射、COW、lazy allocation 和用户态数据拷贝。

### 12.4 LoongArch64 适配

LoongArch64 侧重点包括用户态异常处理、非对齐访问处理、构建产物生成和 QEMU 运行适配。MOSS 新增 `make/kernel-la-wrap.ld`，并通过运行脚本生成 `kernel-la` 比赛产物。

## 第 13 章 工程化与评测框架

### 13.1 离线可复现构建

MOSS 固定 Rust nightly、`Cargo.lock` 和 vendor 依赖，并通过本地 `.tools`、`vendor/axconfig-gen`、`vendor/cargo-binutils` 等减少联网依赖。构建系统默认使用离线依赖和固定版本，降低评测环境中的版本漂移风险。

### 13.2 双架构产物生成

MOSS 顶层 Makefile 支持生成 RISC-V64 与 LoongArch64 两套内核产物，并输出 `kernel-rv`、`kernel-la`。`make/kernel-rv-wrap.ld` 和 `make/kernel-la-wrap.ld` 将裸 kernel binary 包装成目标架构 ELF，满足比赛提交格式。

### 13.3 用户态运行环境准备

MOSS 的 init 脚本会准备 busybox、musl/glibc 动态链接器软链接、`/etc/passwd`、`/etc/group`、`/etc/resolv.conf`、`/proc/sys` 探测文件和必要用户态 shim。该部分保证 glibc、busybox、LTP、mkfs/mount 等工具可以在统一用户态环境中运行。

### 13.4 自动化评测脚本

MOSS 提供 `run_all.sh`、`run_rv.sh`、`run_la.sh`、`shell_rv.sh`、`shell_la.sh`、`summary.sh`、`total.sh` 和 `scripts/extract_ltp_summary.sh` 等脚本，用于构建、运行、交互调试、测试执行和结果汇总。

### 13.5 LTP 与比赛测试结果

MOSS 的评测框架支持 basic、busybox、cyclictest、iozone、iperf、libcbench、libctest、lmbench、lua、netperf、LTP 等测试组。LTP 用例列表可通过内核注入到 init 脚本，并由脚本逐条运行、打印 case 标记、补齐 Summary 和收集统计结果。

## 第 14 章 总结与展望

### 14.1 工作总结

MOSS 相对 StarryOS 基线新增 54 个 syscall 分发项，新增 61 个 `sys_*` 实现函数，增强 82 个已有 `sys_*` 实现函数。其贡献可以归纳为四类：第一，补齐 Linux 兼容 syscall 语义；第二，增强文件系统、I/O、时间、进程、内存和信号等核心模块；第三，支持 RISC-V64 与 LoongArch64 双架构产物；第四，构建自动化、可复现的比赛评测闭环。

### 14.2 不足与未来工作

当前 MOSS 的部分实现仍属于面向比赛和 LTP 的最小兼容实现，例如 BPF verifier、部分网络协议、部分文件系统兼容后端和若干系统管理 syscall。未来可以继续完善真实网络栈、更完整的 BPF 语义、更多文件系统持久化行为、SMP 支持、性能优化以及更多 LTP 用例覆盖。
