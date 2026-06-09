#set text(
  font: ("Times New Roman", "SimSun"),
  size: 11pt,
  lang: "zh",
)
#set page(
  paper: "a4",
  margin: (x: 2.5cm, y: 2.2cm),
)
#set par(
  first-line-indent: (amount: 2em, all: true),
  justify: true,
  leading: 0.75em,
)
#show raw.where(block: true): it => {
  block(
    fill: rgb("#f6f6f6"),
    inset: 1em,
    radius: 6pt,
    width: 100%,
    text(
      font: ("Consolas", "KaiTi"),
      size: 10pt,
      it,
    ),
  )
}
#set heading(numbering: "1.1")

= 系统概述

== 系统简介

moss 是基于 StarryOS 继续开发的 Rust 操作系统内核。StarryOS 提供了组件化的内核基础，包括任务管理、地址空间管理、文件系统框架、系统调用分发、硬件抽象层和多架构平台支持。moss 延续这一设计思路，并在其基础上进一步完善内核功能。

moss 的主要目标是增强系统运行 Linux 用户态程序的能力。系统围绕用户态运行环境和常见系统调用语义，对进程与线程管理、调度与同步、内存管理、文件系统、I/O、信号处理和网络等模块进行了扩展，使用户程序能够获得更完整、稳定的运行环境。

在整体设计上，moss 保持 StarryOS 的模块化结构，通过补齐系统调用语义、资源管理、错误处理、阻塞唤醒和伪文件系统接口等能力，提升系统的实用性与兼容性。同时，moss 支持 RISC-V64 与 LoongArch64 双架构运行，为后续功能验证、应用适配和系统扩展提供了基础。

== 系统架构与模块划分

moss 采用分层与模块化结合的系统结构。整体上，系统可以划分为用户空间、系统服务接口层、内核核心功能层、组件化内核基础层以及平台适配层。用户程序通过系统调用、异常或中断等入口进入内核，内核根据请求类型调度相应的功能模块完成资源管理与系统服务，并最终通过硬件抽象层适配不同体系结构和运行平台。

#figure(
  image("pictrues/架构图.svg", width: 85%),
  caption: [moss 系统整体架构图],
) <fig:moss-arch>

如 @fig:moss-arch 所示，moss 的核心功能并不是由单一模块完成，而是由多个内核子系统协同构成。任务与执行管理负责进程、线程和调度相关逻辑；地址空间与内存管理负责虚拟内存、物理内存和缺页处理；文件与存储管理负责文件系统抽象、路径解析和文件对象管理；I/O 与事件通知负责文件描述符、异步 I/O、事件等待和多路复用；IPC 与同步机制负责信号、管道、futex 和等待队列等进程间协作能力。网络通信子系统、系统资源管理和内核运行支撑则共同补充完整的操作系统运行环境。

从代码组织上看，moss 主要目录结构如下所示。

#figure(
  [
    ```text
    moss
    ├── Cargo.toml                    # Rust workspace 配置
    ├── Makefile                      # 项目构建入口
    ├── src/                          # 用户态入口与初始化脚本
    │   ├── main.rs                   # 用户态启动入口配置
    │   ├── init.sh                   # 初始化脚本与运行环境准备
    │   ├── init_shell.sh             # 交互式 shell 初始化脚本
    │   └── ltp_testcases.sh          # 测试用例列表与运行配置
    ├── kernel/src/                   # moss 内核主体
    │   ├── config/                   # 内核配置与运行参数
    │   ├── task/                     # 任务、进程、线程、信号与 futex
    │   ├── mm/                       # 地址空间、ELF 加载、mmap 与缺页处理
    │   │   └── aspace/               # 用户地址空间与映射后端
    │   ├── file/                     # 文件对象、FD 表、pipe、eventfd 与 epoll
    │   ├── pseudofs/                 # procfs、sysfs、devfs 等伪文件系统
    │   │   └── dev/                  # 设备文件节点与 tty 支持
    │   └── syscall/                  # Linux 风格系统调用分发与实现
    │       ├── task/                 # 进程、线程、wait、exec、ptrace 等调用
    │       ├── mm/                   # brk、mmap、mremap、mlock 等调用
    │       ├── fs/                   # VFS、路径、mount、xattr 与文件 I/O 调用
    │       ├── io_mpx/               # select、poll、epoll 等 I/O 多路复用
    │       ├── ipc/                  # System V IPC 与共享内存相关调用
    │       ├── sync/                 # futex、membarrier 等同步相关调用
    │       └── net/                  # socket、网络选项与 BPF 相关调用
    ├── third_party/                  # 第三方组件与外部库适配
    │   ├── lwext4_rust/              # Ext4 支持库及 Rust 封装
    │   └── syscalls/                 # 多架构 syscall 编号与辅助表
    ├── vendor/                       # 固定版本的本地依赖与 ArceOS 组件
    ├── make/                         # 构建规则、平台配置与链接脚本
    ├── scripts/                      # 镜像准备、运行测试与辅助脚本
    └── docs/                         # 项目文档与报告材料
    ```
  ],
  caption: [moss 主要代码目录],
) <lst:moss-code-layout>

== 系统主要完成情况

当前 moss 已经能够完成 RISC-V64 和 LoongArch64 双架构构建，启动 busybox 用户态，准备 musl/glibc 运行环境，并运行 LTP 与比赛测试。系统主要完成情况如下。

#[
  #set text(size: 9pt)
  #table(
    columns: (1.1fr, 3.6fr),
    align: (left, left),
    inset: 5pt,
    table.header(
      [模块],
      [当前完成情况],
    ),
    [双架构运行],
    [支持 RISC-V64、LoongArch64，生成 `kernel-rv`、`kernel-la`，可通过 QEMU 启动运行。],
    [用户态环境],
    [以 busybox 为入口，准备 musl/glibc、动态链接器、基础目录和 LTP 运行环境。],
    [系统调用],
    [已覆盖进程、内存、文件、I/O、时间、信号、同步、网络、IPC 等主要 syscall 类别。],
    [进程管理],
    [支持 fork、clone、exec、wait、pidfd、ptrace、UID/GID、capability 和进程组等语义。],
    [内存管理],
    [支持 ELF 加载、COW、lazy allocation、mmap、mremap、mlock 和用户态缺页处理。],
    [文件系统],
    [支持 Ext4、tmpfs/ramfs、VFS 路径解析、FD 表、mount、xattr、stat/statx 和 loop device。],
    [伪文件系统],
    [补充 `/proc`、`/sys`、`/dev`，提供进程信息、挂载信息、设备节点和测试探测接口。],
    [I/O 与事件],
    [支持 read/write、pread/pwrite、sendfile、splice、AIO、pipe、fcntl、poll/epoll、inotify、fanotify。],
    [时间与同步],
    [支持 clock、gettimeofday、nanosleep、itimer、POSIX timer、timerfd、time namespace 和 futex。],
    [网络与 BPF],
    [支持本地回环 TCP/UDP、Unix socket、部分 socket option 和最小 BPF map/program 行为。],
    [工程化评测],
    [固定工具链和 vendor 依赖，提供镜像准备、运行库注入、自动运行和结果汇总脚本。],
  )
]

相对 StarryOS 基线，moss 的主要增量包括四点：一是 syscall 分发项由 211 扩展到 264，新增 54 项，新增 61 个 `sys_*` 实现并增强 82 个已有实现；二是补齐文件系统、I/O、进程、时间、信号和内存等模块的 Linux 兼容语义；三是扩展 `/proc`、`/sys`、`/dev` 和 init 脚本，使 busybox、glibc/musl 程序和 LTP 用例能够稳定运行；四是补齐 RISC-V64、LoongArch64 双架构构建运行与评测结果收集流程。

在后续章节中，本文将按照功能模块展开介绍 moss 的设计与实现。其中，第 2 章介绍进程与线程管理，第 3 章介绍调度与同步机制，第 4 章介绍内存管理，第 5 章介绍文件系统，第 6 章介绍 I/O 与事件通知，第 7 章介绍信号与异常处理，第 8 章介绍网络与 BPF 兼容，第 9 章介绍平台适配与工程化评测。
