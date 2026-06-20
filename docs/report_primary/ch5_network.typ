#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(5)

= 网络模块

网络模块负责组织网络数据的收发与传输。moss 基于 StarryOS 改造而来，网络栈的整体框架沿用了 StarryOS 已有的设计。StarryOS 基于 smoltcp 协议栈实现了 TCP/UDP socket 的基本功能，moss 在此基础上针对若干性能瓶颈与兼容性缺口进行了补充。本章先介绍 StarryOS 网络栈的整体结构与现存问题，随后说明 moss 所做的增量。

== StarryOS 的网络栈

=== 整体结构

StarryOS 已经建立了可工作的网络栈框架。系统基于 smoltcp 协议栈，通过 axnet 模块封装 TCP/UDP socket，向上提供 `socket`、`bind`、`connect`、`send`、`recv` 等基础系统调用接口。在设备层，StarryOS 支持以太网设备和 loopback 设备，能够处理 IPv4 流量。

收发路径的基本框架也已搭建完成。应用程序通过 socket 系统调用发送数据，数据经过协议栈封包后通过设备发送；接收路径通过轮询设备并推进协议栈状态。因此，网络栈面对的不是一片空白，而是主干结构已经成形，但在地址族支持、buffer 配置、loopback 路径优化等方面存在不足的实现。

=== 现存问题

StarryOS 的网络栈在简单 TCP/UDP 通信场景下可以工作，但在复杂负载下暴露出若干问题：

1. *地址族支持不完整*。StarryOS 虽然接收 `AF_INET` 和 `AF_INET6` 参数，但内部统一按 IPv4 处理。IPv4-mapped IPv6 地址（如 `::ffff:127.0.0.1`）会导致路由查找失败。iperf 等工具在 glibc/musl 环境下会混用 IPv4、IPv6 和 mapped 地址，基线无法正确处理这些组合。

2. *Socket 选项语义缺失*。缺少 iperf/netperf 等性能测试工具依赖的关键选项。`TCP_INFO` 返回固定假值，`SO_SNDBUF`/`SO_RCVBUF` 不按 Linux 语义翻倍，`IP_RECVERR`、`SO_DONTROUTE` 等返回 `ENOPROTOOPT` 导致应用进入异常路径。

3. *Buffer 容量偏小*。TCP 和 UDP 的发送/接收 buffer 均为 64KB，在高吞吐场景下成为瓶颈。netperf 的 TCP_STREAM 测试仅能达到约 11 Mbits/sec，远低于预期。

4. *Loopback 路径未优化*。本机 `127.0.0.1` 流量仍走完整的外部网络路径，经过多次拷贝和排队。每次 `send()`/`recv()` 都无条件推进整个网络栈，累积的轮询开销显著。

== 地址族支持与兼容性补齐

IPv4 和 IPv6 在 Linux 中并不是完全独立的两套协议。Linux 允许 IPv6 socket 接收 IPv4 连接（通过 IPv4-mapped IPv6 地址），也允许通过 `IPV6_V6ONLY` 选项禁用这一行为。StarryOS 对地址族的处理过于简化，`sys_socket()` 虽然接收不同的 `domain` 参数，但创建的 socket 都按 IPv4 处理。

moss 为 socket 引入显式的地址族标识。在 `TcpSocket` 和 `UdpSocket` 结构中增加 `family` 字段：

#figure(
  [
    ```rust
    pub struct TcpSocket {
        family: IpFamily,  // 标识 IPv4 或 IPv6
        handle: SocketHandle,
        local_addr: Option<SocketAddr>,
        peer_addr: Option<SocketAddr>,
        // ...
    }

    pub enum IpFamily {
        Ipv4,
        Ipv6,
    }
    ```
  ],
  caption: [Socket 中的地址族标识],
) <lst:socket-family>

创建 socket 时根据 `AF_INET` 或 `AF_INET6` 保存对应的 family，后续 bind、connect、accept 操作均按 family 判断地址格式是否匹配。Listen table 在接收连接时检查 family 和 `ipv6_only` 标志，避免 IPv6 wildcard listener 错误接收 IPv4 连接。

在 socket 选项方面，moss 补齐了 `TCP_INFO`、`SO_SNDBUF`、`SO_RCVBUF`、`TCP_QUICKACK`、`SO_LINGER`、`IP_RECVERR`、`SO_DONTROUTE` 等选项的基本语义，使 iperf 和 netperf 不再因缺少选项而进入异常路径。

== Buffer 扩展

StarryOS 原有的 64KB buffer 在高吞吐场景下容易成为瓶颈。发送方或接收方频繁因 buffer 满而阻塞，限制了吞吐上限。moss 将 buffer 容量提升至适合高吞吐场景的配置：

#figure(
  [
    ```rust
    // TCP buffer 从 64KB 提升到 512KB
    const DEFAULT_TCP_RX_BUF_LEN: usize = 512 * 1024;
    const DEFAULT_TCP_TX_BUF_LEN: usize = 512 * 1024;

    // UDP buffer 从 64KB 提升到 1MB
    const DEFAULT_UDP_RX_BUF_LEN: usize = 1024 * 1024;
    const DEFAULT_UDP_TX_BUF_LEN: usize = 1024 * 1024;

    // Router packet buffer 提升到 256 项
    const ROUTER_PACKET_BUFFER_LEN: usize = 256;
    ```
  ],
  caption: [Buffer 容量配置],
) <lst:buffer-config>

扩展后的 buffer 使得发送方和接收方不再因容量限制而频繁阻塞，吞吐能力显著提升。

== Loopback 路径优化

本机流量和外部流量的优化目标完全不同。外部网络需要考虑链路 MTU、拥塞窗口、丢包重传；本机 loopback 是内存内投递，不存在物理链路错误，也不应被网络拥塞算法限制。

StarryOS 的 loopback 流量仍走完整的网络栈路径：send buffer → 协议栈封包 → router queue → loopback device buffer → router RX queue → 协议栈解包，涉及多次拷贝和排队。moss 将 loopback 路径从通用网络路径中分离：

1. *Router 直投*：对目的地址是 loopback 的包，router 直接投递到 RX buffer，不经过设备队列。

2. *大 MTU*：Loopback 使用独立的 MTU（65535 字节），不受外部网卡 1500 字节限制，减少分片开销。

3. *TCP 绕过拥塞窗口*：Loopback TCP tuple 不受拥塞窗口限制，仅受 buffer 容量限制。

4. *UDP 直达*：UDP loopback 直接写入目标 socket 的 RX queue，不经过协议栈封包和路由。

5. *按需轮询*：`send()`/`recv()` 先尝试一次真实操作，只有遇到 `WouldBlock` 才推进网络栈。批量操作场景下避免了重复轮询。

这些优化使得本机流量只付出必要的 socket 层成本，而不是完整的网络栈成本。

== 小结

StarryOS 已经搭建起基于 smoltcp 的网络栈框架，提供了 TCP/UDP socket 的基本功能。moss 在此基础上针对若干性能瓶颈与兼容性缺口进行了补充：

#figure(
  table(
    columns: (1.2fr, 2fr, 2fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([方面], [StarryOS 基线], [moss 增量]),
    [地址族], [仅 IPv4], [区分 IPv4/IPv6，支持 IPv4-mapped IPv6],
    [Socket 选项], [缺少关键选项], [补齐 `TCP_INFO`、`SO_*BUF` 等],
    [Buffer 容量], [TCP/UDP 均 64KB], [TCP 512KB，UDP 1MB],
    [Loopback], [走完整网络栈], [Router 直投、大 MTU、按需轮询],
  ),
  caption: [moss 对网络模块的补充],
) <tab:net-deltas>

这些增量使得 moss 能够运行 iperf、netperf 等网络性能测试工具，loopback 吞吐从约 11 Mbits/sec 提升至 Gbits/sec 级别。
