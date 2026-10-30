#set document(title: "现场赛设计方案文档", author: "MossOS 团队")

// ============================================================
// 公共格式（参考 docs 下现有 Typst 报告）
// ============================================================
#let body-font = ("Times New Roman", "SimSun", "STSong")
#let heading-font = "Microsoft YaHei"
#let punctuation-font = ("SimSun", "STSong", "Noto Serif CJK SC")

#let hdr = context {
  align(center, text(font: body-font, size: 9pt, fill: rgb("#666666"))[
    MossOS 现场赛设计方案文档
  ])
  v(2pt)
  line(length: 100%, stroke: (thickness: 0.5pt, paint: rgb("#aaaaaa")))
}

#let ftr = context {
  align(center, text(
    font: body-font,
    size: 9pt,
    fill: rgb("#666666"),
    counter(page).display("1"),
  ))
}

#set text(font: body-font, size: 11pt, lang: "zh")
#set page(
  paper: "a4",
  margin: (top: 2.5cm, bottom: 2.2cm, x: 2.2cm),
  header: hdr,
  footer: ftr,
)
#set par(
  first-line-indent: (amount: 2em, all: true),
  justify: true,
  leading: 0.82em,
  spacing: 0.72em,
)
#set heading(numbering: "1.1")

// 保持正文和标题字体不变；句读在 SimSun 不可用时回退到横排字形。
#show regex("[，。；、]"): set text(font: punctuation-font)

#show list: it => block(inset: (left: 2em), it)

#show heading.where(level: 1): it => {
  set text(font: heading-font, size: 16pt, weight: "regular")
  block(above: 1.3em, below: 1em, sticky: true)[
    #counter(heading).display(it.numbering) #h(0.45em) #it.body
  ]
}

#show heading.where(level: 2): it => {
  block(above: 1em, below: 0.8em, sticky: true)[
    #set text(font: heading-font, size: 13.5pt, weight: "regular")
    #counter(heading).display(it.numbering) #h(0.45em) #it.body
  ]
}

#let tbl(head, rows, cols: none) = {
  let n = head.len()
  let colspec = if cols == none { (1fr,) * n } else { cols }
  let cell(v) = if type(v) == content { v } else { text(v) }
  table(
    columns: colspec,
    stroke: (thickness: 0.6pt, paint: rgb("#b9c4d4")),
    fill: (x, y) => if y == 0 {
      rgb("#e8edf5")
    } else if calc.even(y) {
      rgb("#f6f8fb")
    } else {
      white
    },
    inset: 0.5em,
    align: (x, y) => if y == 0 { center } else { left },
    table.header(..head.map(h => strong(cell(h)))),
    ..rows.map(r => r.map(cell)).flatten(),
  )
}

// ============================================================
// 封面
// ============================================================
#set page(header: none, footer: none)

#v(4.5cm)
#align(center)[
  #text(font: heading-font, size: 30pt, weight: "bold")[现场赛设计方案文档]
]
#v(0.35cm)
#align(center)[
  #text(font: heading-font, size: 17pt)[MossOS 现场决赛总结]
]

#v(9.5cm)
#align(center)[
  #block(width: 11cm)[
    #set text(size: 13pt)
    #grid(
      columns: (auto, auto),
      column-gutter: 0.8em,
      row-gutter: 1em,
      align: (right, left),
      [*参赛院校：*], [*重庆邮电大学*],
      [*参赛队名：*], [*每天睡满八小时*],
      [*队伍成员：*], [*吴勋、张可欣*],
      [*指导教师：*], [*马彬、钟福金*],
    )
  ]
]

#v(2.8cm)
#align(center)[
  #text(size: 13pt, weight: "bold")[2026 年 8 月]
]
#pagebreak()

#set page(header: hdr, footer: ftr)
#counter(page).update(1)

// ============================================================
// 正文
// ============================================================
= 概述

本次现场赛共四道题，分别考查 `seccomp`、KeyDB、`ftrace` 和 BuildStorm。现场时主要根据分值、现有系统能力及现场实现风险来进行测试实现的，主要精力集中在 KeyDB 和 BuildStorm 两项复杂应用测试上。

上午开赛后，考虑到第二题满分 100 分、分值占比较高，而第一题需要现场完成新的系统调用与进程状态逻辑，短时间内把握较小，因此优先选择第二题。下午先尝试第三题，发现其涉及虚拟文件、插桩记录和调用树生成，现场代码实现难度较大，尝试一段时间后无果，将剩余时间转向准备更充分的第四题。第一题和第三题未再展开完整测试。

现场赛题目完成情况如下：

#let ok = align(center, text(size: 15pt, weight: "bold", fill: rgb("#22863a"))[✓])
#let no = align(center, text(size: 15pt, weight: "bold", fill: rgb("#b31d28"))[✗])

#figure(
  tbl(
    ("题目", "QEMU RISC-V64", "QEMU LoongArch64", "VF2", "2K1000LA"),
    (
      ("题一：seccomp", no, no, no, no),
      ("题二：KeyDB", no, no, ok, ok),
      ("题三：ftrace", no, no, no, no),
      ("题四：BuildStorm", ok, ok, ok, ok),
    ),
    cols: (1.35fr, 1.25fr, 1.45fr, 0.8fr, 1.1fr),
  ),
  caption: [四道题在各平台的完成情况],
)

第二题在 VF2 和 2K1000LA 上均取得一定分数，但两个 QEMU 平台未能完成验证；第四题在四个平台全部跑通，其中两个 QEMU 平台在线上决赛提测时取得满分。

= 上午：KeyDB 部署与测试

第二题要求启动 KeyDB 服务端并完成 SET/GET、INCR、过期时间、列表和 PING 等测试，综合依赖网络、线程、文件 I/O、进程管理与时间系统调用。MossOS 已具备相应的应用兼容基础，因此现场工作的重点是准备运行环境并打通板端测试链路。

== MossOS 对 KeyDB 基本功能的支持

KeyDB 是基于 Redis 协议的多线程数据库，对内核的网络、进程线程、文件系统和时间接口均有较完整的要求。MossOS 已实现 TCP/IP 网络栈以及 `socket`、`bind`、`listen`、`accept`、`connect`、`send`、`recv` 等常用接口，能够支持 KeyDB 在 6379 端口监听并与测试客户端进行通信。

在线程与进程方面，MossOS 支持 `clone`、`fork`、`execve`、futex 和 pthread 所需的线程局部存储、同步与调度机制，可以满足 KeyDB 创建工作线程和并发处理客户端请求的需要。文件系统方面，内核能够挂载可读写 ext4 rootfs，并提供常用文件操作、目录访问、内存映射及动态链接支持。上述基础能力使现场工作的重点能够放在运行环境部署和板端配置上，而不需要临时补充 KeyDB 的核心内核依赖。

== 环境准备

现场环境准备方面主要遇到两方面问题，一方面是 rootfs 的部署问题。由于得到的是目录形式的分散文件，而赛前准备的板卡烧录流程均以完整文件系统镜像为输入，无法直接套用，因此两块开发板根据已有环境采用了不同的处理方式。

VF2 板上的存储分区已有可用的系统镜像，因此现场采用挂载方式处理：在电脑端 mount 对应文件系统，将 KeyDB 服务端、测试客户端和测试脚本等文件写入该分区原有的可用镜像，再使用这一分区启动 MossOS 并进行测试。这种方式避免了重新制作和传输整套 rootfs，较快完成了 VF2 的测试环境准备。

2K1000LA 板上没有可直接复用的镜像，最开始尝试通过 Python 脚本向板端逐个传输测试文件，但传输完成后发现部分文件内容损坏，程序无法正常使用。为保证文件完整性，随后改为先在电脑端挂载一个可用镜像，将全部测试文件复制到镜像中并完成检查，再把整个镜像烧录到 2K1000LA 的磁盘分区。虽然整镜像传输和烧录耗时较长，但最终获得了稳定、可重复挂载的测试环境。

完成 rootfs 后，现场还需要配置板卡网络。比赛电脑同时连接两根网线：一根接入教室局域网，用于比赛环境和资料访问；另一根直连开发板，用于 TFTP 加载内核、文件传输和板端调试。由于对电脑双网口、接口名称、静态 IP、路由和板卡网段的配置不够熟悉，现场在识别两张网卡、避免网段冲突和确认链路连通性上花费了较长时间，也进一步压缩了后续 QEMU 调试时间。

== 测试结果

最终 KeyDB 在两块开发板上均成功运行并取得一定分数，说明网络栈和应用兼容能力能够支撑该负载。由于完成板端部署时上午已接近结束，未能继续定位 QEMU 环境问题，也没有足够时间返回第一题。该阶段的主要不足不是内核功能缺失，而是环境准备和多平台切换效率偏低。


= 下午：BuildStorm 多平台测试

BuildStorm 要求从源码完整编译大型 Rust 工程 `arceos-helloworld`，同时验证动态链接、ext4、进程线程、多核调度、内存映射及长时间重负载运行能力。下午在尝试第三题无果后，集中完成四种平台的 BuildStorm 验证。

== QEMU 平台
RISC-V64 与 LoongArch64 QEMU 均使用 VirtIO 块设备挂载 ext4 环境镜像。8月18日线上提测能够取得满分，主要依靠前期针对编译负载完成的内核优化：使用 per-CPU 物理页缓存降低分配器竞争，将缺页路径拆分为 prepare、execute、commit 三阶段以避免持锁等待 I/O；采用 folio 页缓存合并相邻页访问，只读文件页直接映射并在写入时 COW；同时将 ext4 数据 I/O 移出全局锁，并通过异步 VirtIO 请求提高多核并行度。上述设计使 rustc、cargo 与链接器能够稳定并行运行，并缩短完整构建耗时。

在系统调用层面，BuildStorm 会高频执行 `mmap`、`mprotect`、文件读写和进程线程操作。内核将 `mprotect` 的处理范围限制在相关 VMA，避免反复扫描全部映射；缺页慢路径中的文件读取在地址空间锁外执行，减少多个编译进程之间的串行等待。只读文件映射直接复用页缓存，只有发生写入时才进行私有复制，也减少了 rustc 读取大量依赖文件时的内存分配和复制开销。

在存储路径上，folio 页缓存能够一次处理连续文件页，ext4 数据 I/O 不再长时间占用全局锁，VirtIO 块设备则允许多个请求保持在途。这些优化共同提高了多核利用率，并保证长时间编译不会因锁竞争或 I/O 阻塞失去进展。

== VisionFive 2（VF2）

VF2 上的工具链检查、最小构建和正式 BuildStorm 均成功完成，完整编译耗时为 *692.98 秒*。除第 3.1 节所述的通用内核优化外，VF2 还针对板载 CPU 与 SD 卡存储路径采用了以下优化措施：

- *启用四核 SMP*：按照 VF2 的四核硬件配置启动全部处理器，使 cargo 能够调度多个 rustc 编译任务并行执行，充分利用板载计算资源；
- *启用 SD/MMC IDMAC*：SD 卡驱动不再依赖 CPU 逐块搬运数据，而是由控制器通过 IDMAC 完成连续多块 DMA 读写，降低大规模源码和构建产物读写时的 CPU 开销；
- *扩大单次 DMA 传输规模*：驱动配置 16 个 DMA 描述符和 64 KiB 缓冲区，每个描述符最多描述 4 KiB 数据，单次最多可传输 128 个 512 字节块，更适合 BuildStorm 连续读取源码、依赖与中间产物的访问模式；
- *采用中断和等待队列完成 I/O*：SD/MMC 命令提交后，当前任务进入等待队列，由完成中断负责唤醒。等待磁盘期间 CPU 可以调度其他编译任务，而不需要持续轮询控制器状态。

这些板级措施主要优化了 VF2 上的多核利用率与 SD 卡吞吐，减少了编译过程中 CPU 搬运数据和忙等 I/O 的时间，使通用的页缓存与文件系统优化能够在真实 SD 卡设备上发挥作用。

== 2K1000LA

2K1000LA 上同样完成了工具链检查、最小构建和正式 BuildStorm，完整编译耗时为 *5071.89 秒*。针对该平台处理器、SATA 存储和可用内存的特点，主要采用了以下板级优化措施：

- *将 Node PLL 设置为 900 MHz*：系统启动早期按照官方配置设置 Node PLL，并打开完整频率倍率，直接提升 CPU、缓存和片上互连的运行频率，为 rustc 编译与链接提供更高的计算性能；
- *启用双核 SMP*：启动 2K1000LA 的两个处理器核心，使 cargo 和 rustc 能够并行执行，在板载双核条件下尽可能提高编译吞吐；
- *使用 AHCI DMA 与中断等待*：SATA 数据通过 AHCI DMA 通路在磁盘和内存之间传输，命令完成后由硬件中断唤醒等待队列。编译任务等待文件 I/O 时能够让出 CPU，减少轮询产生的无效占用；
- *启用匿名页 swap*：启动前由 Linux 解析 `/swapfile-moss` 的物理 extent，将逻辑页、磁盘 LBA 和长度通过设备树传入 MossOS。内核据此直接进行页级 SATA I/O，在物理内存不足时换出匿名页，降低 rustc 和链接阶段因内存压力导致 OOM 的风险。

其中，提高 Node PLL 频率和启用双核 SMP 主要改善计算能力，AHCI DMA 与中断等待降低存储 I/O 的 CPU 开销，匿名页 swap 则为长时间、大内存占用的编译过程提供完成保障。

#figure(
  tbl(
    ("平台", "关键设备路径", "现场结果"),
    (
      ("QEMU RISC-V64", "VirtIO-blk + ext4", "线上提测满分"),
      ("QEMU LoongArch64", "VirtIO-blk + ext4", "线上提测满分"),
      ("VisionFive 2", "SD/MMC + IDMAC", "692.98 s"),
      ("2K1000LA", "AHCI/SATA + swap", "5071.89 s"),
    ),
    cols: (1.4fr, 1.8fr, 1.2fr),
  ),
  caption: [BuildStorm 四平台运行结果],
)

= 总结与反思

本次现场赛的主要成果是 KeyDB 和 BuildStorm 在两块开发板上取得有效结果，但是同时也暴露出我们自己对代码的熟悉度以及撰写代码的缺陷，在 AI 盛行的时代我们也需要掌握好代码基础，做到能够独立理解和修改系统调用、VFS 等关键模块。
