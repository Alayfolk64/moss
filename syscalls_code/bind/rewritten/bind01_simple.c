// 本测例验证 bind(2) 的基础错误路径和一个成功路径，包括无效地址长度、非 socket fd、INADDR_ANY 自动端口、地址族不匹配、非本地地址、坏 fd、以及 UNIX 路径前缀不是目录时的 ENOTDIR。

#include "bind_simple_common.h"

// 用于触发 ENOTDIR 的普通文件名。
#define DIR_ENOTDIR "dir_enotdir"

// 用于触发 ENOTDIR 的 UNIX socket 路径尾部。
#define TEST_ENOTDIR "test_enotdir"

// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/bind01_XXXXXX";

// 单个 bind 测试用例，保存 fd 指针、地址、地址长度、期望结果和原始描述文本。
struct bind_case {
    int *socket_fd;
    struct sockaddr *sockaddr;
    socklen_t salen;
    int retval;
    int experrno;
    const char *desc;
};

// 测试用 socket 和辅助 fd。
static int inet_socket;
static int dev_null;
static int fd_ebadf = -1;
static int fd_enotdir;

// 测试用地址结构。
static struct sockaddr_in sin1, sin2, sin3;
static struct sockaddr_un sun, sock_enotdir;

// 原 bind01 覆盖的测试矩阵。
static struct bind_case cases[] = {
    {&inet_socket, (struct sockaddr *)&sin1, 3, -1, EINVAL, "invalid salen"},
    {&dev_null, (struct sockaddr *)&sin1, sizeof(sin1), -1, ENOTSOCK, "invalid socket"},
    {&inet_socket, (struct sockaddr *)&sin2, sizeof(sin2), 0, 0, "INADDR_ANYPORT"},
    {&inet_socket, (struct sockaddr *)&sun, sizeof(sun), -1, EAFNOSUPPORT, "UNIX-domain of current directory"},
    {&inet_socket, (struct sockaddr *)&sin3, sizeof(sin3), -1, EADDRNOTAVAIL, "non-local address"},
    {&fd_ebadf, (struct sockaddr *)&sin1, sizeof(sin1), -1, EBADF, "sockfd is not a valid file descriptor"},
    {&fd_enotdir, (struct sockaddr *)&sock_enotdir, sizeof(sock_enotdir), -1, ENOTDIR, "a component of addr prefix is not a directory"},
};

// 准备所有 socket 和地址结构。
static void setup(void)
{
    // 进入临时目录，避免 UNIX socket 路径污染源码目录。
    simple_enter_temp_dir(temp_dir);

    // 初始化第一个 IPv4 地址，端口 0 让内核分配未使用端口。
    init_ipv4(&sin1, NULL, 0);

    // 初始化成功路径使用的 INADDR_ANY + port 0 地址。
    init_ipv4(&sin2, NULL, 0);

    // 初始化非本地地址，沿用原测例假设 10.255.254.253 不是本地接口。
    memset(&sin3, 0, sizeof(sin3));
    sin3.sin_family = AF_INET;
    sin3.sin_port = 0;
    sin3.sin_addr.s_addr = htonl(0x0AFFFEFD);

    // 初始化错误地址族的 UNIX socket 地址。
    memset(&sun, 0, sizeof(sun));
    sun.sun_family = AF_UNIX;
    strncpy(sun.sun_path, ".", sizeof(sun.sun_path) - 1);

    // 创建普通文件作为路径前缀，随后 UNIX bind 应返回 ENOTDIR。
    int fd = open(DIR_ENOTDIR, O_CREAT | O_RDWR | O_TRUNC, 0777);
    if (fd >= 0)
        close(fd);
    memset(&sock_enotdir, 0, sizeof(sock_enotdir));
    sock_enotdir.sun_family = AF_UNIX;
    snprintf(sock_enotdir.sun_path, sizeof(sock_enotdir.sun_path),
             "%s/%s", DIR_ENOTDIR, TEST_ENOTDIR);

    // 创建测试 fd。
    inet_socket = socket(PF_INET, SOCK_STREAM, 0);
    dev_null = open("/dev/null", O_WRONLY);
    fd_enotdir = socket(AF_UNIX, SOCK_STREAM, 0);
    if (inet_socket < 0 || dev_null < 0 || fd_enotdir < 0) {
        fprintf(stderr, "setup failed: %s\n", strerror(errno));
        exit(1);
    }
}

// 清理所有 fd 和临时目录。
static void cleanup(void)
{
    // 关闭测试 fd。
    if (inet_socket >= 0)
        close(inet_socket);
    if (dev_null >= 0)
        close(dev_null);
    if (fd_enotdir >= 0)
        close(fd_enotdir);

    // 删除临时文件并离开临时目录。
    unlink(DIR_ENOTDIR);
    simple_leave_temp_dir(temp_dir);
}

// 执行单个 bind 用例。
static int run_case(struct bind_case *tc)
{
    // 调用 bind 并保存 errno。
    errno = 0;
    int ret = bind(*tc->socket_fd, tc->sockaddr, tc->salen);
    int saved_errno = errno;

    // 检查成功路径。
    if (tc->experrno == 0) {
        if (ret == 0) {
            printf("%s\n", tc->desc);
            close(inet_socket);
            inet_socket = socket(PF_INET, SOCK_STREAM, 0);
            return 0;
        }
        fprintf(stderr, "%s failed unexpectedly: %s\n", tc->desc, strerror(saved_errno));
        return 1;
    }

    // 检查失败路径的 errno。
    if (ret == -1 && saved_errno == tc->experrno) {
        printf("%s\n", tc->desc);
        return 0;
    }

    // 报告非预期返回值。
    fprintf(stderr, "%s failed unexpectedly; expected: %d - %s, got: %d - %s\n",
            tc->desc, tc->experrno, strerror(tc->experrno),
            saved_errno, strerror(saved_errno));
    return 1;
}

// 入口函数遍历 bind01 的所有场景。
int main(void)
{
    // 准备测试资源。
    setup();

    // 逐项执行测试表。
    int failed = 0;
    for (size_t i = 0; i < SIMPLE_ARRAY_SIZE(cases); i++) {
        if (run_case(&cases[i]) != 0)
            failed = 1;
    }

    // 清理资源。
    cleanup();
    return failed ? 1 : 0;
}
