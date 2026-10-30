#set text(
  font: ("Times New Roman", "Songti SC"),
  size: 11pt,
  lang: "zh",
)
#set page(
  paper: "a4",
  margin: (x: 2.5cm, y: 2.2cm),
)
//段落
#set par(
  first-line-indent: (amount: 2em, all: true),
  justify: true,
  leading: 0.85em,
  spacing: 0.85em,
)
//代码块
#show raw.where(block: true): it => {
  block(
    fill: rgb("#f6f6f6"),
    inset: 1em,
    radius: 6pt,
    width: 100%,
    breakable: true,
    above: 0.35em,
    below: 0.35em,
    align(
      left,
      text(
        font: ("Consolas", "KaiTi"),
        size: 10pt,
        it,
      ),
    ),
  )
}

//标题编号格式
#set heading(numbering: "1.1")

#set figure(numbering: (..nums) => {
  let chapter = counter(heading).get().first()
  numbering("1-1", chapter, ..nums)
})

// 一级标题
#show heading.where(level: 1): it => {
  pagebreak(weak: true)
  set align(center)
  set text(
    font: "PingFang SC",
    size: 16pt,
    weight: "regular",
  )
  v(0em)
  block[
    #if it.numbering != none [
      第#counter(heading).display()章 #it.body
    ] else [
      #it.body
    ]
  ]
  v(0.5em)
}

// 二级标题
#show heading.where(level: 2): it => {
  block(
    above: 1.1em,
    below: 1em,
    sticky: true,
    width: 100%,
  )[
    #set text(
      font: "PingFang SC",
      size: 14pt,
      weight: "regular",
    )
    #counter(heading).display(it.numbering) #h(0.45em) #it.body
  ]
}

// 三级标题
#show heading.where(level: 3): it => {
  block(
    above: 1em,
    below: 1em,
    sticky: true,
    width: 100%,
  )[
    #set text(
      font: "PingFang SC",
      size: 12pt,
      weight: "regular",
    )
    #counter(heading).display(it.numbering) #h(0.45em) #it.body
  ]
}

#counter(heading).update(1)

= 网络栈

网络栈负责组织网络数据的收发与传输，向内连接设备驱动和协议栈实现，向外提供 socket 接口、地址族管理和 I/O 多路复用接口。

== 概述

=== StarryOS 的已有基础

StarryOS 已经建立了可工作的网络栈框架。系统基于 smoltcp 协议栈，通过 axnet 模块封装 TCP/UDP socket，提供 `socket`、`bind`、`connect`、`send`、`recv` 等基础接口。在设备层，StarryOS 支持以太网设备和 loopback 设备，能够处理 IPv4 流量。

早期版本也建立了基本的收发路径。应用通过 socket 系统调用发送数据，数据经过协议栈封包后通过设备发送；接收路径通过轮询设备并推进协议栈状态。因此，网络栈面对的不是一片空白，而是一套主体结构已经成形，但地址族支持不完整、buffer 容量偏小、loopback 路径未优化的实现。

=== base os 实现的主要不足

当用户程序只进行简单的 TCP/UDP 通信时，StarryOS 的网络栈尚能工作。问题主要在负载变得复杂后暴露出来。

地址族支持方面，早期版本对 IPv4/IPv6 的区分不完整。`sys_socket()` 虽然接收 `AF_INET` 和 `AF_INET6`，但内部都按 IPv4 处理。IPv4-mapped IPv6 地址（如 `::ffff:127.0.0.1`）会导致路由查找失败并 panic。iperf 在 glibc/musl 下会混用 IPv4、IPv6 和 mapped 地址，基线无法正确处理这些组合。

Socket 语义方面，缺少 iperf/netperf 依赖的关键选项。`TCP_INFO` 返回固定假值，`SO_SNDBUF`/`SO_RCVBUF` 不按 Linux 语义翻倍，`IP_RECVERR`、`SO_DONTROUTE` 返回 `ENOPROTOOPT` 导致应用进入异常路径。netperf 的 TCP_STREAM 只能达到约 11 Mbits/sec，远低于预期。

Loopback 路径方面，本机 127.0.0.1 流量仍走完整的外部网络路径。数据经过 send buffer → 协议栈封包 → router queue → loopback device buffer → router RX queue → 协议栈解包，多次拷贝和排队。TCP buffer 仅 64KB，限制了吞吐上限。此外，每次 `send()`/`recv()` 都无条件推进网络栈，累积的轮询开销显著。

这些问题说明，基线已经能够完成基本网络操作，却还没有把地址族支持、socket 语义和 loopback 优化完整串联起来。

=== 优化的完善范围

优化保留 StarryOS 的主体结构，重点补齐地址族支持、socket 语义和 loopback 路径之间的联系。主要增量如表所示。

#figure(
  table(
    columns: (1.35fr, 2.3fr, 2.65fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([内容], [StarryOS 基线], [优化增量]),
    [地址族支持], [仅 IPv4], [区分 IPv4/IPv6，支持 IPv4-mapped IPv6 和 `IPV6_V6ONLY`],
    [Socket 语义], [缺少关键选项], [补齐 `TCP_INFO`、`SO_*BUF`、`TCP_QUICKACK`、`SO_LINGER` 等],
    [Buffer 容量], [TCP 64KB，UDP 64KB], [TCP 512KB，UDP 1MB，路由缓冲 256 项],
    [Loopback 路径], [走完整网络栈], [router 直投、大 MTU、TCP 绕过 cwnd、UDP 直达 socket],
    [轮询策略], [每次 I/O 都 poll], [批量 poll、只在 WouldBlock 时推进],
  ),
  caption: [网络栈模块的主要增量],
) <tab:net-overview>

== 核心设计思想

StarryOS 已经搭起了网络栈的基本框架，优化的工作是在这套框架中进一步理清地址族语义、loopback 路径和轮询策略：如何正确处理 IPv4/IPv6 混用；本机流量如何绕过外部网络的通用路径；何时推进网络栈才能减少开销。下面从这几个问题分别说明。

=== 地址族分层

IPv4 和 IPv6 并不是两套互不相干的协议。Linux 允许 IPv6 socket 接收 IPv4 连接（IPv4-mapped IPv6），也允许通过 `IPV6_V6ONLY` 禁用这一行为。优化延续 StarryOS 的做法，但明确区分地址族：socket 创建时保存 `IpFamily::Ipv4` 或 `IpFamily::Ipv6`，bind/connect/accept 按地址族判断地址格式，listen table 按 family 和 `ipv6_only` 匹配连接。

这种划分回答了一个最基本的问题：地址究竟属于 IPv4，还是属于 IPv6。`127.0.0.1` 是 IPv4 loopback，`::1` 是 IPv6 loopback，`::ffff:127.0.0.1` 是 IPv4-mapped IPv6。只要边界清楚，路由查找才不会因为地址格式不匹配而 panic，应用也不会因为 wildcard listener 错误接收不该接收的流量。

=== Loopback 与外部网络的分离

本机流量和外部流量的优化目标完全不同。外部网络需要考虑链路 MTU、拥塞窗口、丢包重传和校验和；本机 loopback 是内存内投递，不存在物理链路错误，也不应该被网络拥塞算法限制。

优化将 loopback 路径从通用网络路径中分离出来。Router 对目的地址是 loopback 的包直接投递到 RX buffer，不经过设备队列；loopback 使用独立的大 MTU（65535），不受外部网卡 1500 字节限制；TCP loopback tuple 绕过拥塞窗口，只受 buffer 容量限制；UDP loopback 直接写入目标 socket 的 RX queue，不经过协议栈封包和路由。

这样，本机流量只付出必要的 socket 层成本，而不是完整的网络栈成本。外部流量仍走原有路径，保证语义正确性。

=== 按需轮询与批量推进

Linux 网络栈不是每次系统调用都重新处理全部 socket，而是通过事件驱动和批量处理减少开销。最近访问的 socket 可能已经有数据可读/可写，无需立即推进协议栈；只有遇到 `WouldBlock` 时才需要等待新事件。

优化将轮询策略从"每次都 poll"改为"按需 poll"。`send()`/`recv()` 先尝试一次真实操作，只有遇到 `WouldBlock` 才推进网络栈并进入等待。`poll()`/`select()`/`epoll_wait()` 在检查 fd readiness 前统一推进一次，避免每个 socket 重复推进。TCP send 累积到一定字节数后才批量 poll，减少每次发送的固定成本。

这样，快路径少一次全局网络栈推进，慢路径仍能正确等待事件。对 loopback 已经有数据的场景，减少了不必要的轮询开销。

=== 性能与语义的平衡

网络栈优化不是若干独立改进的集合，而是一条连续的权衡链：

#align(center)[
  #box(
    inset: 8pt,
    stroke: 0.6pt + gray,
    radius: 3pt,
  )[
    地址族支持 → Socket 语义 → Buffer 扩展 → Loopback 优化 → 轮询策略
  ]
]

地址族支持解决功能正确性；Socket 语义让应用不进入异常路径；Buffer 扩展解除吞吐上限；Loopback 优化减少本机流量开销；轮询策略减少固定成本。优化的多项修改都围绕这条平衡链展开，尤其重视 loopback 路径和批量处理，而不仅是单个 socket option 的补齐。

== 关键数据结构

=== Socket 与地址族

Socket 和地址族是本模块最重要的两类数据。下面的代码省略了与本节无关的字段，只保留二者的关系。

#figure(
  [
    ```rust
    // Socket 保存地址族
    pub struct TcpSocket {
        family: IpFamily,
        handle: SocketHandle,
        local_addr: Option<SocketAddr>,
        peer_addr: Option<SocketAddr>,
    }

    pub struct UdpSocket {
        family: IpFamily,
        handle: SocketHandle,
        local_addr: Option<SocketAddr>,
    }

    // 地址族枚举
    pub enum IpFamily {
        Ipv4,
        Ipv6,
    }
    ```
  ],
  caption: [Socket 与地址族的数据结构],
) <lst:socket-family>

创建 socket 时根据 `AF_INET` 或 `AF_INET6` 保存 family，bind/connect 时按 family 判断地址格式是否匹配。Listen table 在接收连接时也检查 family 和 `ipv6_only` 标志，避免 IPv6 wildcard listener 错误接收 IPv4 连接。

=== Buffer 配置

StarryOS 原有 buffer 可以保存数据，却不适合高吞吐场景。优化增加 buffer 容量配置：

#figure(
  [
    ```rust
    // TCP buffer：从 64KB 提升到 512KB
    const DEFAULT_TCP_RX_BUF_LEN: usize = 512 * 1024;
    const DEFAULT_TCP_TX_BUF_LEN: usize = 512 * 1024;

    // UDP buffer：从 64KB 提升到 1MB
    const DEFAULT_UDP_RX_BUF_LEN: usize = 1024 * 1024;
    const DEFAULT_UDP_TX_BUF_LEN: usize = 1024 * 1024;

    // UDP packet metadata：提升到 1024 项
    const DEFAULT_UDP_METADATA_LEN: usize = 1024;

    // Router packet buffer：提升到 256 项
    const ROUTER_PACKET_BUFFER_LEN: usize = 256;
    ```
  ],
  caption: [Buffer 容量配置],
) <lst:buffer-config>

Buffer 扩展后，发送方/接收方不再因为 buffer 满而频繁阻塞。64KB buffer 很容易让 iperf 的 TCP 流量只能达到几百 Mbits/sec；512KB buffer 后可以支持 Gbits/sec 级别的吞吐。UDP 并发场景也不容易因为 metadata 或 payload ring 过小直接丢包。

=== Loopback 设备标识

内核需要快速判断目的地址是否是 loopback。优化使用统一的判断函数：

#figure(
  [
    ```rust
    impl IpAddress {
        pub fn is_loopback(&self) -> bool {
            match self {
                IpAddress::Ipv4(addr) => addr.is_loopback(),
                IpAddress::Ipv6(addr) => addr.is_loopback(),
            }
        }
    }

    // 设备标识
    pub trait Device {
        fn is_loopback(&self) -> bool;
        fn notify_rx_ready(&self);
    }
    ```
  ],
  caption: [Loopback 判断与设备标识],
) <lst:loopback-device>

Router 在发送时检查目的地址是否 loopback，若是则直接投递到 RX buffer，不经过设备队列。Device trait 增加 `is_loopback()` 和 `notify_rx_ready()`，允许 loopback 设备唤醒接收侧。

== 地址族支持与 Socket 语义

=== IPv4/IPv6 混用的问题

早期版本对地址族的处理过于简化。`sys_socket()` 虽然接收 `AF_INET` 和 `AF_INET6`，但创建的 socket 都按 IPv4 处理，`SocketAddrEx::family()` 固定返回 `AF_INET`。

问题不在地址本身，而在缺少区分。iperf 会根据目标地址自动选择协议，测试 `127.0.0.1` 时使用 IPv4，测试 `::1` 时使用 IPv6，测试 `::ffff:127.0.0.1` 时使用 IPv4-mapped IPv6。如果内核不区分这些地址，路由查找会失败。更重要的是，一个 IPv6 wildcard listener（`::`）可能错误接收 IPv4 连接，违反 `IPV6_V6ONLY` 语义。

=== 地址族区分与 IPV6_V6ONLY

优化为 socket 增加 family 字段。`sys_socket()` 根据 `AF_INET` 或 `AF_INET6` 保存对应的 `IpFamily`，`TcpSocket::new()` 和 `UdpSocket::new()` 在构造时保存该字段。

#figure(
  [
    ```rust
    // sys_socket 根据 domain 创建对应 family
    let family = match domain {
        AF_INET => IpFamily::Ipv4,
        AF_INET6 => IpFamily::Ipv6,
        _ => return Err(LinuxError::EAFNOSUPPORT.into()),
    };

    let socket = if ty == SOCK_STREAM {
        AxFileRef::new(TcpSocket::new(family))
    } else if ty == SOCK_DGRAM {
        AxFileRef::new(UdpSocket::new(family))
    } else {
        return Err(LinuxError::EINVAL.into());
    };
    ```
  ],
  caption: [Socket 创建时保存地址族],
) <lst:socket-family-create>

Listen table 在匹配连接时检查 family 和 `ipv6_only` 标志。IPv6 listener 如果未设置 `IPV6_V6ONLY`，可以接收 IPv4-mapped IPv6 连接；如果设置了该选项，则只接收真正的 IPv6 连接。这样，应用可以通过一个 IPv6 socket 同时服务 IPv4 和 IPv6，也可以强制只接收 IPv6。

修复后，早期日志里的 `no route to destination: ::ffff:127.0.0.1` 不再导致 panic，iperf 在 glibc/musl 下混用 IPv4/IPv6 时能够正常运行。

=== Socket Option 补齐

netperf 和 iperf 依赖多个 socket option 来配置行为和查询状态。早期版本缺少这些选项时，应用会进入异常路径或使用不合理的默认值。

优化补齐的主要选项包括：

#figure(
  table(
    columns: (1.8fr, 4.2fr),
    inset: 5pt,
    stroke: 0.5pt + gray,
    table.header([选项], [作用]),
    [`TCP_INFO`], [返回 TCP state、MSS、PMTU、cwnd 等字段，供应用诊断连接状态],
    [`SO_SNDBUF` / `SO_RCVBUF`], [按 Linux 语义翻倍，并在 socket 未 active 时重建 buffer],
    [`IP_RECVERR` / `IPV6_RECVERR`], [保存接收错误状态，避免返回 `ENOPROTOOPT`],
    [`SO_DONTROUTE`], [保存状态，当前作为 no-op 兼容项处理],
    [`TCP_QUICKACK`], [映射到 smoltcp delayed ACK 开关],
    [`SO_LINGER`], [`linger=0` 的 TCP 关闭走 `abort()`，更接近 Linux 的 RST 语义],
    [`TCP_MAXSEG`], [保存用户指定 MSS，影响 `getsockopt` 和 `TCP_INFO` 展示值],
  ),
  caption: [补齐的主要 Socket Option],
) <tab:socket-options>

`SO_SNDBUF`/`SO_RCVBUF` 按 Linux 语义做内部翻倍。用户设置 16KB 时，内核实际分配 32KB，`getsockopt` 返回 32000。这样，netperf 使用 `-s 16k -S 16k` 后看到的 buffer 大小更接近 Linux 行为。

当前实现的 `TCP_INFO` 返回确定的字段值，而非全部为零。应用可以通过该选项查询 MSS、PMTU 和 cwnd，不再误判连接状态。`TCP_QUICKACK` 映射到 delayed ACK 开关后，应用可以控制 ACK 延迟行为。

修复后，netperf 日志里的 `enable_enobufs`、`so_dontroute`、`errno 92` 等异常提示消失，TCP_STREAM 从约 11 Mbits/sec 提升到约 21-22 Mbits/sec。

== Buffer 扩展与容量优化

=== 容量不足的根源

早期版本的 buffer 容量严格限制了吞吐上限。TCP RX/TX buffer 仅 64KB，UDP RX/TX buffer 也是 64KB，UDP packet metadata 数量有限。

问题不在 buffer 管理算法，而在容量太小。iperf TCP 测试发送大量数据时，64KB buffer 很快填满，发送方必须等待接收方消费数据后才能继续发送。对于 loopback 这种低延迟路径，buffer 成为主要瓶颈。起点阶段 TCP 约 207-283 Mbits/sec，UDP 约 19-39 Mbits/sec，远低于 baseline。

=== Buffer 容量扩展

优化将 buffer 容量大幅提升，如代码 @lst:buffer-config 所示。TCP buffer 从 64KB 扩展到 512KB（8倍），UDP buffer 从 64KB 扩展到 1MB（16倍）。UDP packet metadata 数量调整到 1024 项，router packet buffer 数量调整到 256 项。

#figure(
  [
    ```rust
    // 扩展前后对比
    // TCP RX/TX: 64KB → 512KB
    const DEFAULT_TCP_RX_BUF_LEN: usize = 512 * 1024;

    // UDP RX/TX: 64KB → 1MB
    const DEFAULT_UDP_RX_BUF_LEN: usize = 1024 * 1024;

    // UDP metadata: 默认值 → 1024 项
    const DEFAULT_UDP_METADATA_LEN: usize = 1024;

    // Router buffer: 默认值 → 256 项
    const ROUTER_PACKET_BUFFER_LEN: usize = 256;

    // Socket buffer option 最大值限制为 8MB
    const MAX_SOCKET_BUF_LEN: usize = 8 * 1024 * 1024;
    ```
  ],
  caption: [Buffer 容量扩展前后对比],
) <lst:buffer-expansion>

扩展后，BASIC_TCP 先接近或超过 baseline。后续配合 loopback 优化，TCP 才能拉到 Gbits/sec 级别。UDP 并发场景也不容易因为 metadata 或 payload ring 过小直接丢包。

Socket option 对 buffer 的影响也需要正确处理。优化让 accepted TCP socket 继承 listen socket 的 buffer 配置，避免新连接重新使用默认小 buffer。`SO_SNDBUF`/`SO_RCVBUF` 按 Linux 语义翻倍后，用户设置的 buffer 大小能真正生效。

当前 buffer 容量是固定配置，不会根据系统内存动态调整。对于内存受限的嵌入式系统，固定的大 buffer 可能占用过多内存；对于服务器场景，8MB 的上限可能仍然偏小。这是容量策略的当前限制。

== Loopback 路径优化

=== 通用网络路径的开销

早期版本对本机流量和外部流量使用相同的处理路径。数据从应用发送到 127.0.0.1 时，经过完整的网络栈处理：send buffer → TCP/UDP 封包 → 计算 checksum → router queue → loopback device buffer → router RX queue → 解包 → 校验 checksum → recv buffer。

问题不在协议栈本身，而在 loopback 不需要这些开销。本机 127.0.0.1 或 ::1 流量是内存内投递，不存在物理链路错误，不需要 checksum 校验；不经过真实网卡，不应该受标准 MTU 1500 字节限制；不受网络拥塞影响，不应该被 TCP 拥塞窗口限制。每次拷贝、每次排队、每次校验和计算都是纯粹的固定成本。

这种设计的后果在实际测试中很明显。起点阶段 iperf TCP 约 207-283 Mbits/sec，UDP 约 19-39 Mbits/sec，所有单项得分都是 1.0（被 baseline 保底）。即使补齐 socket option 和扩展 buffer 后，TCP 仍只能达到几百 Mbits/sec。因此，loopback 路径优化不是调参，而是架构调整。

=== Router 直投与大 MTU

优化的第一步是让 router 识别 loopback 流量。当目的地址是 loopback 且 RX buffer 有空间时，router 直接把包放入 RX buffer，不经过设备队列。

#figure(
  [
    ```rust
    // Router 判断目的地址
    let dst_addr = packet.dst_ip_addr();

    if dst_addr.is_loopback() {
        // 目的是 loopback，尝试直投
        if let Some(device) = self.get_loopback_device() {
            if self.rx_buffer.lock().can_enqueue() {
                // 直接放入 RX buffer
                self.rx_buffer.lock().enqueue(packet);
                device.notify_rx_ready();
                return Ok(());
            }
        }
    }

    // 回退到普通发送路径
    self.tx_buffer.lock().enqueue(packet);
    ```
  ],
  caption: [Router 对 loopback 包的直投逻辑],
) <lst:router-loopback>

这一改动减少了至少一次内存拷贝和两次队列操作。本机流量不再必须经历 `tx_buffer → loopback device buffer → rx_buffer` 的多次排队。

优化的第二步是为 loopback 使用独立的大 MTU。外部网卡仍使用标准 MTU 1500，loopback 使用 65535。smoltcp 发送 IP 包时按目的地址选择 MTU：loopback 走大 MTU，非 loopback 走正常 MTU。

#figure(
  [
    ```rust
    // 设备能力区分 loopback MTU
    pub struct DeviceCapabilities {
        pub max_transmission_unit: usize,  // 标准 MTU
        pub loopback_transmission_unit: usize,  // Loopback MTU
    }

    // 发送时按目的地址选择 MTU
    let mtu = if dst_addr.is_loopback() {
        caps.loopback_transmission_unit  // 65535
    } else {
        caps.max_transmission_unit  // 1500
    };
    ```
  ],
  caption: [Loopback 大 MTU 配置],
) <lst:loopback-mtu>

大 MTU 使得 TCP 可以使用大 MSS，减少分段和协议头处理。一个 64KB 的数据块在 MTU 1500 下需要约 44 个包，在 MTU 65535 下只需要 1 个包。Ethernet 设备对超过标准 MTU 的包直接丢弃，避免 loopback 大包误走外部网卡。

这两项改动使得 TCP loopback 从几百 Mbits/sec 提升到 Gbits/sec 级别。

=== TCP loopback 绕过拥塞窗口

2.5.2 节说明了 router 直投和大 MTU 的效果，但 TCP 还受拥塞窗口限制。Linux TCP 拥塞控制是为外部网络设计的，通过慢启动、拥塞避免和快速重传应对网络拥塞和丢包。对于 loopback 这种内存内投递，拥塞控制算法反而成为限制。

问题的核心在于 cwnd（拥塞窗口）的增长速度。即使 loopback 有很大的接收窗口和 buffer 空间，TCP 仍然从小 cwnd 开始，每个 RTT 才能增长。虽然 loopback 的 RTT 很小（微秒级），但仍需要多个 RTT 才能达到满吞吐。更重要的是，某些 ACK/seq 状态下，发送逻辑可能因为 cwnd 限制而暂停，即使远端还有大量 buffer 空间。

优化让 TCP loopback tuple 绕过拥塞窗口限制。对目的地址是 loopback 的连接，发送窗口只受远端窗口、MSS 和 buffer 限制，不再受 cwnd 限制。

#figure(
  [
    ```rust
    // TCP 发送窗口计算
    let send_window = if self.is_loopback_tuple() {
        // Loopback：只受远端窗口和 buffer 限制
        min(remote_window, tx_buffer_available)
    } else {
        // 外部连接：受 cwnd 限制
        min(cwnd, min(remote_window, tx_buffer_available))
    };
    ```
  ],
  caption: [TCP loopback 绕过拥塞窗口],
) <lst:tcp-loopback-cwnd>

优化还修复了 `next_transmit_seq()`，避免某些 ACK/seq 状态下发送偏移回退。TCP SYN 里 advertised MSS 对 loopback 使用大 MTU 对应的 MSS。`TCP_INFO` 中返回当前 path 的 MSS/PMTU，应用可以看到 loopback 的真实配置。

此外，loopback TCP tuple 不再因为 delayed ACK timer 等待 ACK 发送，也绕过 Nagle 的"小包已有未确认数据时等待 ACK"限制。这些改动不改变用户态 `TCP_NODELAY` 选项状态，只是让内核对 loopback 使用更激进的策略。

当前 iperf 结果中，glibc TCP 为 2293.76 / 2775.04 / 2088.96 Mbits/sec，musl TCP 为 2181.12 / 2508.80 / 2109.44 Mbits/sec。相比起点的 207-283 Mbits/sec，提升约 10 倍。

=== UDP loopback 直达 socket

TCP 的优化已经显著，但 UDP 还有更大的空间。2.5.3 节说明了 TCP 绕过 cwnd，UDP 可以更进一步：直接写入目标 socket 的 RX queue。

问题在于 UDP loopback 仍走协议栈路径。发送端构造完整 IP/UDP 包，计算 checksum，经过 router，loopback device，再由接收端解包、校验 checksum、匹配 socket。对于本机投递，这些步骤都是多余的：我们知道目的地址，可以直接找到目标 socket；不经过物理链路，不需要 IP 封包和 checksum。

优化增加 `UdpSocket::try_loopback_send()`。判断目的地址是 loopback 时，不构造 IP/UDP 包，而是扫描 socket set，找到匹配的本地 UDP socket，直接把 payload 写入目标 socket 的 RX queue。

#figure(
  [
    ```rust
    pub fn try_loopback_send(&self, data: &[u8], remote: SocketAddr)
        -> Option<usize> {
        if !remote.ip().is_loopback() {
            return None;  // 不是 loopback，走普通路径
        }

        // 扫描 socket set，找到匹配的 socket
        let sockets = SOCKET_SET.lock();
        for socket in sockets.iter() {
            if socket.can_receive(remote, self.local_addr) {
                // 直接写入目标 socket 的 RX queue
                if let Ok(written) = socket.rx_enqueue(data) {
                    socket.notify_rx_ready();
                    return Some(written);
                }
            }
        }

        None  // 找不到目标或 RX queue 满，回退
    }
    ```
  ],
  caption: [UDP loopback 直达 socket RX queue],
) <lst:udp-loopback-direct>

直投匹配规则区分 connected socket 和 unconnected listener。Connected UDP socket 只接收精确 peer 的包，避免新 stream 的握手包被投递到旧 connected socket。Unconnected listener 接收新 peer 的包。如果目标 RX queue 满了或找不到目标 socket，则回退到普通网络栈路径。

成功直投后通过 `notify_rx_ready()` 唤醒接收侧。为避免长时间占住 CPU，增加 `UDP_LOOPBACK_YIELD_BYTES`，loopback 连续发送一定字节后主动 yield。

这一优化的效果最为显著。从 0612-180317 到 0612-191510，UDP 提升如下：

#figure(
  table(
    columns: (1.2fr, 1.8fr, 1.5fr, 1.5fr),
    inset: 5pt,
    stroke: 0.5pt + gray,
    table.header([libc], [项目], [直投前 (Mbits/sec)], [直投后 (Mbits/sec)]),
    [glibc], [BASIC_UDP], [647.00], [1566.72],
    [glibc], [PARALLEL_UDP], [560.00], [2140.16],
    [glibc], [REVERSE_UDP], [705.00], [1658.88],
    [musl], [BASIC_UDP], [603.00], [1597.44],
    [musl], [PARALLEL_UDP], [536.00], [2273.28],
    [musl], [REVERSE_UDP], [711.00], [1669.12],
  ),
  caption: [UDP loopback 直投前后对比],
) <tab:udp-loopback-perf>

UDP 从约 0.6-0.7 Gbits/sec 提升到约 1.6-2.27 Gbits/sec，提升约 2.5-4 倍。

当前实现仍需要扫描 socket set。连接数更多时，socket 查找和全局锁会成为新瓶颈。后续可以考虑按 `(local_addr, local_port, peer)` 建立快速查找表，减少扫描开销。

== 轮询策略优化

=== 每次 I/O 都 poll 的开销

早期版本的轮询策略过于保守。每次 `send()`/`recv()` 都无条件推进网络栈，`select()`/`poll()`/`epoll_wait()` 也在检查每个 fd 时重复推进。

早期版本的轮询策略过于保守。每次 `send()`/`recv()` 都无条件推进网络栈，`select()`/`poll()`/`epoll_wait()` 也在检查每个 fd 时重复推进。

问题不在推进本身，而在频率过高。Loopback 已经有数据可读/可写的场景下，socket 可以立即完成操作，无需推进网络栈。但早期版本仍然每次都 `poll_interfaces()`，遍历所有设备和 socket，检查状态变化。对于 iperf/netperf 这种大量 send/recv + poll/select 的测试，累积的轮询开销显著。

=== 按需推进与批量处理

优化将轮询策略改为按需推进。`send()`/`recv()` 先尝试一次真实操作，只有遇到 `WouldBlock` 才推进网络栈并进入等待。

#figure(
  [
    ```rust
    pub fn send(&self, data: &[u8]) -> AxResult<usize> {
        // 先尝试一次发送
        match self.socket.send(data) {
            Ok(n) => return Ok(n),
            Err(AxError::WouldBlock) => {
                // 只有 WouldBlock 时才 poll 并等待
                self.network_send_poller().wait()?;
            }
            Err(e) => return Err(e),
        }
        // 等待后重试
        self.socket.send(data)
    }
    ```
  ],
  caption: [按需推进网络栈],
) <lst:poll-on-demand>

`select()`/`poll()`/`epoll_wait()` 在检查 fd readiness 前统一推进一次，避免每个 socket 重复推进。用户态阻塞等待网络 fd 时，不需要每个 socket 自己重复推进网络栈。

#figure(
  [
    ```rust
    // 在 poll/select/epoll 开始时统一推进
    fn poll_network_once() {
        axnet::poll_interfaces();
    }

    pub fn sys_poll(fds: &mut [PollFd], timeout: i32) -> LinuxResult<usize> {
        poll_network_once();  // 统一推进一次
        // 然后检查各个 fd 的 readiness
        ...
    }
    ```
  ],
  caption: [I/O 多路复用统一推进网络栈],
) <lst:poll-unified>

TCP send 还增加批量处理。不再每次成功写入后都立刻 `poll_interfaces()`，而是累积到一定字节数后再 poll。


#figure(
  [
    ```rust
    const TCP_SEND_POLL_BYTES: usize = 64 * 1024;

    pub fn send(&mut self, data: &[u8]) -> AxResult<usize> {
        let sent = self.socket.send(data)?;
        self.bytes_since_poll += sent;

        // 累积到阈值后才 poll
        if self.bytes_since_poll >= TCP_SEND_POLL_BYTES {
            poll_interfaces();
            self.bytes_since_poll = 0;
        }

        Ok(sent)
    }
    ```
  ],
  caption: [TCP send 批量 poll],
) <lst:tcp-batch-poll>

如果 tx buffer 满导致 `WouldBlock`，仍然走原来的网络 poller 推进网络栈。这是通用 TCP 发送路径批处理，不依赖应用名称或命令参数。

netperf TCP_STREAM 从约 20-22 Mbits/sec 提升到约 50-52 Mbits/sec，说明每次 send 后立即全局 poll 是明显的固定成本。

当前实现的批量阈值是固定的 64KB。对于不同的网络条件和应用模式，最优阈值可能不同。未来可以考虑自适应调整批量大小。

== 性能提升与对比

=== 优化效果演进

优化从"低于 baseline/功能不稳定"逐步提升到"稳定超过 baseline 并接近 Ubuntu 参考结果"。主要阶段如下：

#figure(
  table(
    columns: (2.2fr, 1.5fr, 1.5fr, 2.8fr),
    inset: 5pt,
    stroke: 0.5pt + gray,
    table.header([阶段], [glibc 总分], [musl 总分], [关键现象]),
    [起点], [6.000000], [6.000000], [TCP 约 207-283 Mbits/sec，UDP 约 19-39 Mbits/sec，被 baseline 保底],
    [初步拉升], [6.006242], [6.000000], [BASIC_TCP 刚过 baseline，UDP 和并发 TCP 仍低],
    [功能稳定], [8.825720], [8.701600], [TCP 到 2.0-2.5 Gbits/sec，UDP 到 0.53-0.71 Gbits/sec],
    [当前版本], [10.114469], [10.068430], [UDP 到 1.56-2.27 Gbits/sec，TCP 保持在 2.1-2.8 Gbits/sec],
    [Ubuntu 参考], [10.740733], [10.604013], [Ubuntu 总分仍更高，尤其 TCP 和部分 UDP 仍有差距],
  ),
  caption: [iperf 性能演进],
) <tab:iperf-evolution>

起点到当前版本的主要变化不是单纯调参数，而是先补齐 socket 语义和测试可运行性，再减少轮询、排队、拷贝和拥塞窗口带来的本机 loopback 开销。


=== 优化效果分析

地址族支持使得 iperf 能够正确运行。早期 `::ffff:127.0.0.1` 导致 panic，补齐 IPv4/IPv6 区分和 `IPV6_V6ONLY` 后，iperf 在 glibc/musl 下混用 IPv4/IPv6/IPv4-mapped IPv6 均能正常工作。

Socket 语义补齐让应用不进入异常路径。netperf 的 `enable_enobufs`、`so_dontroute`、`errno 92` 提示消失后，TCP_STREAM 从约 11 Mbits/sec 提升到约 21-22 Mbits/sec。`SO_SNDBUF`/`SO_RCVBUF` 按 Linux 语义翻倍后，用户设置的 buffer 能真正生效。

Buffer 扩展解除吞吐上限。TCP buffer 从 64KB 扩展到 512KB，UDP buffer 从 64KB 扩展到 1MB。扩展后，BASIC_TCP 先接近或超过 baseline，后续配合 loopback 优化，TCP 才能拉到 Gbits/sec 级别。

Loopback 优化是最显著的改进。Router 直投减少排队和拷贝，大 MTU 减少分段，TCP 绕过 cwnd 解除拥塞限制，UDP 直达 socket 避免协议栈封包。TCP 从起点的 207-283 Mbits/sec 提升到当前的 2.1-2.8 Gbits/sec，提升约 10 倍。UDP 从 0.6-0.7 Gbits/sec 提升到 1.6-2.27 Gbits/sec，提升约 2.5-4 倍。

轮询策略优化减少固定成本。按需推进和批量 poll 使得 netperf TCP_STREAM 从约 20-22 Mbits/sec 提升到约 50-52 Mbits/sec。对 loopback 已经有数据的场景，减少了不必要的全局轮询。

=== 与 Ubuntu 的对比

#figure(
  table(
    columns: (2fr, 4fr),
    inset: 5pt,
    stroke: 0.5pt + gray,
    table.header([方面], [差距分析]),
    [TCP 吞吐], [Ubuntu glibc TCP 为 3041.28 / 3573.76 / 3276.80 Mbits/sec，StarryOS 为 2293.76 / 2775.04 / 2088.96 Mbits/sec],
    [UDP 吞吐], [Ubuntu UDP 可达 2.7-3.8 Gbits/sec，StarryOS 为 1.6-2.27 Gbits/sec],
    [数据路径], [StarryOS TCP 仍通过 smoltcp socket buffer 和协议状态机搬运数据，Linux loopback 有更深的批量化和调度优化],
    [Socket 查找], [StarryOS UDP 直投仍需扫描 socket set，连接数多时成为瓶颈；Linux 有更高效的查找结构],
  ),
  caption: [与 Ubuntu 的差距分析],
) <tab:ubuntu-comparison>

与 Ubuntu 相比，StarryOS 的主要差距在数据路径的成熟度上。当前 TCP 仍通过 smoltcp socket buffer 和协议状态机搬运数据，没有做到 Linux loopback 那种更深的批量化和调度优化。UDP 直投仍需要扫描 socket set，连接数更多时，socket 查找和全局锁会成为新瓶颈。

当前已经超过 baseline，说明核心设计思想（2.2 节所述的四个关键抽象）是合理的。性能差距主要在工程实现的成熟度上，而非架构设计上的根本缺陷。

后续优化优先方向：给 UDP loopback 建立按 `(local_addr, local_port, peer)` 的快速查找表，减少扫描 socket set；对 TCP/UDP loopback 做批量收发，降低每个 datagram/segment 的锁和 wake 成本；拆分全局 socket set 锁，减少并发场景下的锁竞争。

== 本章小结

优化选择沿用 StarryOS 的网络栈框架，围绕四条主线完成加固：以地址族分层和 loopback 分离组成架构基础，以 IPv4/IPv6/IPv4-mapped IPv6 支持完善地址族语义，以 router 直投、大 MTU、绕过 cwnd 和 UDP 直达构成 loopback 快路径，并把按需轮询和批量推进串成完整优化链。

在此基础上，优化补齐 `TCP_INFO`、`SO_*BUF`、`IPV6_V6ONLY` 等 socket 语义，扩展 TCP buffer 到 512KB、UDP buffer 到 1MB，实现 loopback router 直投和 UDP 直达 socket，优化轮询策略为按需推进和批量处理。当前尚未实现自适应批量、UDP 快速查找表和 socket set 锁拆分，但网络栈从地址族到 loopback 路径的主要优化关系已经比基线更完整。

