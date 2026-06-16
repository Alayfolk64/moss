// 本公共头为 bind 简化测例提供 socket 地址初始化、完整读写、临时目录和通用清理工具。

#ifndef BIND_SIMPLE_COMMON_H
#define BIND_SIMPLE_COMMON_H

// 启用 GNU/POSIX 扩展声明，保证 socket 相关扩展常量尽量可见。
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <netinet/in.h>
#include <netinet/ip.h>
#include <pthread.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/un.h>
#include <unistd.h>

// 旧系统头可能没有 SCTP 协议号，这里补 Linux/IANA 常用值。
#ifndef IPPROTO_SCTP
#define IPPROTO_SCTP 132
#endif

// 旧系统头可能没有 UDP-Lite 协议号，这里补 Linux/IANA 常用值。
#ifndef IPPROTO_UDPLITE
#define IPPROTO_UDPLITE 136
#endif

// bind 通信测例的 UNIX pathname socket 名称。
#define MAIN_SOCKET_FILE "test.sock"

// bind 通信测例的 UNIX abstract socket 名称。
#define ABSTRACT_SOCKET_PATH "\0test.sock"

// bind datagram 测例的 peer socket 名称。
#define PEER_SOCKET_FILE "peer.sock"

// IPv4 loopback 地址。
#define IPV4_ADDRESS "127.0.0.1"

// 通信缓冲区大小。
#define BUFFER_SIZE 128

// 用 2 表示当前环境不满足测例前提。
#define EXIT_SKIP 2

// 单个 bind 通信用例，来自原 libbind.h。
struct bind_test_case {
    int type;
    int protocol;
    struct sockaddr *address;
    socklen_t addrlen;
    const char *description;
};

// 创建并进入临时目录。
static inline void simple_enter_temp_dir(char *template)
{
    // mkdtemp 会生成唯一目录。
    if (mkdtemp(template) == NULL) {
        fprintf(stderr, "mkdtemp failed: %s\n", strerror(errno));
        exit(EXIT_FAILURE);
    }

    // 进入临时目录，让 UNIX pathname socket 文件不会污染源码目录。
    if (chdir(template) != 0) {
        fprintf(stderr, "chdir failed: %s\n", strerror(errno));
        exit(EXIT_FAILURE);
    }
}

// 离开并删除临时目录。
static inline void simple_leave_temp_dir(const char *path)
{
    // 先离开临时目录，再删除目录本身。
    chdir("/");
    if (path)
        rmdir(path);
}

// 持续写入直到缓冲区全部写完。
static inline int simple_write_full(int fd, const void *buf, size_t len)
{
    // 逐段推进写指针，处理短写。
    const char *p = (const char *)buf;
    while (len > 0) {
        ssize_t ret = write(fd, p, len);
        if (ret < 0)
            return -1;
        if (ret == 0) {
            errno = EIO;
            return -1;
        }
        p += ret;
        len -= (size_t)ret;
    }
    return 0;
}

// 持续读取直到缓冲区填满。
static inline int simple_read_full(int fd, void *buf, size_t len)
{
    // 逐段推进读指针，处理短读。
    char *p = (char *)buf;
    while (len > 0) {
        ssize_t ret = read(fd, p, len);
        if (ret < 0)
            return -1;
        if (ret == 0) {
            errno = ECONNRESET;
            return -1;
        }
        p += ret;
        len -= (size_t)ret;
    }
    return 0;
}

// 初始化 IPv4 sockaddr_in。
static inline void init_ipv4(struct sockaddr_in *addr, const char *ip, uint16_t port)
{
    // 清零后填充 family、port 和地址。
    memset(addr, 0, sizeof(*addr));
    addr->sin_family = AF_INET;
    addr->sin_port = htons(port);
    if (!ip)
        addr->sin_addr.s_addr = htonl(INADDR_ANY);
    else
        inet_pton(AF_INET, ip, &addr->sin_addr);
}

// 初始化 IPv6 sockaddr_in6。
static inline void init_ipv6(struct sockaddr_in6 *addr, const struct in6_addr *ip, uint16_t port)
{
    // 清零后填充 family、port 和地址。
    memset(addr, 0, sizeof(*addr));
    addr->sin6_family = AF_INET6;
    addr->sin6_port = htons(port);
    addr->sin6_addr = *ip;
}

// 把监听 socket 的实际地址转换成可连接地址。
static inline socklen_t get_connect_address(int sock, struct sockaddr_storage *out)
{
    // getsockname 读取 bind 后内核分配的地址和端口。
    socklen_t len = sizeof(*out);
    if (getsockname(sock, (struct sockaddr *)out, &len) != 0) {
        fprintf(stderr, "getsockname failed: %s\n", strerror(errno));
        exit(EXIT_FAILURE);
    }

    // wildcard IPv4 地址需要改成 loopback，方便本机连接。
    if (out->ss_family == AF_INET) {
        struct sockaddr_in *in = (struct sockaddr_in *)out;
        if (in->sin_addr.s_addr == htonl(INADDR_ANY))
            inet_pton(AF_INET, IPV4_ADDRESS, &in->sin_addr);
    }

    // wildcard IPv6 地址需要改成 loopback，方便本机连接。
    if (out->ss_family == AF_INET6) {
        struct sockaddr_in6 *in6 = (struct sockaddr_in6 *)out;
        if (!memcmp(&in6->sin6_addr, &in6addr_any, sizeof(in6addr_any)))
            in6->sin6_addr = in6addr_loopback;
    }

    return len;
}

// 统计数组元素个数。
#define SIMPLE_ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))

#endif
