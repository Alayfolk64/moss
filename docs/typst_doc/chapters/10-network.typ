#import "../template.typ": evidence

= 网络子系统

== Feature 控制

网络实现受 `starry-kernel/net` feature 控制。顶层 `qemu` feature 包含 `starry-kernel/net`，因此 QEMU 默认构建路径会启用网络。未启用 `net` 时，`kernel/src/syscall/net/mod.rs` 为 socket 族系统调用提供 stub，统一返回 `ENOSYS`。

启用 `net` 时，`syscall/net` re-export `socket`、`io`、`name`、`opt`、`cmsg` 等子模块，并使用 `axnet` 的 socket 实现。

== Socket 对象

`kernel/src/file/net.rs::Socket` 包装 `axnet::Socket` 并实现 `FileLike`：

- `read` 调用 `recv`，`write` 调用 `send`。
- `stat` 返回 socket 类型的 `Kstat`。
- nonblocking 状态通过 axnet socket option 读取和设置。
- `poll`/`register` 直接代理到底层 socket。
- `path` 输出 `socket:[addr]` 风格字符串。

因此网络 fd 与普通文件、管道、eventfd 等对象共享相同的 fd 表和 poll/epoll 路径。

== 支持的 socket 类型

`sys_socket` 当前可确认支持：

#table(
  columns: (2fr, 2fr, 6fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*domain*], [*type*], [*后端*],
  [`AF_INET` / `AF_INET6`], [`SOCK_STREAM`], [`axnet::tcp::TcpSocket`，协议允许 0、TCP、SCTP。],
  [`AF_INET` / `AF_INET6`], [`SOCK_DGRAM`], [`axnet::udp::UdpSocket`，协议允许 0、UDP、UDPLITE。],
  [`AF_UNIX`], [`SOCK_STREAM` / `SOCK_SEQPACKET`], [`UnixSocket` + `StreamTransport`。],
  [`AF_UNIX`], [`SOCK_DGRAM`], [`UnixSocket` + `DgramTransport`。],
  [`AF_VSOCK`], [`SOCK_STREAM`], [仅在 `vsock` cfg 下编译。],
)

`socketpair` 只接受 `AF_UNIX`，支持 stream、dgram 和 seqpacket。`bind` 对特权端口做 euid 检查：非 root 绑定 1 到 1023 端口返回 `EACCES`。

== 网络 I/O 与选项

`sendto/sendmsg` 使用 `VmBytes` 或 `IoVectorBuf` 从用户空间读取数据，支持目的地址和 control message；`recvfrom/recvmsg` 支持 `MSG_PEEK`、`MSG_TRUNC`、返回源地址以及 `SCM_RIGHTS` 文件描述符传递。`getsockopt/setsockopt` 覆盖 `SO_REUSEADDR`、`SO_ERROR`、`SO_SNDBUF`、`SO_RCVBUF`、`SO_KEEPALIVE`、`SO_RCVTIMEO`、`SO_SNDTIMEO`、`SO_PASSCRED`、`SO_PEERCRED`、`TCP_NODELAY`、`TCP_MAXSEG`、`TCP_INFO`、`IP_TTL` 等选项。

`setsockopt(SO_ATTACH_BPF)` 会从 fd 表取出 `BpfProgram` 并执行其有限测试逻辑。这是 socket option 与 BPF 测试对象的连接点。

== 代码依据

#evidence((
  [`网络开关`], [`kernel/src/syscall/net/mod.rs`], [`net` 关闭时 socket 相关 syscall 返回 `ENOSYS`，开启时导入 socket/io/name/opt/cmsg。],
  [`socket 创建`], [`kernel/src/syscall/net/socket.rs`], [`sys_socket` 根据 domain/type 选择 TcpSocket、UdpSocket 或 UnixSocket。],
  [`socket fd`], [`kernel/src/file/net.rs`], [`Socket` 实现 `FileLike` 和 `Pollable`，代理 axnet send/recv/poll。],
  [`网络 I/O`], [`kernel/src/syscall/net/io.rs`], [`sendmsg/recvmsg` 支持 iovec、地址和 control message。],
  [`socket options`], [`kernel/src/syscall/net/opt.rs`], [`getsockopt/setsockopt` 分发多种 SOL_SOCKET/TCP/IP 选项，并处理 `SO_ATTACH_BPF`。],
))
