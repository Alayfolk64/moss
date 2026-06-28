#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(1)

= 概述

== 项目简介

moss 是用 Rust 实现的宏内核，基于 ArceOS 的组件化框架，从 StarryOS 演化而来。ArceOS 把内核功能拆成独立的 crate：硬件抽象层、调度器、内存分配器、文件系统框架和网络栈各自独立，按需组合。moss 在这套底层之上实现进程管理、信号处理、文件描述符抽象和系统调用，构成完整的宏内核。

moss 支持 RISC-V 64 和 LoongArch64，两种架构共享同一套内核源码，硬件差异由 ArceOS 底层屏蔽。

== 系统架构

moss 的整体结构见 @fig:overview-arch，从上到下分为五层。

用户态程序通过系统服务接口层（syscall 入口）进入内核。内核主体按功能域分为八个子系统：任务与执行管理（Task、Process、Thread、Scheduler）覆盖进程和线程的完整生命周期；地址空间与内存管理（Virtual Memory、Page Fault、COW）负责用户地址空间；文件与存储管理（VFS、File System、FD Table、Mount）提供统一的文件接口；I/O 与事件通知（FD、AIO、Poll、Epoll）处理异步 I/O 和多路复用；IPC 与同步机制（Signal、Futex、Pipe、Wait Queue）处理进程间通信；网络通信子系统（Socket、TCP、UDP、Unix Socket、BPF）实现网络协议栈；系统资源管理（Permission、Limit、Credential、Namespace）管理进程凭据与资源约束；内核运行支撑（Timer、Trap、Config、Logging）提供基础运行时设施。

内核主体之下是 StarryOS/ArceOS 的组件化基础，提供 `axtask`、`axmm`、`axfs`、`axnet`、`axhal`、`axsync`、`axruntime` 等组件，moss 对其中部分做了定制，放在 `vendor/` 目录中。最底层是硬件，支持 RISC-V 64 和 LoongArch64 两种体系结构。

#figure(
  [
    #align(center)[
      #image("./pictrues/架构图浅色.svg", width: 65%)
    ]
  ],
  caption: [moss 系统整体架构],
) <fig:overview-arch>

== 初赛成果

截至 6 月 27 日 17:15 ，moss（队名"每天睡满八小时"）在初赛排行榜上位列第 4，综合得分 2601.18。进程管理、内存管理、文件系统、信号、网络和设备驱动几个主要模块均已完成，系统调用覆盖了 Linux ABI 的常用接口。在 QEMU 上可以运行 busybox shell、lua 和 libc 测试程序。

#figure(
  [
    #align(center)[
      #image("./pictrues/初赛排名1.png", width: 100%)
    ]
    #v(0.4em)
    #align(center)[
      #image("./pictrues/初赛排名2.png", width: 100%)
    ]
  ],
  caption: [初赛排行榜（截至 2026 年 6 月 27 日）],
) <fig:rank>

== 代码结构

```
moss/
├── src/
│   └── main.rs              # 内核入口，初始化运行时
├── kernel/
│   └── src/
│       ├── task/            # 进程与线程管理
│       ├── mm/              # 地址空间与 ELF 加载
│       ├── file/            # FileLike 抽象与文件描述符表
│       ├── pseudofs/        # procfs / sysfs 伪文件系统
│       ├── syscall/         # 系统调用分发与实现
│       └── config/          # 内核配置
├── vendor/                  # 定制版 ArceOS 组件
│   ├── axhal/               # 硬件抽象层
│   ├── axfs/ axfs-ng-vfs/   # 文件系统框架
│   ├── starry-process/      # 进程抽象
│   ├── starry-signal/       # 信号库
│   └── starry-smoltcp/      # 网络库
├── third_party/             # 第三方库（lwext4_rust 等）
├── docs/                    # 设计文档
└── Makefile                 # 构建入口
```

`vendor/` 里是 ArceOS 上游组件的定制版，接口与上游保持兼容，按宏内核需要做了扩展。各模块的设计在后续各章展开。
