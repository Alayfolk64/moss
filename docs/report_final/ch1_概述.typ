#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(1)

= 概述

== 项目简介

MossOS 是用 Rust 语言实现的 Linux ABI 兼容宏内核，基于 ArceOS 组件化框架，从 StarryOS 演化而来。系统复用 ArceOS 的任务、内存、文件系统、网络和硬件抽象组件，并在其上实现进程线程、用户地址空间、信号、文件描述符和 Linux 系统调用语义。

MossOS 支持 RISC-V64 和 LoongArch64 两种指令集，内核主体在四个平台目标间共享：RISC-V QEMU virt、LoongArch QEMU virt、StarFive VisionFive 2 开发板 和 Loongson LS2K1000LA 开发板。平台差异由统一接口、定制平台 crate 和设备驱动共同承担。

== 系统架构

MossOS 的整体结构如 @fig:overview-arch 所示。用户程序通过 Linux ABI 系统调用进入内核；内核按功能划分为任务与执行、地址空间、文件存储、I/O 与事件、IPC 与同步、网络、资源管理和运行支撑等功能域；下层由 ArceOS 组件和具体平台实现提供调度、内存、驱动与硬件能力。图中各功能域表示系统的职责划分，不代表其已覆盖 Linux 的全部接口。

#figure(
  [
    #align(center)[
      #image("./pictrues/架构图浅色.svg", width: 75%)
    ]
  ],
  caption: [MossOS 系统整体架构],
) <fig:overview-arch>

`axcpu` 封装不同指令集的寄存器、陷阱和上下文切换操作；`axplat` 定义中断、时钟、串口、内存和启动等平台接口；四个平台 crate 分别实现这些接口。`axhal` 向内核提供统一的硬件访问入口，`axruntime` 负责主从核、内存、调度器、中断和驱动的初始化顺序。决赛期间进一步从 DTB 读取 CPU 与内存拓扑，使得同一套多核运行时能够适配 QEMU 和物理开发板。

== 支持平台

#figure(
  table(
    columns: (1.05fr, 0.8fr, 1.45fr, 2.1fr),
    inset: (x: 6pt, y: 6pt),
    stroke: 0.5pt + gray,
    table.header(
      align(center, [*平台*]),
      align(center, [*决赛验证配置*]),
      align(center, [*主要硬件路径*]),
      align(center, [*验证状态*]),
    ),
    [RISC-V QEMU virt], [8 核、8 GiB], [SBI、PLIC、VirtIO MMIO], [cagent 与 BuildStorm 完整运行],
    [LoongArch QEMU virt], [12 核、8 GiB], [EIOINTC、IOCSR IPI、VirtIO PCI], [cagent 与 BuildStorm 完整运行],
    [VisionFive 2], [4 核、4 GiB], [PLIC、UART、SD/MMC、GMAC1], [已完成实板启动、存储、网络与用户程序验证],
    [LS2K1000LA], [2 核目标], [LIOINTC、AHCI、GMAC、RTC], [已通过交叉构建和状态机测试，尚未实板验证],
  ),
  caption: [MossOS 决赛版平台支持与验证状态],
) <tab:platform-status>

== 从初赛到决赛

=== 初赛成绩

截至 2026 年 6 月 30 日 20:22，MossOS（队名“每天睡满八小时”）在初赛排行榜上位列第 3，综合得分 2601.18。当时进程、内存、文件系统、信号、网络和设备驱动已具备主干功能，并可在 QEMU 上运行 BusyBox shell、Lua 和 libc 测试程序。

#figure(
  [
    #align(center)[
      #image("./pictrues/初赛排名1.png", width: 100%)
    ]
  ],
  caption: [初赛排行榜（截至 2026 年 6 月 30 日）],
) <fig:preliminary-rank>

=== 决赛成绩

截至 2026 年 8 月 11 日，队伍在决赛线上排行榜中位列第 1，总分 758.2。RISC-V64 与 LoongArch64 的 BuildStorm 得分均为 180.0，cagent-glibc 得分均为 199.1。

#figure(
  [
    #align(center)[
      #image("./pictrues/决赛排名.png", width: 100%)
    ]
  ],
  caption: [决赛线上排行榜（截至 2026 年 8 月 11 日）],
) <fig:final-rank>

=== 决赛阶段主要进展

#figure(
  table(
    columns: (1fr, 3.8fr),
    inset: (x: 8pt, y: 6pt),
    stroke: 0.5pt + gray,
    table.header(align(center, [*方向*]), align(center, [*主要进展*])),
    [多核正确性], [从 DTB 获取 CPU 与内存拓扑，完善从核启动、远端唤醒、跨核 TLB 刷新和多核调度],
    [进程与同步], [完善 `vfork`、线程组 leader 发起的 `execve`、`rseq`、Futex、管道、信号与任务退出的并发语义],
    [物理平台], [完成 VisionFive 2 四核实板路径，并建立 LS2K1000LA 的 FDT 双核平台与基础外设驱动],
    [高并发构建], [完成 BuildStorm 专项性能优化，设计与评测数据见专项报告],
  ),
  caption: [MossOS 决赛阶段的主要进展],
) <tab:final-progress>

BuildStorm 的性能分析、优化设计和评测数据已在《MossOS 内核设计优化实现文档——BuildStorm》中独立说明，本报告不再重复展开。

== 代码结构

#block(breakable: false)[
```
MossOS/
├── src/                                  # 内核程序入口
├── kernel/src/
│   ├── task/                            # 进程、线程与信号
│   ├── mm/                              # 地址空间、缺页与 TLB
│   ├── file/                            # 文件抽象与描述符表
│   ├── pseudofs/                        # procfs、sysfs 与 devfs
│   ├── syscall/                         # Linux 系统调用
│   └── ipi.rs                           # 跨核回调
├── vendor/
│   ├── axcpu/ axhal/ axruntime/         # CPU、硬件抽象与运行时
│   ├── axtask/ axmm/ axfs/ axnet/       # 内核基础组件
│   ├── axdriver*/                       # 块设备与网络驱动
│   ├── axplat-riscv64-qemu-virt/        # RISC-V QEMU 平台
│   ├── axplat-loongarch64-qemu-virt/    # LoongArch QEMU 平台
│   ├── axplat-riscv64-visionfive2/      # VisionFive 2 平台
│   └── axplat-loongarch64-ls2k1000/     # LS2K1000LA 平台
├── third_party/                           # 第三方库
├── docs/                                  # 设计、评测与上板文档
└── Makefile                               # 统一构建入口
```
]

`vendor/` 保存了针对 MossOS 定制的 ArceOS 组件。本报告后续章节依次介绍进程线程、调度同步、内存、文件系统、信号、网络以及平台与设备驱动设计。
