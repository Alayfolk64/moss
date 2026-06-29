#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(9)

= 总结与展望

== 工作总结

Moss 目标是在 RISC-V 64 和 LoongArch64 上提供接近 Linux 的系统调用语义，使部分 Linux 应用程序能够不加修改地运行。

在进程与执行模型上，Moss 支持 fork/clone/exec/exit/wait 的主要路径，包括线程组、进程组和会话的三层组织，以及 UID/GID 凭据管理和 pidfd 接口。调度器在多核环境下运行，支持 CPU 亲和性和部分 rlimit 资源约束；futex 实现了 WAIT/WAKE、REQUEUE 和 robust list，能够支撑 glibc 的 pthread 库。

在内存管理上，Moss 完善了 mmap/munmap/mremap/mprotect 的主要路径，支持匿名映射、文件映射和写时复制，缺页处理覆盖了懒分配和 COW 触发两条路径。文件系统方面，Moss 通过统一的 VFS 层支持 ext4、FAT32、procfs 和 sysfs，并实现部分新式挂载 API、文件事件通知和 I/O 多路复用（epoll/poll/select）。信号机制覆盖标准信号与实时信号的主要语义，包括权限校验、停止/继续状态、嵌套信号帧和基础 signalfd。

网络方面，Moss 支持 TCP、UDP 和 Unix Domain Socket，实现了 socket、bind、connect、accept、send、recv 及相关选项接口。设备驱动通过三层平台抽象屏蔽架构差异，支持 virtio 块设备、网卡和串口，两种目标架构共享同一套内核源码。

== 未来工作

当前实现覆盖了宏内核的主干功能，但仍有若干方向值得在决赛阶段继续推进。

在功能扩展方面，io_uring 是 Linux 5.1 引入的高性能异步 I/O 接口，通过共享环形缓冲区避免系统调用开销，对 I/O 密集型负载有显著收益，希望以后能够参考 io_uring 实现一套异步 IO 机制。seccomp 系统调用过滤机制是容器安全的基础，也是运行沙箱类应用的前提条件。mount namespace 和 user namespace 目前只有基础实现，完整的容器隔离语义需要进一步完善。

在性能优化方面，当前调度器采用 RR 策略，对交互型和批处理型任务缺乏区分；引入多级反馈队列或 CFS 风格的调度器可以改善实际负载下的响应延迟。粗粒度锁在多核场景下是主要瓶颈，页缓存、文件描述符表和进程树的锁粒度均有细化空间。

// 以下为队伍自定义内容，请在此处补充决赛具体计划：
