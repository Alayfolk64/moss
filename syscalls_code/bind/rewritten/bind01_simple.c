// 本测例验证 bind() 在无效地址长度、非 socket fd、合法 INADDR_ANY:0、地址族不匹配、非本地地址、无效 fd 以及路径前缀不是目录时的返回值和 errno。

#include "bind_simple_common.h"

// 用于构造 ENOTDIR 场景的普通文件名。
#define DIR_ENOTDIR "dir_enotdir"

// 用于构造 ENOTDIR 场景的 socket 路径后缀。
#define TEST_ENOTDIR "test_enotdir"

// 临时目录模板，使相对路径测试文件不污染源码目录。
static char temp_dir[] = "/tmp/bind01_XXXXXX";

// IPv4 stream socket，多个场景复用。
static int inet_socket = -1;

// /dev/null 文件描述符，用于触发 ENOTSOCK。
static int dev_null = -1;

// 明确无效的文件描述符，用于触发 EBADF。
static int fd_ebadf = -1;

// AF_UNIX socket，用于触发路径前缀 ENOTDIR。
static int fd_enotdir = -1;

// invalid salen 场景使用的 IPv4 地址。
static struct sockaddr_in sin1;

// 合法 INADDR_ANY:0 场景使用的 IPv4 地址。
static struct sockaddr_in sin2;

// 非本地地址场景使用的 IPv4 地址。
static struct sockaddr_in sin3;

// 地址族不匹配场景使用的 UNIX 地址。
static struct sockaddr_un sun;

// 路径前缀不是目录场景使用的 UNIX 地址。
static struct sockaddr_un sock_enotdir;

// 单个 bind() 测试场景。
struct test_case {
    int *socket_fd;
    struct sockaddr *sockaddr;
    socklen_t salen;
    int retval;
    int experrno;
    const char *desc;
};

// 原 bind01 覆盖的错误和成功路径。
static struct test_case test_cases[] = {
    {&inet_socket, (struct sockaddr *)&sin1, 3, -1, EINVAL, "invalid salen"},
    {&dev_null, (struct sockaddr *)&sin1, sizeof(sin1), -1, ENOTSOCK, "invalid socket"},
    {&inet_socket, (struct sockaddr *)&sin2, sizeof(sin2), 0, 0, "INADDR_ANYPORT"},
    {&inet_socket, (struct sockaddr *)&sun, sizeof(sun), -1, EAFNOSUPPORT, "UNIX-domain of current directory"},
    {&inet_socket, (struct sockaddr *)&sin3, sizeof(sin3), -1, EADDRNOTAVAIL, "non-local address"},
    {&fd_ebadf, (struct sockaddr *)&sin1, sizeof(sin1), -1, EBADF, "sockfd is not a valid file descriptor"},
    {&fd_enotdir, (struct sockaddr *)&sock_enotdir, sizeof(sock_enotdir), -1, ENOTDIR, "a component of addr prefix is not a directory"},
};

// 打开一个 IPv4 stream socket。
static int open_inet_socket(void)
{
    // 创建失败说明环境无法执行 bind01。
    int fd = socket(PF_INET, SOCK_STREAM, 0);
    if (fd < 0)
        simple_die_errno("socket(PF_INET) failed");
    return fd;
}

// 准备 bind01 所需的 socket、地址和普通文件。
static void setup(void)
{
    // 进入临时目录，模拟 LTP 的 needs_tmpdir。
    simple_enter_temp_dir(temp_dir);

    // 初始化 invalid salen 场景的地址。
    init_ipv4(&sin1, NULL, 0);

    // 初始化合法 INADDR_ANY:0 地址。
    init_ipv4(&sin2, NULL, 0);

    // 初始化一个通常不属于本机接口的 IPv4 地址。
    init_ipv4(&sin3, NULL, 0);
    sin3.sin_addr.s_addr = htonl(0x0AFFFEFD);

    // UNIX 地址指向当前目录，用于触发地址族不支持。
    memset(&sun, 0, sizeof(sun));
    sun.sun_family = AF_UNIX;
    strncpy(sun.sun_path, ".", sizeof(sun.sun_path) - 1);

    // 创建普通文件，使后续路径中的前缀不是目录。
    int file_fd = open(DIR_ENOTDIR, O_CREAT | O_WRONLY | O_TRUNC, 0777);
    if (file_fd < 0)
        simple_die_errno("touch dir_enotdir failed");
    close(file_fd);

    // UNIX 地址穿过普通文件前缀，预期 bind() 返回 ENOTDIR。
    memset(&sock_enotdir, 0, sizeof(sock_enotdir));
    sock_enotdir.sun_family = AF_UNIX;
    snprintf(sock_enotdir.sun_path, sizeof(sock_enotdir.sun_path),
             "%s/%s", DIR_ENOTDIR, TEST_ENOTDIR);

    // 打开所有测试会用到的 fd。
    inet_socket = open_inet_socket();
    dev_null = open("/dev/null", O_WRONLY);
    if (dev_null < 0)
        simple_die_errno("open(/dev/null) failed");
    fd_enotdir = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd_enotdir < 0)
        simple_die_errno("socket(AF_UNIX) failed");
}

// 清理 bind01 创建的 fd、文件和临时目录。
static void cleanup(void)
{
    // 关闭仍然打开的文件描述符。
    if (inet_socket >= 0)
        close(inet_socket);
    if (dev_null >= 0)
        close(dev_null);
    if (fd_enotdir >= 0)
        close(fd_enotdir);

    // 删除临时普通文件和目录。
    unlink(DIR_ENOTDIR);
    simple_leave_temp_dir(temp_dir);
}

// 执行一个 bind01 场景，并检查返回值和 errno。
static int verify_bind(const struct test_case *tc)
{
    // 清空 errno 后调用 bind()。
    errno = 0;
    int ret = bind(*tc->socket_fd, tc->sockaddr, tc->salen);
    int saved_errno = errno;

    // 检查期望失败的场景。
    if (tc->experrno) {
        if (ret == -1 && saved_errno == tc->experrno) {
            printf("%s\n", tc->desc);
            return 0;
        }
        fprintf(stderr, "%s: returned %d, errno %d (expected %d)\n",
                tc->desc, ret, saved_errno, tc->experrno);
        return 1;
    }

    // 检查期望成功的 INADDR_ANYPORT 场景。
    if (ret == tc->retval) {
        printf("%s\n", tc->desc);
        close(inet_socket);
        inet_socket = open_inet_socket();
        return 0;
    }

    // 报告非预期成功/失败结果。
    fprintf(stderr, "%s: returned %d, errno %d (expected return %d)\n",
            tc->desc, ret, saved_errno, tc->retval);
    return 1;
}

// 入口函数按原 bind01 表顺序运行所有场景。
int main(void)
{
    // 准备资源。
    setup();

    // 遍历测试表并汇总失败状态。
    int failed = 0;
    for (size_t i = 0; i < SIMPLE_ARRAY_SIZE(test_cases); i++) {
        if (verify_bind(&test_cases[i]) != 0)
            failed = 1;
    }

    // 统一清理后返回结果。
    cleanup();
    return failed ? 1 : 0;
}
