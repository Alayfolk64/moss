# iperf 网络栈优化过程记录

这份文档记录本轮 iperf 优化从“低于 baseline/功能不稳定”到“RV 上稳定超过 baseline 并接近 Ubuntu 参考结果”的过程。重点记录三件事：改了哪些代码、解决了什么问题、带来了什么效果。

## 结果演进

| 阶段 | 结果文档 | 状态 | glibc 总分 | musl 总分 | 关键现象 |
|---|---|---|---:|---:|---|
| 起点 | `docs/benchmarks/iperf/results/0612-022826.md` | 全部被 baseline 保底 | 6.000000 | 6.000000 | TCP 约 207-283 Mbits/sec，UDP 约 19-39 Mbits/sec，所有单项得分都是 1.0 |
| 初步拉升 | `docs/benchmarks/iperf/results/0612-050446.md` | 少数项目接近 baseline | 6.006242 | 6.000000 | BASIC_TCP 刚过 baseline，UDP 和并发 TCP 仍低于 baseline |
| 功能仍不稳定 | `docs/benchmarks/iperf/results/0612-154110.md` | glibc 缺 3 项，musl 有提升 | - | 7.235472 | BASIC_TCP/BASIC_UDP 有提升，但 PARALLEL/REVERSE 仍有缺项或低分 |
| 全部项目稳定 | `docs/benchmarks/iperf/results/0612-180317.md` | 6 个 RV 单项均有结果 | 8.825720 | 8.701600 | TCP 到 2.0-2.5 Gbits/sec，UDP 到 0.53-0.71 Gbits/sec |
| 当前版本 | `docs/benchmarks/iperf/results/0612-191510.md` | UDP 明显提升 | 10.114469 | 10.068430 | UDP 到 1.56-2.27 Gbits/sec，TCP 保持在 2.1-2.8 Gbits/sec |
| Ubuntu 参考 | `docs/benchmarks/iperf/results/Ubuntu.md` | 参考上限 | 10.740733 | 10.604013 | Ubuntu RV 总分仍更高，尤其 TCP 和部分 UDP 仍有差距 |

当前版本相对起点的主要变化不是单纯调参数，而是先补齐 socket 语义和测试可运行性，再减少轮询、排队、拷贝、校验和拥塞窗口带来的本机 loopback 开销。

## P0：先让 iperf 正确、稳定地跑完

### 1. 区分 IPv4/IPv6 socket family

涉及代码：

- `kernel/src/syscall/net/socket.rs`
- `kernel/src/syscall/net/addr.rs`
- `vendor/axnet/src/family.rs`
- `vendor/axnet/src/tcp.rs`
- `vendor/axnet/src/udp.rs`
- `vendor/axnet/src/listen_table.rs`

修改内容：

- `sys_socket()` 根据 `AF_INET` / `AF_INET6` 创建 `IpFamily::Ipv4` 或 `IpFamily::Ipv6`。
- `TcpSocket::new()`、`UdpSocket::new()` 保存 family，不再默认都按 IPv4 格式化地址。
- `SocketAddrEx::family()` 返回真实地址族，而不是固定返回 `AF_INET`。
- 新增 `IpFamily` 帮助函数，统一处理 IPv4、IPv6、IPv4-mapped IPv6 和 `IPV6_V6ONLY`。
- listen table 按 family 和 `ipv6_only` 判断包能不能进入对应监听 socket。

效果：

- 修复早期 `::ffff:127.0.0.1` 找不到路由导致 panic 的问题。
- iperf 在 glibc/musl 下会混用 IPv4、IPv6、IPv4-mapped IPv6，这一步是功能正确性的基础。
- 避免一个 IPv6 wildcard listener 错误接收不该接收的 IPv4/IPv6 流量。

### 2. 补齐 iperf 依赖的 socket option

涉及代码：

- `kernel/src/syscall/net/opt.rs`
- `vendor/axnet/src/options.rs`
- `vendor/axnet/src/general.rs`
- `vendor/axnet/src/tcp.rs`
- `vendor/axnet/src/udp.rs`

修改内容：

- 增加 `IPV6_V6ONLY`。
- 增加最小可用的 `TCP_INFO`，返回确定的 TCP state、MSS、PMTU、cwnd 等字段。
- 支持 `SO_SNDBUF`、`SO_RCVBUF`、`SO_SNDBUFFORCE` 的保存和读取。
- TCP/UDP 在 socket 尚未 active 时按用户设置重建 smoltcp buffer。

效果：

- iperf/iperf3 在启动、连接和统计阶段不会因为缺 socket option 走异常路径。
- 用户态看到的 buffer/MSS/PMTU 不再是固定假值，避免应用按过小参数工作。

### 3. 路由错误改为 errno，而不是 kernel panic

涉及代码：

- `vendor/axnet/src/service.rs`
- `vendor/axnet/src/tcp.rs`
- `vendor/axnet/src/udp.rs`

修改内容：

- `Service::get_source_address()` 从直接 `panic!("no route to destination")` 改为返回 `AxResult<IpAddress>`。
- TCP/UDP connect/send 路径向用户态返回 `ENETUNREACH` 类错误。

效果：

- 早期日志里 `no route to destination: ::ffff:127.0.0.1` 会直接 panic，这会中断整个评测。
- 修改后错误能回到 socket syscall 层处理，测试稳定性提高。

### 4. 修复 accept 返回地址

涉及代码：

- `kernel/src/syscall/net/socket.rs`

修改内容：

- `sys_accept4()` 返回新 socket 的 `peer_addr()`，不再误用 `local_addr()`。

效果：

- accept 后用户态拿到的远端地址正确。
- 对依赖连接对端地址的测试和调试输出更接近 Linux 行为。

### 5. 分离 TCP/UDP bind 冲突检查

涉及代码：

- `vendor/axnet/src/wrapper.rs`
- `vendor/axnet/src/tcp.rs`
- `vendor/axnet/src/udp.rs`

修改内容：

- 将原来的通用 bind 检查拆成 `bind_check_tcp()` 和 `bind_check_udp()`。
- wildcard 地址、具体地址、TCP/UDP 不同协议之间按 Linux 更接近的语义判断冲突。

效果：

- 避免 TCP/UDP 端口互相误判冲突。
- 降低 iperf server/client 反复启动时因为端口状态不一致失败的概率。

### 6. iperf 测试环境清理

涉及代码：

- `src/init.sh`

修改内容：

- 增加 `cleanup_iperf_daemons()`，在 musl/glibc iperf 组前后清理后台 `iperf` / `iperf3` server。
- 当前本地验证配置把 `iperf=1`、`iozone=0`，用于缩短 iperf 迭代时间。

效果：

- 避免上一组 libc 遗留的 iperf server 占用端口或接收下一组连接。
- 这属于评测流程稳定性修改；完整评测前需要确认测试开关是否要恢复。

## P1：减少通用网络栈轮询开销

### 1. select/poll/epoll 等待时统一推进网络栈

涉及代码：

- `kernel/src/syscall/io_mpx/mod.rs`
- `kernel/src/syscall/io_mpx/poll.rs`
- `kernel/src/syscall/io_mpx/select.rs`
- `kernel/src/syscall/io_mpx/epoll.rs`

修改内容：

- 新增 `poll_network_once()`。
- `poll()`、`select()`、`epoll_wait()` 在真正检查 fd readiness 前推进一次 `axnet::poll_interfaces()`。

效果：

- 用户态阻塞等待网络 fd 时，不需要每个 socket 自己重复推进网络栈。
- 对 iperf 这种大量 send/recv + poll/select 的测试，可以减少重复全局 poll。

### 2. socket send/recv 只在 WouldBlock 时主动 poll

涉及代码：

- `vendor/axnet/src/general.rs`
- `vendor/axnet/src/tcp.rs`
- `vendor/axnet/src/udp.rs`

修改内容：

- 新增 `network_send_poller()` 和 `network_recv_poller()`。
- TCP/UDP 的 `send()`、`recv()` 先尝试一次真实操作，只有遇到 `WouldBlock` 时才 `poll_interfaces()` 并进入等待。
- `TcpSocket::poll()` 和 `UdpSocket::poll()` 不再每次都无条件 `poll_interfaces()`。

效果：

- 快路径少一次全局网络栈推进。
- 对 loopback 已经有数据可读/可写的场景，减少 syscall 内部固定成本。

## P1：扩大 buffer，解除早期吞吐上限

涉及代码：

- `vendor/axnet/src/consts.rs`
- `vendor/axnet/src/general.rs`
- `vendor/axnet/src/tcp.rs`
- `vendor/axnet/src/udp.rs`
- `vendor/axnet/src/listen_table.rs`

修改内容：

- TCP RX/TX buffer 从 64 KiB 调到 512 KiB。
- UDP RX/TX buffer 从 64 KiB 调到 1 MiB。
- UDP packet metadata 数量调到 1024。
- router packet buffer 数量调到 256。
- socket buffer option 最大值限制为 8 MiB。
- accepted TCP socket 继承 listen socket 的 buffer 配置。

效果：

- 起点阶段 TCP/UDP 都明显低于 baseline，64 KiB buffer 很容易让发送方/接收方频繁阻塞。
- 扩大 buffer 后，BASIC_TCP 先接近或超过 baseline，后续 TCP 才能拉到 Gbits/sec 级别。
- UDP 并发场景不容易因为 metadata 或 payload ring 过小直接丢包。

## P1：loopback 路由直投和大 MTU

### 1. router 对 loopback 包走直接 RX 路径

涉及代码：

- `vendor/axnet/src/router.rs`
- `vendor/axnet/src/device/mod.rs`
- `vendor/axnet/src/device/loopback.rs`
- `vendor/axnet/src/device/ethernet.rs`

修改内容：

- `Device` 增加 `is_loopback()` 和 `notify_rx_ready()`。
- loopback device 标记为 loopback，并支持唤醒接收侧。
- router 在目的路由是 loopback 且能放入 RX buffer 时，直接把包放入 router RX buffer。
- 非 loopback 设备仍走原 send 路径。
- Ethernet 设备对超过标准 MTU 的包直接丢弃，避免 loopback 大包误走外部网卡。

效果：

- 本机 127.0.0.1 流量不再必须经历完整 `tx_buffer -> loopback device buffer -> rx_buffer` 多次排队。
- 减少一次以上内存拷贝和队列操作。
- 这是 TCP loopback 提升到 Gbits/sec 的核心路径之一。

### 2. loopback 使用独立大 MTU

涉及代码：

- `vendor/axnet/src/consts.rs`
- `vendor/axnet/src/router.rs`
- `vendor/starry-smoltcp/src/phy/mod.rs`
- `vendor/starry-smoltcp/src/iface/interface/mod.rs`
- `vendor/starry-smoltcp/src/iface/packet.rs`
- `vendor/starry-smoltcp/src/wire/ip.rs`

修改内容：

- 增加 `LOOPBACK_MTU = 65535`，外部标准 MTU 仍是 1500。
- `DeviceCapabilities` 增加 `loopback_transmission_unit`。
- smoltcp 发送 IP 包时按目的地址选择 MTU：loopback 走大 MTU，非 loopback 走正常 MTU。
- `TxToken::set_dst_addr()` 把目的 IP 传到 axnet router token，router 可以判断是否直投 loopback。
- `IpAddress::is_loopback()` 用于统一判断 IPv4/IPv6 loopback。

效果：

- loopback 可以使用大 MSS/大包减少分段和协议头处理。
- 不会把外部网卡也错误地变成 jumbo MTU。

## P1：TCP loopback 快路径

涉及代码：

- `vendor/starry-smoltcp/src/socket/tcp.rs`
- `vendor/axnet/src/tcp.rs`
- `vendor/starry-smoltcp/src/iface/interface/mod.rs`
- `vendor/starry-smoltcp/src/iface/packet.rs`

修改内容：

- TCP 根据目的地址使用 path-specific MTU/MSS。
- SYN 里 advertised MSS 对 loopback 使用大 MTU 对应的 MSS。
- 对 loopback tuple 保留远端窗口、MSS、发送 buffer 限制，但不再受拥塞窗口限制。
- 修复 `next_transmit_seq()`，避免某些 ACK/seq 状态下发送偏移回退。
- `TCP_INFO` 中返回当前 path 的 MSS/PMTU。

效果：

- Linux loopback 本质上不是物理链路，不应该被普通外部网络的低 cwnd 限制。
- 当前 TCP 结果从早期几百 Mbits/sec 提升到 2.1-2.8 Gbits/sec。
- `0612-191510.md` 中 glibc TCP 为 2293.76 / 2775.04 / 2088.96 Mbits/sec，musl TCP 为 2181.12 / 2508.80 / 2109.44 Mbits/sec。

## P1：UDP loopback 直达 socket RX queue

这是当前最后一轮最明显的优化。

涉及代码：

- `vendor/axnet/src/udp.rs`
- `vendor/starry-smoltcp/src/socket/udp.rs`
- `vendor/starry-smoltcp/src/storage/packet_buffer.rs`

修改内容：

- `UdpSocket::try_loopback_send()` 判断目的地址是 loopback 时，不再先构造完整 IP/UDP 包走 router。
- 发送端扫描 socket set，找到匹配的本地 UDP socket，直接把 payload 写入目标 socket 的 RX queue。
- `PacketBuffer::try_enqueue_with()` 支持先写未提交 payload，闭包成功后才提交 metadata，避免失败时留下半包。
- smoltcp UDP socket 增加 `connected_endpoint`。
- 直投匹配规则区分 connected socket 和 unconnected listener：
  - connected UDP socket 只接收精确 peer 的包。
  - unconnected listener 接收新 peer 的包。
- 如果目标 RX queue 满了或找不到目标 socket，则回退到普通网络栈路径。
- 成功直投后通过 `notify_route_device()` 唤醒接收侧。
- 增加 `UDP_LOOPBACK_YIELD_BYTES`，loopback 连续发送一定字节后主动 yield，避免长时间占住 CPU。

效果：

- 避免 UDP loopback 的 IP 封包、路由、checksum、router queue、device queue 等通用路径成本。
- 修复过一次 PARALLEL_UDP 超时问题：原因是新 stream 的握手包被投递到旧 connected UDP socket；`connected_endpoint` 后按 peer 精确匹配解决。
- `0612-180317.md` 到 `0612-191510.md` 的 UDP 提升：

| libc | 项目 | 直投前 | 直投后 |
|---|---|---:|---:|
| glibc | BASIC_UDP | 647.00 | 1566.72 |
| glibc | PARALLEL_UDP | 560.00 | 2140.16 |
| glibc | REVERSE_UDP | 705.00 | 1658.88 |
| musl | BASIC_UDP | 603.00 | 1597.44 |
| musl | PARALLEL_UDP | 536.00 | 2273.28 |
| musl | REVERSE_UDP | 711.00 | 1669.12 |

## P2：loopback checksum/offload

涉及代码：

- `vendor/starry-smoltcp/src/phy/mod.rs`
- `vendor/starry-smoltcp/src/iface/interface/tcp.rs`
- `vendor/starry-smoltcp/src/iface/interface/udp.rs`
- `vendor/starry-smoltcp/src/iface/packet.rs`

修改内容：

- `ChecksumCapabilities::loopback_transport()` 判断源和目的都是 loopback 时关闭 TCP/UDP checksum。
- TCP/UDP 收包解析和发包填充都使用 loopback-aware checksum capability。

效果：

- loopback 是内存内投递，传输层 checksum 对本机路径没有发现物理链路错误的意义。
- 减少每个 TCP/UDP 包的 checksum 计算和校验成本。
- 对大包和高 PPS UDP 更明显。

## P2：设备和 trait 适配

涉及代码：

- `vendor/starry-smoltcp/src/phy/fault_injector.rs`
- `vendor/starry-smoltcp/src/phy/fuzz_injector.rs`
- `vendor/starry-smoltcp/src/phy/pcap_writer.rs`
- `vendor/starry-smoltcp/src/phy/tracer.rs`

修改内容：

- 为新增的 `TxToken::set_dst_addr()` 做 passthrough。

效果：

- 保持 smoltcp 其它包装设备还能编译和转发目的地址信息。
- 这是 loopback destination-aware MTU/route token 的配套修改。

## P2：评分抽取和结果沉淀

涉及代码：

- `docs/benchmarks/iperf/get_result.py`
- `.gitignore`

修改内容：

- 新增 iperf 结果抽取脚本。
- 默认读取 `../../output_rv.txt` 和 `../../output_la.txt`。
- 按 `#### OS COMP TEST GROUP START iperf-musl/glibc ####` 分组。
- 对每个 `====== iperf XXX begin ======` 测试段，取最后一条 `receiver` 行的 bitrate。
- `Gbits/sec` 按评测脚本换算为 `1024 Mbits/sec`。
- 生成 `docs/benchmarks/iperf/results/MMDD-HHMMSS.md`。
- `.gitignore` 增加 `__pycache__/` 和 `*.py[cod]`，避免 Python 临时文件进入版本库。

效果：

- 可以持续保存每次优化后的结果，而不是只看终端日志。
- 便于和 Ubuntu 参考结果、不同优化阶段结果做同口径对比。

## P2：vendor checksum 同步

涉及代码：

- `vendor/axnet/.cargo-checksum.json`
- `vendor/starry-smoltcp/.cargo-checksum.json`

修改内容：

- 修改 vendored crate 源码后同步 checksum。

效果：

- 避免 Cargo 在 vendor 校验阶段因为 checksum 不一致失败。

## 当前优化点按优先级总结

| 优先级 | 优化点 | 主要文件 | 主要效果 |
|---|---|---|---|
| P0 | IPv4/IPv6 family、IPv4-mapped IPv6、`IPV6_V6ONLY` | `kernel/src/syscall/net/*`、`vendor/axnet/src/family.rs`、`tcp.rs`、`udp.rs` | 从 panic/错误路由变成可正常跑 iperf |
| P0 | `TCP_INFO`、socket buffer option | `kernel/src/syscall/net/opt.rs`、`vendor/axnet/src/options.rs`、`general.rs` | 补齐 iperf 依赖的 socket 语义 |
| P0 | route error 返回 errno | `vendor/axnet/src/service.rs` | 避免 `no route` 直接 panic |
| P0 | 清理 iperf daemon | `src/init.sh` | 避免 musl/glibc 之间端口和后台进程串扰 |
| P1 | 减少重复 `poll_interfaces()` | `kernel/src/syscall/io_mpx/*`、`vendor/axnet/src/general.rs` | 降低 send/recv/poll/select/epoll 固定开销 |
| P1 | 扩大 TCP/UDP/router buffer | `vendor/axnet/src/consts.rs`、`tcp.rs`、`udp.rs`、`listen_table.rs` | 解除 64 KiB 小 buffer 带来的吞吐上限 |
| P1 | loopback router 直投 | `vendor/axnet/src/router.rs`、`device/*` | 减少 loopback 包多次排队和拷贝 |
| P1 | loopback 大 MTU/MSS | `vendor/starry-smoltcp/src/phy/mod.rs`、`iface/interface/mod.rs`、`socket/tcp.rs` | 减少 TCP 分段，提高 Gbits/sec 吞吐 |
| P1 | TCP loopback 绕过 cwnd | `vendor/starry-smoltcp/src/socket/tcp.rs` | 本机连接不再被普通链路拥塞窗口限制 |
| P1 | UDP loopback 直达 socket RX queue | `vendor/axnet/src/udp.rs`、`vendor/starry-smoltcp/src/socket/udp.rs` | UDP 从约 0.6-0.7 Gbits/sec 提升到约 1.6-2.27 Gbits/sec |
| P2 | loopback checksum skip | `vendor/starry-smoltcp/src/iface/*`、`phy/mod.rs` | 降低本机 TCP/UDP 包 checksum 成本 |
| P2 | 结果抽取脚本 | `docs/benchmarks/iperf/get_result.py` | 固化评分口径，保存每轮结果 |

## 当前剩余差距

Ubuntu RV 参考结果总分是 glibc 10.740733、musl 10.604013；当前 StarryOS RV 是 glibc 10.114469、musl 10.068430。当前已经超过 baseline，但还没有完全追平 Ubuntu。

主要差距：

- TCP：Ubuntu glibc BASIC/PARALLEL/REVERSE TCP 为 3041.28 / 3573.76 / 3276.80 Mbits/sec，当前是 2293.76 / 2775.04 / 2088.96 Mbits/sec。
- UDP：当前 UDP 已明显提升，但 Ubuntu 的 BASIC/PARALLEL/REVERSE UDP 仍可到 2.7-3.8 Gbits/sec。
- 当前 UDP 直投仍需要扫描 socket set；连接数更多时，socket 查找和全局锁会成为新瓶颈。
- 当前 TCP 仍通过 smoltcp socket buffer 和协议状态机搬运数据，没有做到 Linux loopback 那种更深的批量化和调度优化。

后续如果继续优化，优先方向应该是：

- 给 UDP loopback 建立按 `(local_addr, local_port, peer)` 的快速查找表，减少扫描 socket set。
- 对 TCP/UDP loopback 做批量收发，降低每个 datagram/segment 的锁和 wake 成本。
- 拆分全局 socket set 锁，减少并发 UDP/PARALLEL_TCP 下的锁竞争。
- 继续减少 `poll_interfaces()` 的调用粒度，避免 send 后立即全局 poll 造成额外工作。
