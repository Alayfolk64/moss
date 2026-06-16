// 本测例验证 connect() 在坏 fd、无效 sockaddr 指针、无效地址长度、非 socket fd、已连接 socket、连接被拒绝以及无效地址族时的返回值和 errno；服务端子进程带超时和清理，避免原 LTP 无限 accept 循环卡住。

#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#ifdef __linux__
#include <sys/syscall.h>
#endif

// 统计数组元素个数，用于遍历原 connect01 的测试表。
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))

// 服务端最长存活秒数，用来替代原测例的无限循环。
#define SERVER_TIMEOUT_SEC 20

// 当前测试使用的 socket 或文件 fd。
static int current_fd = -1;

// 当前 fd 是否需要在场景结束后关闭。
static int current_fd_should_close;

// 服务端监听地址，也是成功连接和 EISCONN 场景的目标地址。
static struct sockaddr_in sin1;

// 未监听端口地址，用于触发 ECONNREFUSED。
static struct sockaddr_in sin2;

// 无效地址族地址，用于触发 EAFNOSUPPORT。
static struct sockaddr_in sin4;

// 服务端子进程 pid。
static pid_t server_pid = -1;

// 子进程收到 SIGTERM 后退出的标志。
static volatile sig_atomic_t server_stop;

// 每个场景需要的 fd 准备方式。
enum setup_kind {
    SETUP_BAD_FD,
    SETUP_SOCKET,
    SETUP_DEV_NULL,
    SETUP_CONNECTED,
};

// 单个 connect() 测试场景。
struct test_case {
    int domain;
    int type;
    int proto;
    struct sockaddr *sockaddr;
    socklen_t salen;
    int retval;
    int experrno;
    enum setup_kind setup;
    const char *desc;
};

// 原 connect01 覆盖的失败路径。
static struct test_case test_cases[] = {
    {PF_INET, SOCK_STREAM, 0, (struct sockaddr *)&sin1,
     sizeof(struct sockaddr_in), -1, EBADF, SETUP_BAD_FD,
     "bad file descriptor"},
    {PF_INET, SOCK_STREAM, 0, (struct sockaddr *)(uintptr_t)-1,
     sizeof(struct sockaddr_in), -1, EFAULT, SETUP_SOCKET,
     "invalid socket buffer"},
    {PF_INET, SOCK_STREAM, 0, (struct sockaddr *)&sin1,
     3, -1, EINVAL, SETUP_SOCKET, "invalid salen"},
    {0, 0, 0, (struct sockaddr *)&sin1,
     sizeof(sin1), -1, ENOTSOCK, SETUP_DEV_NULL, "invalid socket"},
    {PF_INET, SOCK_STREAM, 0, (struct sockaddr *)&sin1,
     sizeof(sin1), -1, EISCONN, SETUP_CONNECTED, "already connected"},
    {PF_INET, SOCK_STREAM, 0, (struct sockaddr *)&sin2,
     sizeof(sin2), -1, ECONNREFUSED, SETUP_SOCKET,
     "connection refused"},
    {PF_INET, SOCK_STREAM, 0, (struct sockaddr *)&sin4,
     sizeof(sin4), -1, EAFNOSUPPORT, SETUP_SOCKET,
     "invalid address family"},
};

// 直接执行 connect syscall；Linux 上绕过 libc 对坏指针的预处理。
static int raw_connect(int sockfd, const struct sockaddr *addr,
                       socklen_t addrlen)
{
    // Linux 使用 syscall(SYS_connect) 保留原测例对无效指针的内核 errno 检查。
#ifdef __linux__
    return (int)syscall(SYS_connect, sockfd, addr, addrlen);
#else
    return connect(sockfd, addr, addrlen);
#endif
}

// 初始化 IPv4 地址。
static void init_ipv4(struct sockaddr_in *addr, in_addr_t ip, in_port_t port)
{
    // 清零结构体后填入地址族、端口和地址。
    memset(addr, 0, sizeof(*addr));
    addr->sin_family = AF_INET;
    addr->sin_port = port;
    addr->sin_addr.s_addr = ip;
}

// 选择一个当前未监听的 TCP 端口，用于 ECONNREFUSED 场景。
static in_port_t pick_unused_tcp_port(void)
{
    // 通过临时 bind(:0) 让内核分配端口。
    int fd = socket(PF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        fprintf(stderr, "server socket failed: %s\n", strerror(errno));
        return 0;
    }

    // 绑定到 INADDR_ANY:0。
    struct sockaddr_in addr;
    init_ipv4(&addr, htonl(INADDR_ANY), 0);
    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        fprintf(stderr, "server bind failed: %s\n", strerror(errno));
        close(fd);
        return 0;
    }

    // 读取内核分配的端口。
    socklen_t len = sizeof(addr);
    if (getsockname(fd, (struct sockaddr *)&addr, &len) != 0) {
        fprintf(stderr, "getsockname failed: %s\n", strerror(errno));
        close(fd);
        return 0;
    }

    // 关闭临时 socket 后，该端口通常会拒绝连接。
    close(fd);
    return addr.sin_port;
}

// SIGTERM 处理函数通知服务端循环退出。
static void handle_sigterm(int signo)
{
    // 只设置异步信号安全的标志位。
    (void)signo;
    server_stop = 1;
}

// 服务端子进程接受连接并保持它们打开一小段时间。
static void server_loop(int listen_fd)
{
    // 安装终止信号处理，父进程清理时可让子进程退出。
    signal(SIGTERM, handle_sigterm);

    // 保存已接受的连接，避免 already connected 场景过早被对端关闭。
    int clients[FD_SETSIZE];
    for (int i = 0; i < FD_SETSIZE; i++)
        clients[i] = -1;

    // 循环直到超时或收到父进程终止通知。
    time_t end_time = time(NULL) + SERVER_TIMEOUT_SEC;
    while (!server_stop && time(NULL) < end_time) {
        fd_set rfds;
        FD_ZERO(&rfds);
        FD_SET(listen_fd, &rfds);
        int max_fd = listen_fd;

        // 把已接受连接加入 select，用于发现对端关闭。
        for (int i = 0; i < FD_SETSIZE; i++) {
            if (clients[i] >= 0) {
                FD_SET(clients[i], &rfds);
                if (clients[i] > max_fd)
                    max_fd = clients[i];
            }
        }

        // select 使用 1 秒超时，确保能定期检查退出条件。
        struct timeval tv;
        tv.tv_sec = 1;
        tv.tv_usec = 0;
        int ret = select(max_fd + 1, &rfds, NULL, NULL, &tv);
        if (ret < 0) {
            if (errno == EINTR)
                continue;
            _exit(1);
        }

        // 接受新的客户端连接，并放入 clients 表。
        if (FD_ISSET(listen_fd, &rfds)) {
            int newfd = accept(listen_fd, NULL, NULL);
            if (newfd >= 0) {
                int stored = 0;
                if (newfd < FD_SETSIZE) {
                    for (int i = 0; i < FD_SETSIZE; i++) {
                        if (clients[i] < 0) {
                            clients[i] = newfd;
                            stored = 1;
                            break;
                        }
                    }
                }
                if (!stored)
                    close(newfd);
            }
        }

        // 对已关闭的客户端连接做回收。
        for (int i = 0; i < FD_SETSIZE; i++) {
            if (clients[i] >= 0 && FD_ISSET(clients[i], &rfds)) {
                char c;
                ssize_t n = read(clients[i], &c, 1);
                if (n <= 0) {
                    close(clients[i]);
                    clients[i] = -1;
                }
            }
        }
    }

    // 退出前关闭所有客户端连接和监听 fd。
    for (int i = 0; i < FD_SETSIZE; i++) {
        if (clients[i] >= 0)
            close(clients[i]);
    }
    close(listen_fd);
    _exit(0);
}

// 启动本地 TCP 服务端。
static pid_t start_server(struct sockaddr_in *addr)
{
    // 创建监听 socket。
    int listen_fd = socket(PF_INET, SOCK_STREAM, 0);
    if (listen_fd < 0) {
        fprintf(stderr, "server socket failed: %s\n", strerror(errno));
        return -1;
    }

    // 绑定到 INADDR_ANY:0，让内核分配端口。
    init_ipv4(addr, htonl(INADDR_ANY), 0);
    if (bind(listen_fd, (struct sockaddr *)addr, sizeof(*addr)) != 0) {
        fprintf(stderr, "server bind failed: %s\n", strerror(errno));
        close(listen_fd);
        return -1;
    }
    if (listen(listen_fd, 10) != 0) {
        fprintf(stderr, "server listen failed: %s\n", strerror(errno));
        close(listen_fd);
        return -1;
    }

    // 读取实际监听端口。
    socklen_t len = sizeof(*addr);
    if (getsockname(listen_fd, (struct sockaddr *)addr, &len) != 0) {
        fprintf(stderr, "getsockname failed: %s\n", strerror(errno));
        close(listen_fd);
        return -1;
    }

    // fork 子进程运行服务端循环。
    pid_t pid = fork();
    if (pid < 0) {
        fprintf(stderr, "server fork failed: %s\n", strerror(errno));
        close(listen_fd);
        return -1;
    }
    if (pid == 0)
        server_loop(listen_fd);

    // 父进程关闭监听 fd，只保留子进程 pid。
    close(listen_fd);
    return pid;
}

// 停止服务端子进程。
static void stop_server(void)
{
    // 如果子进程存在，发送 SIGTERM 并等待回收。
    if (server_pid > 0) {
        kill(server_pid, SIGTERM);
        waitpid(server_pid, NULL, 0);
        server_pid = -1;
    }
}

// 准备 connect01 的全局地址和服务端。
static int setup_global(void)
{
    // 启动服务端，sin1 会被填入实际监听地址。
    server_pid = start_server(&sin1);
    if (server_pid < 0)
        return 1;

    // 准备一个未监听端口地址，用于连接拒绝场景。
    in_port_t unused_port = pick_unused_tcp_port();
    if (unused_port == 0)
        return 1;
    init_ipv4(&sin2, htonl(INADDR_ANY), unused_port);

    // 准备无效地址族场景。
    init_ipv4(&sin4, htonl(0x0AFFFEFD), 0);
    sin4.sin_family = 47;
    return 0;
}

// 为单个测试场景准备 fd。
static int setup_case(const struct test_case *tc)
{
    // 默认没有需要关闭的 fd。
    current_fd = -1;
    current_fd_should_close = 0;

    // 根据场景选择 fd 准备方式。
    switch (tc->setup) {
    case SETUP_BAD_FD:
        current_fd = 400;
        return 0;
    case SETUP_DEV_NULL:
        current_fd = open("/dev/null", O_WRONLY);
        if (current_fd < 0) {
            fprintf(stderr, "open(/dev/null) failed: %s\n", strerror(errno));
            return 1;
        }
        current_fd_should_close = 1;
        return 0;
    case SETUP_SOCKET:
        current_fd = socket(tc->domain, tc->type, tc->proto);
        if (current_fd < 0) {
            fprintf(stderr, "socket() failed: %s\n", strerror(errno));
            return 1;
        }
        current_fd_should_close = 1;
        return 0;
    case SETUP_CONNECTED:
        current_fd = socket(tc->domain, tc->type, tc->proto);
        if (current_fd < 0) {
            fprintf(stderr, "socket() failed: %s\n", strerror(errno));
            return 1;
        }
        current_fd_should_close = 1;
        if (raw_connect(current_fd, (const struct sockaddr *)&sin1,
                        sizeof(sin1)) != 0) {
            fprintf(stderr, "connect setup failed: %s\n", strerror(errno));
            return 1;
        }
        return 0;
    }

    return 1;
}

// 清理单个场景准备的 fd。
static void cleanup_case(void)
{
    // 只有由本测例打开的 fd 才关闭，避免误关固定坏 fd。
    if (current_fd_should_close && current_fd >= 0)
        close(current_fd);
    current_fd = -1;
    current_fd_should_close = 0;
}

// 执行一个 connect() 失败场景。
static int verify_connect(const struct test_case *tc)
{
    // 准备场景 fd。
    if (setup_case(tc) != 0)
        return 1;

    // 清空 errno 后调用 connect syscall。
    errno = 0;
    long ret = raw_connect(current_fd, tc->sockaddr, tc->salen);
    int saved_errno = errno;

    // 按原测例格式检查返回值和 errno。
    int failed = 0;
    if (ret != tc->retval || (ret < 0 && saved_errno != tc->experrno)) {
        fprintf(stderr, "%s ; returned %ld (expected %d), errno %d (expected %d)\n",
                tc->desc, ret, tc->retval, saved_errno, tc->experrno);
        failed = 1;
    } else {
        printf("%s successful\n", tc->desc);
    }

    // 清理当前场景 fd。
    cleanup_case();
    return failed;
}

// 入口函数启动服务端、运行所有 connect01 场景并清理。
int main(void)
{
    // 准备服务端和地址。
    if (setup_global() != 0) {
        stop_server();
        return 1;
    }

    // 逐项运行测试表。
    int failed = 0;
    for (size_t i = 0; i < ARRAY_SIZE(test_cases); i++) {
        if (verify_connect(&test_cases[i]) != 0)
            failed = 1;
    }

    // 停止服务端子进程。
    stop_server();
    return failed ? 1 : 0;
}
