// 本公共头为 bind 简化测例提供 socket 地址初始化、临时目录、完整读写和超时设置等小工具，避免重新引入 LTP 框架。

#ifndef BIND_SIMPLE_COMMON_H
#define BIND_SIMPLE_COMMON_H

// 启用 GNU/POSIX 扩展声明，保证 Linux socket 相关接口尽量可见。
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netinet/ip.h>
#include <pthread.h>
#include <pwd.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/un.h>
#include <unistd.h>

// 用 2 表示环境不满足或内核能力缺失。
#define EXIT_SKIP 2

// pathname UNIX socket 使用的主 socket 文件名。
#define MAIN_SOCKET_FILE "test.sock"

// Linux abstract UNIX socket 使用的地址，首字节必须是 NUL。
#define ABSTRACT_SOCKET_PATH "\0test.sock"

// datagram UNIX socket peer 使用的 socket 文件名。
#define PEER_SOCKET_FILE "peer.sock"

// IPv4 loopback 地址字符串。
#define IPV4_ADDRESS "127.0.0.1"

// bind04/bind05 通信缓冲区大小。
#define BUFFER_SIZE 128

// 如果系统头缺少 SCTP 协议号，补上 Linux 常用值以便编译；运行时 socket() 会判断是否支持。
#ifndef IPPROTO_SCTP
#define IPPROTO_SCTP 132
#endif

// 如果系统头缺少 UDP-Lite 协议号，补上 Linux 常用值以便编译；运行时 socket() 会判断是否支持。
#ifndef IPPROTO_UDPLITE
#define IPPROTO_UDPLITE 136
#endif

// 如果系统头缺少 SOCK_SEQPACKET，补上常用值以便编译；运行时 socket() 会判断是否支持。
#ifndef SOCK_SEQPACKET
#define SOCK_SEQPACKET 5
#endif

// 统计数组元素个数。
#define SIMPLE_ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))

// bind04/bind05 共用的协议场景描述。
struct bind_test_case {
    int type;
    int protocol;
    struct sockaddr *address;
    socklen_t addrlen;
    const char *description;
};

// 打印带 errno 的错误并终止当前测例。
static inline void simple_die_errno(const char *message)
{
    // 保留调用点提供的英文错误上下文，追加 errno 文本帮助定位环境问题。
    fprintf(stderr, "%s: %s\n", message, strerror(errno));
    exit(EXIT_FAILURE);
}

// 持续写入直到缓冲区全部发送完毕。
static inline int simple_write_full(int fd, const void *buf, size_t len)
{
    // 用字节指针推进，处理 socket 短写。
    const char *p = (const char *)buf;
    while (len > 0) {
        ssize_t ret = write(fd, p, len);

        // EINTR 只表示被信号打断，重试即可。
        if (ret < 0 && errno == EINTR)
            continue;

        // 其他错误交给调用者报告。
        if (ret < 0)
            return -1;

        // write 返回 0 表示无法继续推进，按 I/O 错误处理。
        if (ret == 0) {
            errno = EIO;
            return -1;
        }

        // 更新剩余数据范围。
        p += ret;
        len -= (size_t)ret;
    }

    return 0;
}

// 持续读取直到缓冲区填满。
static inline int simple_read_full(int fd, void *buf, size_t len)
{
    // 用字节指针推进，处理 socket 短读。
    char *p = (char *)buf;
    while (len > 0) {
        ssize_t ret = read(fd, p, len);

        // EINTR 只表示被信号打断，重试即可。
        if (ret < 0 && errno == EINTR)
            continue;

        // 其他错误交给调用者报告。
        if (ret < 0)
            return -1;

        // 对端提前关闭时认为通信失败。
        if (ret == 0) {
            errno = ECONNRESET;
            return -1;
        }

        // 更新剩余读取范围。
        p += ret;
        len -= (size_t)ret;
    }

    return 0;
}

// 创建并进入临时目录，模拟 LTP 的 needs_tmpdir 行为。
static inline void simple_enter_temp_dir(char *template)
{
    // mkdtemp 会把模板转换成唯一目录名。
    if (mkdtemp(template) == NULL)
        simple_die_errno("mkdtemp failed");

    // 后续相对路径 socket 文件都落在该临时目录中。
    if (chdir(template) != 0)
        simple_die_errno("chdir failed");
}

// 离开并删除临时目录。
static inline void simple_leave_temp_dir(const char *path)
{
    // 先切到根目录，避免当前工作目录阻止删除。
    chdir("/");

    // 删除失败只作为清理警告，不覆盖主要测试结果。
    if (path && rmdir(path) != 0)
        fprintf(stderr, "rmdir %s: %s\n", path, strerror(errno));
}

// 初始化 IPv4 sockaddr，ip 为 NULL 时使用 INADDR_ANY。
static inline void init_ipv4(struct sockaddr_in *addr, const char *ip,
                             in_port_t port)
{
    // 清零结构体，避免未初始化填充字节干扰内核解析。
    memset(addr, 0, sizeof(*addr));
    addr->sin_family = AF_INET;
    addr->sin_port = port;

    // 根据传入字符串选择 loopback 或 wildcard 地址。
    if (ip) {
        if (inet_pton(AF_INET, ip, &addr->sin_addr) != 1)
            simple_die_errno("inet_pton IPv4 failed");
    } else {
        addr->sin_addr.s_addr = htonl(INADDR_ANY);
    }
}

// 初始化 IPv6 sockaddr。
static inline void init_ipv6(struct sockaddr_in6 *addr,
                             const struct in6_addr *ip, in_port_t port)
{
    // 清零结构体，避免未初始化字段影响 bind/connect。
    memset(addr, 0, sizeof(*addr));
    addr->sin6_family = AF_INET6;
    addr->sin6_port = port;
    addr->sin6_addr = *ip;
}

// 为 socket 设置收发超时，防止 peer 线程异常时主线程永久阻塞。
static inline void simple_set_socket_timeout(int fd)
{
    // 使用较短超时即可发现本地通信问题，同时避免测例卡死。
    struct timeval tv;
    tv.tv_sec = 5;
    tv.tv_usec = 0;

    // 超时设置失败不直接终止，因为有些协议可能不支持其中一个方向。
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
}

// 判断地址是否是 Linux abstract UNIX socket。
static inline int simple_is_abstract_unix(const struct sockaddr *addr)
{
    // 只有 AF_UNIX 且 sun_path 首字节为 NUL 才是 abstract 地址。
    if (!addr || addr->sa_family != AF_UNIX)
        return 0;
    return ((const struct sockaddr_un *)addr)->sun_path[0] == '\0';
}

// 判断当前编译目标是否支持 abstract UNIX socket。
static inline int simple_abstract_unix_supported(void)
{
    // abstract UNIX socket 是 Linux 特性，其他系统上跳过对应场景。
#ifdef __linux__
    return 1;
#else
    return 0;
#endif
}

// 把 bind 后的地址转换成客户端可连接地址。
static inline socklen_t get_connect_address(int sock,
                                            struct sockaddr_storage *storage)
{
    // 先读取内核分配后的真实地址和端口。
    socklen_t len = sizeof(*storage);
    memset(storage, 0, sizeof(*storage));
    if (getsockname(sock, (struct sockaddr *)storage, &len) != 0)
        simple_die_errno("getsockname failed");

    // wildcard IPv4 地址不能直接作为远端地址，改成 loopback。
    if (storage->ss_family == AF_INET) {
        struct sockaddr_in *addr = (struct sockaddr_in *)storage;
        if (addr->sin_addr.s_addr == htonl(INADDR_ANY))
            addr->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    }

    // wildcard IPv6 地址不能直接作为远端地址，改成 ::1。
    if (storage->ss_family == AF_INET6) {
        struct sockaddr_in6 *addr6 = (struct sockaddr_in6 *)storage;
        if (IN6_IS_ADDR_UNSPECIFIED(&addr6->sin6_addr))
            addr6->sin6_addr = in6addr_loopback;
    }

    return len;
}

#endif
