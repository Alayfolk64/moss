// 本测例验证设置 IPV6_ADDRFORM 后，connect(AF_UNSPEC) 断开并重新 bind/listen 的 accepted IPv6 socket 能正确转换成 IPv4 listen socket，覆盖 CVE-2018-9568 相关路径。

#ifdef __linux__

#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

// 原测例循环次数。
#define LOOP_COUNT 1000

// 监听 IPv6 socket。
static int listenfd = -1;

// 被 accept 后转换地址族的 socket。
static int fd = -1;

// 连接到 IPv6 listener 的客户端 socket。
static int confd1 = -1;

// 连接到转换后 IPv4 listener 的客户端 socket。
static int confd2 = -1;

// 转换后 listener accept() 返回的 socket。
static int confd3 = -1;

// IPv6 listener 绑定地址。
static struct sockaddr_in6 bind_addr;

// 转换后 IPv4 listener 绑定地址。
static struct sockaddr_in bind_addr4;

// IPv4 客户端连接地址。
static struct sockaddr_in client_addr;

// AF_UNSPEC 地址，用来断开已连接 socket。
static struct sockaddr reset_addr;

// 测试开始时的内核 taint 值。
static long initial_taint = -1;

// 读取 Linux taint 状态，失败时返回 -1。
static long read_kernel_taint(void)
{
    // 打开 /proc/sys/kernel/tainted，模拟 LTP 的 taint_check 能力。
    FILE *fp = fopen("/proc/sys/kernel/tainted", "r");
    if (!fp)
        return -1;

    // 读取十进制 taint 值。
    long value = -1;
    if (fscanf(fp, "%ld", &value) != 1)
        value = -1;
    fclose(fp);
    return value;
}

// 关闭一个 fd 并重置为 -1。
static void close_fd(int *target)
{
    // 避免重复关闭无效 fd。
    if (*target >= 0) {
        close(*target);
        *target = -1;
    }
}

// 清理当前迭代打开的 socket。
static void cleanup_iteration(void)
{
    // 按连接依赖顺序关闭 fd。
    close_fd(&confd3);
    close_fd(&confd2);
    close_fd(&confd1);
    close_fd(&fd);
}

// 清理全局监听 socket 和当前迭代资源。
static void cleanup(void)
{
    // 先清理迭代 fd，再关闭 listener。
    cleanup_iteration();
    close_fd(&listenfd);
}

// 初始化 IPv4 地址。
static void init_ipv4(struct sockaddr_in *addr, in_addr_t ip, in_port_t port)
{
    // 清零并填充地址族、地址和端口。
    memset(addr, 0, sizeof(*addr));
    addr->sin_family = AF_INET;
    addr->sin_addr.s_addr = ip;
    addr->sin_port = port;
}

// 初始化 IPv6 地址。
static void init_ipv6(struct sockaddr_in6 *addr, const struct in6_addr *ip,
                      in_port_t port)
{
    // 清零并填充地址族、地址和端口。
    memset(addr, 0, sizeof(*addr));
    addr->sin6_family = AF_INET6;
    addr->sin6_addr = *ip;
    addr->sin6_port = port;
}

// 读取 socket 的本地地址，并转成客户端可连接的 IPv4 地址。
static socklen_t get_connect_address(int sock,
                                     struct sockaddr_storage *storage)
{
    // 读取 bind/listen 后的实际 IPv4 地址和端口。
    socklen_t len = sizeof(*storage);
    memset(storage, 0, sizeof(*storage));
    if (getsockname(sock, (struct sockaddr *)storage, &len) != 0) {
        fprintf(stderr, "getsockname() failed: %s\n", strerror(errno));
        return 0;
    }

    // INADDR_ANY 不能作为远端地址，改成 127.0.0.1。
    if (storage->ss_family == AF_INET) {
        struct sockaddr_in *addr = (struct sockaddr_in *)storage;
        if (addr->sin_addr.s_addr == htonl(INADDR_ANY))
            addr->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    }

    return len;
}

// 准备 IPv6 listener 和固定地址结构。
static int setup(void)
{
    // 初始化各类地址。
    init_ipv6(&bind_addr, &in6addr_any, 0);
    init_ipv4(&bind_addr4, htonl(INADDR_ANY), 0);
    memset(&reset_addr, 0, sizeof(reset_addr));
    reset_addr.sa_family = AF_UNSPEC;

    // 创建 IPv6 TCP listener。
    listenfd = socket(AF_INET6, SOCK_STREAM, IPPROTO_TCP);
    if (listenfd < 0) {
        fprintf(stderr, "socket(AF_INET6) failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 确保 IPv6 listener 接受 IPv4-mapped 连接。
    int off = 0;
    setsockopt(listenfd, IPPROTO_IPV6, IPV6_V6ONLY, &off, sizeof(off));

    // 绑定并监听随机端口。
    if (bind(listenfd, (struct sockaddr *)&bind_addr, sizeof(bind_addr)) != 0) {
        fprintf(stderr, "bind() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    if (listen(listenfd, 5) != 0) {
        fprintf(stderr, "listen() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 读取 listener 的实际端口，用于 IPv4 客户端连接。
    socklen_t size = sizeof(bind_addr);
    if (getsockname(listenfd, (struct sockaddr *)&bind_addr, &size) != 0) {
        fprintf(stderr, "getsockname() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    init_ipv4(&client_addr, htonl(INADDR_LOOPBACK), bind_addr.sin6_port);
    initial_taint = read_kernel_taint();
    return 0;
}

// 执行一次 IPV6_ADDRFORM 转换和重新 listen 检查。
static int run_iteration(void)
{
    // 先用 IPv4 客户端连接 IPv6 listener。
    confd1 = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (confd1 < 0) {
        fprintf(stderr, "socket(AF_INET) failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    if (connect(confd1, (struct sockaddr *)&client_addr,
                sizeof(client_addr)) != 0) {
        fprintf(stderr, "connect() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // accept() 得到要转换地址族的 socket。
    fd = accept(listenfd, NULL, NULL);
    if (fd < 0) {
        fprintf(stderr, "accept() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 把 accepted IPv6 socket 转换成 IPv4 socket。
    int optval = AF_INET;
    errno = 0;
    int ret = setsockopt(fd, SOL_IPV6, IPV6_ADDRFORM, &optval,
                         sizeof(optval));
    int saved_errno = errno;
    if (ret == -1) {
        fprintf(stderr, "setsockopt(IPV6_ADDRFORM) failed: %s\n",
                strerror(saved_errno));
        return EXIT_FAILURE;
    }
    if (ret != 0) {
        fprintf(stderr, "setsockopt(IPV6_ADDRFORM) returned invalid value: %s\n",
                strerror(saved_errno));
        return EXIT_FAILURE;
    }

    // 用 AF_UNSPEC disconnect，再 bind/listen 成新的 IPv4 listener。
    if (connect(fd, &reset_addr, sizeof(reset_addr)) != 0) {
        fprintf(stderr, "connect(AF_UNSPEC) failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    if (bind(fd, (struct sockaddr *)&bind_addr4, sizeof(bind_addr4)) != 0) {
        fprintf(stderr, "bind() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    if (listen(fd, 5) != 0) {
        fprintf(stderr, "listen() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 连接转换后的 IPv4 listener。
    struct sockaddr_storage client_addr2;
    socklen_t addrlen = get_connect_address(fd, &client_addr2);
    if (addrlen == 0)
        return EXIT_FAILURE;
    confd2 = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (confd2 < 0) {
        fprintf(stderr, "socket(AF_INET) failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    if (connect(confd2, (struct sockaddr *)&client_addr2, addrlen) != 0) {
        fprintf(stderr, "connect() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    confd3 = accept(fd, NULL, NULL);
    if (confd3 < 0) {
        fprintf(stderr, "accept() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    return 0;
}

// 主测试循环。
static int run_test(void)
{
    // 重复原测例的 1000 次转换检查。
    for (int i = 0; i < LOOP_COUNT; i++) {
        int ret = run_iteration();
        cleanup_iteration();
        if (ret != 0)
            return ret;

        // 如果运行过程中新增 taint，按原测例报告 vulnerable。
        long taint = read_kernel_taint();
        if (initial_taint >= 0 && taint >= 0 && taint != initial_taint) {
            fprintf(stderr, "Kernel is vulnerable\n");
            return EXIT_FAILURE;
        }
    }

    // 保留原测例最终成功输出。
    printf("Nothing bad happened, probably\n");
    return 0;
}

// 入口函数准备 listener，执行循环并清理。
int main(void)
{
    // 准备全局 listener。
    int ret = setup();
    if (ret == 0)
        ret = run_test();

    // 清理资源。
    cleanup();
    return ret;
}

#else

#include <stdio.h>

// 用 2 表示环境不支持该 Linux CVE 测例。
#define EXIT_SKIP 2

// 非 Linux 系统没有 IPV6_ADDRFORM 语义，保留可编译入口。
int main(void)
{
    // connect02 是 Linux IPV6_ADDRFORM 专属测例。
    fprintf(stderr, "SKIP: connect02 requires Linux IPV6_ADDRFORM support\n");
    return EXIT_SKIP;
}

#endif
