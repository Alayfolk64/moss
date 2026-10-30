#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(8)

= 多架构硬件平台适配与设备驱动

== 平台适配概述

=== 基于 ArceOS 的硬件抽象体系

MossOS 基于 ArceOS 组件化框架并从 StarryOS 演进而来。系统复用了 ArceOS 的 `axcpu`、`axplat`、`axhal` 和 `axdriver` 等基础组件，并针对决赛平台补充了新的平台 crate 和原生设备驱动。

这套硬件抽象按职责分为三层。`axcpu` 处理指令集相关的 Trap、寄存器和上下文切换；`axplat` 定义启动、内存、中断、时钟、串口和电源等平台接口；`axhal` 将这些接口包装为内核其余模块可直接调用的统一入口。具体平台通过 `#[impl_plat_interface]` 在编译期完成静态绑定，不需要在运行时通过大量条件分支选择实现。

如 @fig:multi-platform-stack 所示，平台层和驱动层共同吸收机器差异，`axcpu` 则收敛指令集相关实现。

#figure(
  image("./pictrues/图-多架构硬件抽象.svg", width: 92%),
  caption: [MossOS 多架构硬件抽象结构],
) <fig:multi-platform-stack>

=== 从双架构 QEMU 到四平台支持

初赛阶段，MossOS 已能在 RISC-V64 QEMU virt 和 LoongArch QEMU virt 上运行，存储与网络主要依赖 QEMU 提供的 VirtIO 设备。决赛阶段又增加了 StarFive VisionFive 2 和 Loongson 2K1000LA 两个物理平台。真实硬件不再具有统一的 VirtIO 拓扑，内核需要接管固件传入的设备树，处理板级 CPU 编号、中断路由、存储控制器、网卡以及 DMA 一致性。

四个目标各由独立的平台配置和平台 crate 提供底层实现，上层调度、内存、文件系统和网络代码仍保持共享。

=== 四个平台的软硬件结构

#figure(
  table(
    columns: (1.25fr, 1.35fr, 1.45fr, 1.55fr),
    inset: (x: 5pt, y: 6pt),
    stroke: 0.5pt + gray,
    align: left,
    table.header([*平台*], [*启动与 CPU*], [*中断机制*], [*主要设备*]),
    [RISC-V QEMU virt], [OpenSBI，核数可配置], [PLIC、SBI timer/IPI], [VirtIO MMIO 块设备与网卡],
    [LoongArch QEMU virt], [QEMU 固件，核数可配置], [PCH-PIC + EIOINTC、IOCSR IPI], [VirtIO PCI 块设备与网卡],
    [VisionFive 2], [OpenSBI + U-Boot，4 个 U74], [PLIC、SBI timer/IPI], [UART、SD/MMC、GMAC1],
    [2K1000LA], [U-Boot，2 个 LA264], [LIOINTC、IOCSR IPI], [UART、RTC、AHCI、GMAC0],
  ),
  caption: [MossOS 四个目标平台的主要硬件路径],
) <tab:platform-overview>

== 设备树与硬件资源发现

=== 设备树的传递与解析

物理平台由 OpenSBI 或 U-Boot 将设备树二进制数据（DTB）传给内核。早期启动代码先保留 DTB 物理地址，建立足以访问内核和 DTB 的临时映射，再进入通用运行时。`axhal::dtb` 向平台层和驱动层提供统一解析入口，同时将 DTB 本身及固件保留区排除在页分配器之外。

QEMU 目标的机器结构稳定，主要资源可由平台配置给出；物理板则需要同时考虑固件实际交付的拓扑。因此 MossOS 使用“板级约束 + DTB 资源发现”的方式：稳定不变的地址窗口由平台描述，可能随固件和板卡配置变化的信息从 DTB 获取。

资源发现与初始化流程如 @fig:dtb-discovery 所示。

#figure(
  image("./pictrues/图-设备树资源发现.svg", width: 92%),
  caption: [设备树传递与硬件资源发现],
) <fig:dtb-discovery>

=== VisionFive 2 的硬件拓扑适配

VisionFive 2 平台描述给出 RAM、PLIC、UART、SD/MMC 和 GMAC1 的基本 MMIO 窗口，内核保留固件传入的 DTB，用于确认 CPU 与内存拓扑、固件保留区以及网卡 MAC 地址等信息。JH7110 的物理 hart 0 不提供普通 S-mode 运行环境，因此 MossOS 将逻辑 CPU 0--3 映射到物理 hart 1--4，并在中断和从核启动中始终使用这一映射。

=== 2K1000LA 的动态资源发现

2K1000LA 的板级实现更多地依赖固件 DTB。平台层在早期、尚未启用动态内存分配时解析 CPU 硬件 ID、UART、LIOINTC、RTC、watchdog 以及 reboot/poweroff syscon；驱动则继续从 DTB 获取 AHCI 和 GMAC 的 MMIO、IRQ、PHY 与 DMA 参数。这样既避免在通用代码中写死样例板地址，也能根据 `status`、alias 和 phandle 选择实际启用的设备。

=== 物理内存与 MMIO 布局

`MemIf` 统一描述可用 RAM、保留内存、MMIO 区间和物理/虚拟地址转换。VisionFive 2 的 4 GiB RAM 在早期由 Sv39 页表覆盖；2K1000LA 的 1 GiB 物理内存分布在低端和高端两个不连续区间，内核同时排除启动固件和 DTB 保留窗口。完成最终映射后，上层只通过 `axhal::mem` 访问普通内存和设备寄存器，不需要理解各板的地址窗口。

== 中断与多核事件机制

=== 统一中断抽象与分发流程

CPU 进入 Trap 后，`axcpu` 先区分同步异常和中断，再将中断原因交给 `axhal::irq`。具体平台通过 `IrqIf` 完成处理函数注册、中断源使能、真实 IRQ 号获取以及硬件确认。定时器中断驱动调度时钟，IPI 用于远端唤醒和跨核 TLB 刷新，外部中断则按 IRQ 号进入 UART、存储或网卡驱动。

统一分发路径如 @fig:unified-irq 所示，四个平台仅在中断控制器和硬件确认方式上保留差异。

#figure(
  image("./pictrues/图-统一中断分发.svg", width: 92%),
  caption: [四平台的统一中断分发路径],
) <fig:unified-irq>

=== RISC-V 平台的 PLIC 与 SBI

RISC-V 上层共享三类 S-mode 中断：STIP 用于定时器，SSIP 用于核间通知，SEIP 表示 PLIC 转发的外部设备中断。外部中断到达后，平台层先从 PLIC claim 寄存器获取真实 IRQ，调用处理函数后再 complete。定时器和 IPI 分别通过 SBI `set_timer` 与 `send_ipi` 提供。

RISC-V QEMU 上的 VirtIO MMIO 窗口对应 PLIC IRQ 1--8。VisionFive 2 也使用 PLIC，但需要将逻辑 CPU 转换为 JH7110 的物理 hart 和 S-mode PLIC context；其 UART、SD/MMC 和 GMAC1 都通过该路径产生设备中断。

=== LoongArch 平台的 EIOINTC、LIOINTC 与 IOCSR

LoongArch 的定时器由 CPU 稳频计数器和 `tcfg`/`ticlr` 寄存器控制，核间中断通过 IOCSR IPI 寄存器发送和确认。两个 LoongArch 平台的主要区别在外部中断路由：QEMU virt 使用 PCH-PIC 与 EIOINTC，2K1000LA 则使用由 DTB 描述的 LIOINTC。

2K1000LA 的 AHCI 和 GMAC 使用电平触发中断。LIOINTC 处理路径在调用设备处理函数前临时屏蔽当前源，待设备状态清除后再重新使能，避免未撤销的电平导致反复进入中断。

=== 四个平台的中断机制对比

#figure(
  table(
    columns: (1.25fr, 1.25fr, 1.25fr, 1.65fr),
    inset: (x: 5pt, y: 6pt),
    stroke: 0.5pt + gray,
    align: left,
    table.header([*平台*], [*定时器*], [*IPI*], [*外部中断*]),
    [RISC-V QEMU], [SBI timer / STIP], [SBI IPI / SSIP], [PLIC，VirtIO MMIO 和 UART],
    [LoongArch QEMU], [稳频计数器], [IOCSR IPI], [PCH-PIC + EIOINTC，VirtIO PCI 和 UART],
    [VisionFive 2], [SBI timer / STIP], [SBI IPI / SSIP], [PLIC，UART、SD/MMC 和 GMAC1],
    [2K1000LA], [稳频计数器], [IOCSR IPI], [LIOINTC，UART、AHCI 和 GMAC0],
  ),
  caption: [四个平台的中断路径对比],
) <tab:platform-irq>

== 设备驱动

=== 统一设备驱动框架

`axdriver` 通过 `DriverProbe` 统一设备探测，根据设备类型选择全局、MMIO 或 PCI 探测入口。探测成功的设备转换为 `AxDeviceEnum` 并注册到全局设备表，块设备向上实现 `BlockDriverOps`，网卡向上实现 `NetDriverOps`。因此 VFS 与 ext4 无需区分 VirtIO、SD/MMC 或 AHCI，网络栈也无需区分 VirtIO-net 和两种 GMAC。

=== QEMU 平台的 VirtIO 驱动

RISC-V QEMU 在固定 MMIO 窗口中枚举 VirtIO 设备，LoongArch QEMU 则通过 PCI ECAM 枚举 BDF，配置 BAR 后创建 VirtIO PCI 传输层。两条路径最终都进入同一 VirtIO 块设备或网卡实现。

VirtIO 通过 virtqueue 交换描述符链，`VirtIoHalImpl` 负责分配 DMA 页、转换物理地址和映射 MMIO。RISC-V VirtIO MMIO 可根据窗口地址得到 PLIC IRQ；LoongArch QEMU 中 VirtIO-blk 的 PCI legacy INTx 队列通知不够可靠，当前对块设备使用轮询完成，不影响平台定时器、IPI 和其他设备中断的正常工作。

=== VisionFive 2 的 SD/MMC 与 GMAC 驱动

VisionFive 2 的根文件系统位于 SD 卡。MossOS 在 `simple-sdmmc` 基础上将 JH7110 DWMMC 控制器接入块设备框架，补充了 PLIC 中断、等待队列、超时恢复、边界检查和 `flush()` 语义。当前决赛配置已启用控制器 IDMAC，使用对齐的描述符区和 bounce buffer 完成块数据传输。

网络方面，MossOS 为 JH7110 GMAC1 完成时钟与复位接管、RGMII 模式配置、MAC 地址获取、TX/RX 描述符环和 PLIC 中断接入。硬中断只记录设备事件并唤醒网络路径，报文和描述符回收在后续 poll 中完成，避免在中断上下文执行过多工作。UART 驱动也按 JH7110 的 4 字节寄存器步长完成适配，并使用接收中断和软件环形缓冲区支持交互式终端。

=== 2K1000LA 的 AHCI 与 GMAC 驱动

2K1000LA 使用板载 SATA 磁盘作为根存储。MossOS 在 `simple-ahci` 基础上完成 AHCI 控制器探测、端口识别、中断完成和可睡眠等待，并保留有界轮询作为中断遗失时的恢复路径。驱动能够解析 MBR/GPT，按 `/dev/sdaN`、`PARTUUID` 或 `PARTLABEL` 选择 ext4 根分区，再将该分区封装为上层可直接使用的块设备。

GMAC0 驱动从 DTB 获取控制器、PHY、RGMII、MAC 地址和 DMA burst 参数，通过增强型描述符环完成收发，并将 DMA 事件接入 LIOINTC。除 AHCI 和 GMAC 外，平台还实现了 DTB 驱动的 UART、RTC、watchdog 以及 syscon 复位和关机路径，使内核能够完整接管板级运行环境。

=== DMA 与缓存一致性

DMA 正确性是物理设备驱动与 QEMU 路径的主要差异之一。VisionFive 2 的 SD/MMC IDMAC 将静态描述符和 bounce buffer 放在 `.dma_uncached` 段；GMAC1 在 CPU 与 DMA 交换缓冲区所有权时，通过 JH7110 L2 cache 操作和 `fence iorw, iorw` 维护一致性。2K1000LA 的 AHCI 和 GMAC 校验 DTB 中的 DMA mask，使用非缓存 DMW 访问设备数据，并通过 `dbar 0` 保证寄存器与描述符的可见顺序。

这些处理被封装在各驱动的 HAL 实现内，块设备框架、网络栈和用户程序不需要感知底层缓存是否一致。

=== 驱动验证与能力边界

#figure(
  table(
    columns: (1.25fr, 1.55fr, 1.55fr, 2.1fr),
    inset: (x: 5pt, y: 6pt),
    stroke: 0.5pt + gray,
    align: left,
    table.header([*平台*], [*存储路径*], [*网络路径*], [*当前验证边界*]),
    [RISC-V QEMU], [VirtIO-blk MMIO], [VirtIO-net MMIO], [已完成决赛用户态负载验证],
    [LoongArch QEMU], [VirtIO-blk PCI], [VirtIO-net PCI], [已完成决赛用户态负载验证],
    [VisionFive 2], [SD/MMC], [JH7110 GMAC1], [已完成四核、存储、双向 TCP 和用户程序实板验证],
    [2K1000LA], [AHCI SATA], [DWMAC GMAC0], [已完成双核、AHCI、ext4 和用户态实板验证；GMAC 数据面仍需补充完整压力验证],
  ),
  caption: [四个平台的设备驱动与验证边界],
) <tab:driver-validation>

四个平台并不共享相同的启动流程、中断控制器和设备数据路径。MossOS 统一的是平台接口和上层设备语义：平台层将固件、内存和中断差异收敛到 `axplat`，驱动层将 VirtIO、SD/MMC、AHCI 和不同 GMAC 收敛为统一块设备与网络设备接口。因此，文件系统、网络栈、调度器和系统调用实现无需包含具体板级分支。
