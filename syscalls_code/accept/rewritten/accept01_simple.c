// 本测例验证 accept() 在坏 fd、无效地址缓冲区、无效地址长度、没有待接受连接以及 UDP socket 上调用时，是否返回原测例期望的 errno。

#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

// 统计数组元素个数，用于遍历原 accept01 的测试表。
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))

// 明确选择一个通常未打开的 fd，用于触发 EBADF。
static int invalid_socketfd = 400;

// 已 bind 但未 listen 的 TCP socket。
static int socket_fd = -1;

// 已 bind 的 UDP socket。
static int udp_fd = -1;

// TCP socket 绑定地址。
static struct sockaddr_in sin0;

// UDP socket 绑定地址。
static struct sockaddr_in sin1;

// accept() 输出地址缓冲区。
static struct sockaddr_in fsin1;

// 单个 accept() 失败场景。
struct test_case {
    int domain;
    int type;
    int proto;
    int *fd;
    struct sockaddr *sockaddr;
    socklen_t salen;
    int experrno;
    const char *desc;
};

// 原 accept01 覆盖的失败路径。
static struct test_case test_cases[] = {
    {PF_INET, SOCK_STREAM, 0, &invalid_socketfd,
     (struct sockaddr *)&fsin1, sizeof(fsin1), EBADF,
     "bad file descriptor"},
    {PF_INET, SOCK_STREAM, 0, &socket_fd, (struct sockaddr *)3,
     sizeof(fsin1), EINVAL, "invalid socket buffer"},
    {PF_INET, SOCK_STREAM, 0, &socket_fd, (struct sockaddr *)&fsin1,
     1, EINVAL, "invalid salen"},
    {PF_INET, SOCK_STREAM, 0, &socket_fd, (struct sockaddr *)&fsin1,
     sizeof(fsin1), EINVAL, "no queued connections"},
    {PF_INET, SOCK_STREAM, 0, &udp_fd, (struct sockaddr *)&fsin1,
     sizeof(fsin1), EOPNOTSUPP, "UDP accept"},
};

// 初始化 IPv4 地址为 INADDR_ANY:0。
static void init_any_addr(struct sockaddr_in *addr)
{
    // 端口 0 让内核自动分配，避免固定端口冲突。
    memset(addr, 0, sizeof(*addr));
    addr->sin_family = AF_INET;
    addr->sin_port = 0;
    addr->sin_addr.s_addr = htonl(INADDR_ANY);
}

// 准备 TCP 和 UDP socket。
static void setup(void)
{
    // 创建并绑定未 listen 的 TCP socket。
    init_any_addr(&sin0);
    socket_fd = socket(PF_INET, SOCK_STREAM, 0);
    if (socket_fd < 0) {
        fprintf(stderr, "socket(PF_INET, SOCK_STREAM) failed: %s\n", strerror(errno));
        _exit(1);
    }
    if (bind(socket_fd, (struct sockaddr *)&sin0, sizeof(sin0)) != 0) {
        fprintf(stderr, "bind(TCP) failed: %s\n", strerror(errno));
        _exit(1);
    }

    // 创建并绑定 UDP socket。
    init_any_addr(&sin1);
    udp_fd = socket(PF_INET, SOCK_DGRAM, 0);
    if (udp_fd < 0) {
        fprintf(stderr, "socket(PF_INET, SOCK_DGRAM) failed: %s\n", strerror(errno));
        _exit(1);
    }
    if (bind(udp_fd, (struct sockaddr *)&sin1, sizeof(sin1)) != 0) {
        fprintf(stderr, "bind(UDP) failed: %s\n", strerror(errno));
        _exit(1);
    }
}

// 关闭 setup() 打开的 socket。
static void cleanup(void)
{
    // 逐个关闭有效 fd。
    if (socket_fd >= 0)
        close(socket_fd);
    if (udp_fd >= 0)
        close(udp_fd);
}

// 执行一个 accept() 失败场景。
static int verify_accept(const struct test_case *tc)
{
    // accept() 可能改写长度参数，因此每次使用局部副本。
    socklen_t salen = tc->salen;

    // 清空 errno 后调用 accept()。
    errno = 0;
    int ret = accept(*tc->fd, tc->sockaddr, &salen);
    int saved_errno = errno;

    // 检查失败返回值和 errno。
    if (ret == -1 && saved_errno == tc->experrno) {
        printf("%s\n", tc->desc);
        return 0;
    }

    // accept() 意外成功时关闭返回的新 fd，避免泄漏。
    if (ret >= 0)
        close(ret);

    // 报告非预期结果。
    fprintf(stderr, "%s: returned %d, errno %d (expected %d)\n",
            tc->desc, ret, saved_errno, tc->experrno);
    return 1;
}

// 入口函数按原 accept01 测试表运行所有场景。
int main(void)
{
    // 准备 socket。
    setup();

    // 逐项运行并汇总失败状态。
    int failed = 0;
    for (size_t i = 0; i < ARRAY_SIZE(test_cases); i++) {
        if (verify_accept(&test_cases[i]) != 0)
            failed = 1;
    }

    // 清理 socket 后返回结果。
    cleanup();
    return failed ? 1 : 0;
}
