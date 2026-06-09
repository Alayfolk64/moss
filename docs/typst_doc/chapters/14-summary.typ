#import "../template.typ": evidence

= 总结与后续工作

== 当前设计特点

从当前源码看，StarryOS 的核心特点是：在 ArceOS/ax 组件上构建 Linux 用户态兼容内核，使用 Rust 模块化组织系统调用、用户地址空间、进程状态、文件描述符对象和伪文件系统。系统调用入口集中，内部对象尽量通过 `FileLike`、`AddrSpace`、`ProcessData` 等结构统一生命周期和访问路径。

内存管理已经具备 ELF 装载、用户栈/堆、mmap/brk、缺页填充、CoW、shared/file/linear 后端等基础。任务系统在 `axtask` 之上挂载 Linux 兼容状态。文件系统层既支持 axfs 普通文件，也支持 `/dev`、`/proc`、tmpfs、pipe、eventfd、timerfd、signalfd、socket、epoll 等常见 Linux fd 对象。

== 已确认边界

本文刻意不把测试适配脚本和伪信息文件解释为完整内核能力。特别是：

- `/proc/meminfo` 等 dummy 内容不代表真实内存统计。
- `/lib/modules/.../.config` 是测试环境文件，不代表所有配置项都有完整内核实现。
- framebuffer 和 input 支持不等于 GUI 桌面或 X11/Wayland 支持。
- BPF 支持是有限的 map/prog fd 与测试逻辑，不是完整 eBPF verifier/JIT/runtime。
- 网络 socket 支持依赖 `net` feature 和 `axnet`，关闭 feature 时显式返回 `ENOSYS`。

== 后续可补充内容

后续若要把本文档扩展成提交级设计报告，建议优先补充三类证据：

1. 架构/平台启动日志：记录构建命令、目标架构、rootfs、QEMU 或开发板参数。
2. 测试结果日志：按测试组归档原始输出和 summary，不直接手写成绩。
3. 模块图和调用链图：基于现有源码路径绘制，不引用其他 OS 的功能描述。

== 代码依据

#evidence((
  [`核心模块`], [`kernel/src/lib.rs`], [内核公开 entry，内部模块覆盖 config/file/mm/pseudofs/syscall/task/time。],
  [`兼容层入口`], [`kernel/src/syscall/mod.rs`], [`handle_syscall` 集中分发 Linux syscall。],
  [`启动与收尾`], [`kernel/src/entry.rs`], [init 进程退出后 unmount rootfs、flush 并 system_off。],
))
