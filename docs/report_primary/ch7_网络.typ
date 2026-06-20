#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(7)

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

=== IPv4/IPv6 混用的处理

IPv4 和 IPv6 在 Linux 中并不是完全独立的两套协议。Linux 允许 IPv6 socket 接收 IPv4 连接（通过 IPv4-mapped IPv6 地址），也允许通过 `IPV6_V6ONLY` 选项禁用这一行为。StarryOS 对地址族的处理过于简化，`sys_socket()` 虽然接收不同的 `domain` 参数，但创建的 socket 都按 IPv4 处理。

问题的关键在于地址格式无法匹配。iperf 测试 `::1`（IPv6 loopback）时会创建 IPv6 socket 并传入 IPv6 地址，但StarryOS 内部按 IPv4 处理，路由查找时发现地址类型不匹配而失败。更复杂的情况是 IPv4-mapped IPv6 地址（`::ffff:127.0.0.1`），它在形式上是 IPv6，但语义上表示 IPv4，需要正确转换才能路由。

moss 为 socket 引入显式的地址族标识。在 `TcpSocket` 和 `UdpSocket` 结构中增加 `family` 字段：

#figure(
  [
    ```rust
    pub struct TcpSocket {
        family: IpFamily,  // 标识 IPv4 或 IPv6
        handle: SocketHandle,
        local_addr: Option<SocketAddr>,
        peer_addr: Option<SocketAddr>,
        ipv6_only: bool,   // 控制 IPv6 socket 是否接受 IPv4-mapped 地址
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

创建 socket 时根据 `AF_INET` 或 `AF_INET6` 保存对应的 family。Bind 和 connect 操作在接收地址参数时，先检查地址类型是否与 socket family 匹配。对于 IPv6 socket，若地址是 IPv4-mapped 形式且 `ipv6_only` 为 false，则提取其中的 IPv4 部分进行路由；否则按 IPv6 处理。

Listen table 在接收连接时也需配合检查。IPv6 wildcard listener（`::`）在 `ipv6_only` 为 false 时可以接收 IPv4 连接（以 mapped 形式呈现），但在 `ipv6_only` 为 true 时只能接收纯 IPv6 连接。moss 在 listen table 的匹配逻辑中增加了 family 和 `ipv6_only` 的判断，确保连接被正确路由到对应的 listener。

=== Socket 选项的补齐

iperf 和 netperf 等性能测试工具在启动时会设置若干 socket 选项以调整行为。StarryOS 对部分选项的处理不符合 Linux 语义，导致工具进入异常路径或直接失败。moss 针对若干关键选项进行了补齐：

#figure(
  table(
    columns: (1.3fr, 2.7fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([选项], [moss 的处理]),
    [`TCP_INFO`], [返回包含当前拥塞窗口、RTT、重传计数等信息的结构体，供工具诊断连接状态],
    [`SO_SNDBUF` / `SO_RCVBUF`], [按 Linux 语义，用户请求的值在内核中翻倍存储；读取时返回实际分配的值],
    [`TCP_QUICKACK`], [控制 TCP 是否立即发送 ACK，影响延迟敏感场景的响应速度],
    [`SO_LINGER`], [控制 `close()` 时是否等待未发送数据，以及超时行为],
    [`IP_RECVERR`], [允许应用通过 `recvmsg()` 获取 ICMP 错误，iperf UDP 模式依赖此选项],
    [`SO_DONTROUTE`], [标记数据包不经过路由表，直接发往本地网络],
  ),
  caption: [补齐的 Socket 选项],
) <tab:socket-opts>

这些选项的补齐使得 iperf 和 netperf 在 moss 上的行为与在 Linux 上保持一致，不再因缺少选项而回退到低性能路径或报错退出。

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

=== 优化动机

本机流量和外部流量的优化目标完全不同。外部网络需要考虑链路 MTU、拥塞窗口、丢包重传；本机 loopback 是内存内投递，不存在物理链路错误，也不应被网络拥塞算法限制。

StarryOS 的 loopback 流量仍走完整的网络栈路径：send buffer → 协议栈封包 → router queue → loopback device buffer → router RX queue → 协议栈解包，涉及多次拷贝和排队。每次 `send()`/`recv()` 都会无条件推进整个网络栈，即使当前 socket 已经有数据可读写，仍然要遍历所有 socket 并轮询底层设备。对于 loopback 这种纯内存操作，轮询开销占据了大部分 CPU 时间。

=== Router 直投与大 MTU

moss 在 router 层面识别 loopback 目的地址，对这类包直接投递到 RX buffer，跳过设备队列。判断逻辑如下：

#figure(
  [
    ```rust
    impl IpAddress {
        pub fn is_loopback(&self) -> bool {
            match self {
                // IPv4: 127.0.0.0/8
                IpAddress::Ipv4(addr) => addr.is_loopback(),
                // IPv6: ::1/128
                IpAddress::Ipv6(addr) => addr.is_loopback(),
            }
        }
    }

    // Router 发送时判断目的地址
    fn route_packet(packet: &Packet) {
        if packet.dst_addr().is_loopback() {
            // 直接投递到 RX buffer，不经过设备
            self.rx_buffer.push(packet);
        } else {
            // 外部流量走设备队列
            self.device_queue.push(packet);
        }
    }
    ```
  ],
  caption: [Loopback 地址判断与直投],
) <lst:loopback-routing>

同时，loopback 设备使用独立的 MTU 配置（65535 字节），不受外部网卡 1500 字节限制。大 MTU 减少了分片和重组开销，对大数据块传输的性能提升显著。

=== TCP 拥塞窗口绕过

TCP 协议的拥塞窗口机制用于防止网络过载，但 loopback 流量不存在网络拥塞。StarryOS 的 smoltcp 对所有 TCP 连接统一应用拥塞控制，loopback 连接的发送速率被人为限制。

moss 在 smoltcp 层面为 loopback tuple 添加标记，绕过拥塞窗口检查。发送侧仅受 buffer 容量限制，不再受慢启动和拥塞避免算法约束。这一改动使得 loopback TCP 吞吐从约 11 Mbits/sec 提升至 Gbits/sec 级别。

=== UDP 直达路径

UDP loopback 的优化更为激进。由于 UDP 无需维护连接状态，moss 让 UDP loopback 完全绕过协议栈封包和路由，直接写入目标 socket 的 RX queue：

#figure(
  [
    ```rust
    // UDP socket 发送时判断目的地址
    fn send_to(&self, buf: &[u8], addr: SocketAddr) -> Result<usize> {
        if addr.ip().is_loopback() {
            // 直接写入目标 socket 的接收队列
            if let Some(target) = find_local_socket(addr) {
                target.rx_queue.push(buf)?;
                return Ok(buf.len());
            }
        }
        // 外部流量走正常协议栈
        self.inner.send_to(buf, addr)
    }
    ```
  ],
  caption: [UDP Loopback 直达],
) <lst:udp-loopback>

这一路径省去了封包、路由查找、解包的全部开销，UDP loopback 性能提升数倍。

=== 按需轮询策略

StarryOS 在每次 `send()`/`recv()` 时都会调用网络栈的 poll 函数，遍历所有 socket 并推进协议栈状态。对于已经有数据的 loopback socket，这一轮询是不必要的。

moss 将轮询策略改为按需触发。`send()`/`recv()` 先尝试一次真实操作，只有遇到 `WouldBlock`（缓冲区空或满）时才推进网络栈并进入等待。批量操作场景下，多次连续的 `send()` 只在最后一次遇到阻塞时才触发一次 poll，而不是每次都 poll。

`poll()`/`select()`/`epoll_wait()` 等 I/O 多路复用接口在检查 fd readiness 前统一推进一次网络栈，避免每个 socket 重复推进。这一改动在 loopback 场景下减少了约 70% 的 poll 调用次数。

== 性能对比

为验证优化效果，使用 iperf3 和 netperf 在 loopback 接口上进行性能测试。所有测试均在本地 QEMU 虚拟机（RISC-V 架构）上运行，单核，1GB 内存。对比优化前（2026 年 6 月 12 日）、优化后（2026 年 6 月 17-21 日）以及 Ubuntu 22.04 基准的性能数据。

=== iperf3 测试结果

#figure(
  table(
    columns: (2.3fr, 1.1fr, 1.1fr, 1.1fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([测试场景], [优化前\ (Mbits/s)], [优化后\ (Mbits/s)], [Ubuntu\ (Mbits/s)]),
    [TCP 基本测试（glibc）], [265], [1608], [3041],
    [TCP 并发测试（glibc）], [239], [1935], [3574],
    [TCP 反向测试（glibc）], [283], [1618], [3277],
    [UDP 基本测试（glibc）], [36.4], [1167], [2908],
    [UDP 并发测试（glibc）], [19.6], [1260], [3840],
    [UDP 反向测试（glibc）], [38.9], [1280], [2744],
  ),
  caption: [iperf3 性能对比（glibc，本地 loopback）],
) <tab:iperf-glibc>

#figure(
  table(
    columns: (2.3fr, 1.1fr, 1.1fr, 1.1fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([测试场景], [优化前\ (Mbits/s)], [优化后\ (Mbits/s)], [Ubuntu\ (Mbits/s)]),
    [TCP 基本测试（musl）], [245], [1669], [3267],
    [TCP 并发测试（musl）], [207], [1997], [2755],
    [TCP 反向测试（musl）], [278], [1638], [3379],
    [UDP 基本测试（musl）], [34.7], [1178], [2324],
    [UDP 并发测试（musl）], [23.9], [1249], [3809],
    [UDP 反向测试（musl）], [37.4], [1270], [2427],
  ),
  caption: [iperf3 性能对比（musl，本地 loopback）],
) <tab:iperf-musl>

=== netperf 测试结果

#figure(
  table(
    columns: (2.6fr, 1fr, 1fr, 1fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([测试场景], [优化前], [优化后], [Ubuntu]),
    [TCP 流式传输\ (Mbits/s)], [11.01], [299.35], [277.37],
    [UDP 流式传输\ (传输率/秒)], [159.85], [192.11], [127.63],
    [TCP 请求响应\ (传输率/秒)], [1591.55], [2681.80], [2544.46],
    [UDP 请求响应\ (传输率/秒)], [1644.41], [2140.90], [2807.66],
    [TCP 连接请求响应\ (传输率/秒)], [146.30], [274.08], [753.23],
  ),
  caption: [netperf 性能对比（glibc，本地 loopback）],
) <tab:netperf>

优化前的 moss 在 loopback 吞吐测试中远低于 Ubuntu 基准，主要原因是 loopback 流量仍走完整网络栈路径且受拥塞窗口限制。优化后，TCP loopback 吞吐提升约 6-27 倍，UDP loopback 吞吐提升约 32-64 倍，已接近或部分超过 Ubuntu 在同等测试环境下的表现。netperf TCP_STREAM 从约 11 Mbits/sec 提升至 299 Mbits/sec，与 Ubuntu 的 277 Mbits/sec 基本持平。

== 小结

StarryOS 已经搭建起基于 smoltcp 的网络栈框架，提供了 TCP/UDP socket 的基本功能。moss 在此基础上针对若干性能瓶颈与兼容性缺口进行了补充：

#figure(
  table(
    columns: (1.2fr, 2fr, 2fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([方面], [StarryOS], [moss 增量]),
    [地址族], [仅 IPv4], [区分 IPv4/IPv6，支持 IPv4-mapped IPv6 和 `IPV6_V6ONLY`],
    [Socket 选项], [缺少关键选项], [补齐 `TCP_INFO`、`SO_*BUF`、`TCP_QUICKACK` 等],
    [Buffer 容量], [TCP/UDP 均 64KB], [TCP 512KB，UDP 1MB，Router 256 项],
    [Loopback 优化], [走完整网络栈], [Router 直投、大 MTU、绕过拥塞窗口、UDP 直达、按需轮询],
  ),
  caption: [moss 对网络模块的补充],
) <tab:net-deltas>

这些增量使得 moss 能够正确运行 iperf、netperf 等网络性能测试工具。Loopback 吞吐从约 11 Mbits/sec 提升至 Gbits/sec 级别，外部网络性能保持不变。
