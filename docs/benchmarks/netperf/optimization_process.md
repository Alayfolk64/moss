# netperf 网络栈优化记录

这份文档记录本轮 netperf 工作：先补齐评分抽取脚本和 Ubuntu 参考结果，再处理当前 StarryOS 上 netperf 暴露出来的 socket 兼容性问题。当前修改属于通用 socket 语义补齐，不是针对 netperf 名称做特殊分支。

## 结果演进

| 阶段 | 结果文档 | glibc 总分 | musl 总分 | 关键现象 |
|---|---|---:|---:|---|
| 起点 | `docs/benchmarks/netperf/results/0613-005714.md` | 5.827964 | 5.833977 | TCP_STREAM 只有约 11 Mbits/sec，日志里还有 `SO_DONTROUTE`、`IP_RECVERR`、`getprotobyname` 相关异常路径 |
| 补齐 socket 语义后 | `docs/benchmarks/netperf/results/0613-012311.md` | 5.848334 | 5.833616 | TCP_STREAM 提升到约 21-22 Mbits/sec，异常提示消失，socket buffer 显示为 32000 |
| 继续对齐 Linux option | `docs/benchmarks/netperf/results/0613-022124.md` | 5.840921 | 5.843688 | 增加 `SO_*BUFFORCE`、`SO_LINGER`、`TCP_QUICKACK`、`TCP_MAXSEG set`；当前 netperf 参数未明显触发这些路径，分数基本不变 |
| TCP send poll 批处理 | `docs/benchmarks/netperf/results/0613-103134.md` | 5.838587 | 5.848585 | TCP_STREAM 提升到约 50-52 Mbits/sec，说明每次 send 后立即全局 poll 是明显固定成本 |
| loopback ACK/Nagle/短连接推进 | `docs/benchmarks/netperf/results/0613-105133.md` | 5.843723 | 5.851062 | TCP_CRR 提升到约 157-160，TCP_STREAM 小幅提升；RR 波动不大 |
| 锁提升实验（已回退） | `docs/benchmarks/netperf/results/0613-110021.md` | 5.856434 | 5.851311 | UDP_STREAM 有涨，但 UDP_RR/TCP_RR 回落，收益不稳定且扩大锁持有范围，未保留 |
| Ubuntu 参考 | `docs/benchmarks/netperf/results/Ubuntu.md` | 7.447960 | 8.089466 | TCP_STREAM、UDP_RR、TCP_RR、TCP_CRR 明显高于当前 StarryOS，是后续优化目标 |

## P0：评分抽取和参考报告

涉及代码：

- `docs/benchmarks/netperf/get_result.py`
- `docs/benchmarks/netperf/results/Ubuntu.md`
- `docs/benchmarks/netperf/results/0613-*.md`

修改内容：

- 按评测脚本的 netperf 解析逻辑提取结果。
- 识别 `#### OS COMP TEST GROUP START netperf-musl/glibc ####` 分组。
- 识别 `UDP_STREAM`、`TCP_STREAM`、`UDP_RR`、`TCP_RR`、`TCP_CRR`。
- 对测试段内第一条 5/6 列数字行取最后一列作为 result。
- 保存带时间戳的 Markdown 报告。

效果：

- 可以从 `output_rv.txt`、`output_la.txt` 直接生成本地 netperf 评分报告。
- 可以用 Ubuntu netperf 日志生成同一口径的参考报告。

## P0：补齐 netperf 运行所需的 libc 数据

涉及代码：

- `src/init.sh`

修改内容：

- 初始化 `/etc/protocols`，加入 `ip`、`icmp`、`tcp`、`udp`。

效果：

- glibc netperf 调用 `getprotobyname("ip")` 时不再失败。
- 这是通用 libc 运行时文件，其他依赖 protocol database 的程序也能受益。

## P0：补齐通用 socket option

涉及代码：

- `kernel/src/syscall/net/opt.rs`
- `vendor/axnet/src/options.rs`
- `vendor/axnet/src/general.rs`
- `vendor/axnet/.cargo-checksum.json`

修改内容：

- 增加 `IP_RECVERR`、`IPV6_RECVERR` 到 socket option 分发。
- 在 axnet 中保存 `ReceiveError` 状态，当前作为 no-op 兼容项处理。
- 保存 `SO_DONTROUTE` 状态，避免用户态设置该选项时返回 `ENOPROTOOPT`。
- `SO_SNDBUF`、`SO_RCVBUF` 按 Linux 语义做内部翻倍，并受 `MAX_SOCKET_BUF_LEN` 限制。

效果：

- netperf 日志里 `enable_enobufs`、`so_dontroute`、`errno 92` 这类异常提示消失。
- netperf 使用 `-s 16k -S 16k` 后，用户态看到的 send/recv buffer 从 16000 变为 32000，更接近 Linux 行为。
- TCP_STREAM 从约 11 Mbits/sec 提升到约 21-22 Mbits/sec。

## P1：继续补齐 Linux socket option 语义

涉及代码：

- `kernel/src/syscall/net/opt.rs`
- `vendor/axnet/src/options.rs`
- `vendor/axnet/src/general.rs`
- `vendor/axnet/src/tcp.rs`
- `vendor/axnet/src/udp.rs`
- `vendor/axnet/.cargo-checksum.json`

修改内容：

- 增加 `SO_SNDBUFFORCE`、`SO_RCVBUFFORCE` 映射，当前和普通 buffer option 使用同一容量限制。
- 增加 `SO_LINGER` get/set 状态；`linger=0` 的 TCP 关闭走 `abort()`，更接近 Linux 的 RST 语义。
- 增加 `TCP_QUICKACK`，映射到 smoltcp delayed ACK 开关。
- 增加 `TCP_MAXSEG` set 的最小支持，保存用户指定 MSS 并影响 `getsockopt(TCP_MAXSEG)` / `TCP_INFO` 展示值。

效果：

- RV netperf 编译运行通过，glibc/musl 5 项均有输出。
- 当前测试命令没有显式设置 `TCP_QUICKACK`、`TCP_MAXSEG` 或 `SO_LINGER`，所以分数基本不变。
- 这一步主要减少 Linux 兼容性缺口，避免后续网络测例进入 `ENOPROTOOPT` 或降级路径。

## P1：TCP send 后批量 poll

涉及代码：

- `vendor/axnet/src/consts.rs`
- `vendor/axnet/src/tcp.rs`
- `vendor/axnet/.cargo-checksum.json`

修改内容：

- 增加 `TCP_SEND_POLL_BYTES`。
- `TcpSocket::send()` 不再每次成功写入后都立刻 `poll_interfaces()`。
- 发送字节数累计到阈值后再 poll；如果 tx buffer 满导致 `WouldBlock`，仍然走原来的网络 poller 推进网络栈。

效果：

- TCP_STREAM 从约 20-22 Mbits/sec 提升到约 50-52 Mbits/sec。
- 这是通用 TCP 发送路径批处理，不依赖 netperf 名称或命令参数。

## P1：loopback TCP ACK/Nagle 和短连接推进

涉及代码：

- `vendor/starry-smoltcp/src/socket/tcp.rs`
- `vendor/starry-smoltcp/.cargo-checksum.json`
- `vendor/axnet/src/tcp.rs`
- `vendor/axnet/.cargo-checksum.json`

修改内容：

- loopback TCP tuple 上不再因为 delayed ACK timer 等待 ACK 发送。
- loopback TCP tuple 上绕过 Nagle 的“小包已有未确认数据时等待 ACK”限制，但不改变用户态 `TCP_NODELAY` 选项状态。
- `connect()` 建立后立即 `poll_interfaces()`，去掉固定 `yield_now()`。

效果：

- TCP_CRR 从上一版约 140-150 提升到约 157-160。
- TCP_STREAM 小幅提升到 glibc 54.42、musl 50.95。
- 这说明短连接固定调度成本有所下降，但 TCP_STREAM 的主瓶颈仍在 loopback TCP 数据路径和 smoltcp 包处理成本。

## P1：poll_interfaces 锁提升实验（已回退）

涉及代码：

- `vendor/axnet/src/lib.rs`
- `vendor/axnet/.cargo-checksum.json`

尝试内容：

- 将 `poll_interfaces()` 中每轮循环重复获取 `SERVICE` / `SOCKET_SET` 锁，改为一次持锁跑完整个 poll 循环。

结果：

- `docs/benchmarks/netperf/results/0613-110021.md` 中 UDP_STREAM 有提升，但 UDP_RR/TCP_RR 回落。
- 该改动扩大了全局锁持有范围，收益不稳定，已经回退。

## 后续优化优先级

| 优先级 | 方向 | 目标项目 | 原因 |
|---|---|---|---|
| P1 | TCP send/recv 快路径批处理 | TCP_STREAM | 当前 TCP_STREAM 仍远低于 baseline 和 Ubuntu，瓶颈已经不是缺 socket option，而是 TCP loopback 数据路径本身 |
| P1 | loopback 直投或减少排队/拷贝 | TCP_STREAM、UDP_STREAM | 本机 127.0.0.1 流量不应该付出普通网卡路径的完整队列成本 |
| P1 | request/response 场景减少固定 syscall/轮询成本 | UDP_RR、TCP_RR | RR 项每次包很小，固定调度和 poll 成本占比高 |
| P2 | connect/accept/close/TIME_WAIT 路径优化 | TCP_CRR | TCP_CRR 主要测短连接，连接生命周期成本比传输成本更重要 |
| P2 | 更完整的 Linux socket option 兼容 | 所有项目 | 避免用户态 benchmark 进入降级路径，但单独补 option 对吞吐提升有限 |

当前最值得继续做的是 TCP loopback 数据路径，而不是继续调 netperf 参数。`TCP_STREAM` 已经从最初约 11 Mbits/sec 提升到约 50-54 Mbits/sec，但仍低于 baseline 79.75 和 Ubuntu 参考值，说明后面需要继续减少 TCP 发送、ACK、poll 和 loopback 投递路径上的通用开销。
