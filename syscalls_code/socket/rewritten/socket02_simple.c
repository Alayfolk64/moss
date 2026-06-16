// 本测例验证 socket(2) 创建时携带 SOCK_CLOEXEC 或 SOCK_NONBLOCK 标志后，fcntl 能读到对应的 FD_CLOEXEC 或 O_NONBLOCK 状态。

#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

// 旧系统头可能没有 SOCK_CLOEXEC，这里只为编译提供 Linux 常用值。
#ifndef SOCK_CLOEXEC
#define SOCK_CLOEXEC 02000000
#endif

// 旧系统头可能没有 SOCK_NONBLOCK，这里只为编译提供 Linux 常用值。
#ifndef SOCK_NONBLOCK
#define SOCK_NONBLOCK 00004000
#endif

// 单个 flag 检查用例，保存 socket type、期望 flag、fcntl 命令和原始描述文本。
struct test_case {
    int type;
    int flag;
    int fl_cmd;
    const char *des;
};

// 原 socket02 覆盖的 close-on-exec 和 non-blocking 组合。
static struct test_case cases[] = {
    {SOCK_STREAM, 0, F_GETFD, "no close-on-exec"},
    {SOCK_STREAM | SOCK_CLOEXEC, FD_CLOEXEC, F_GETFD, "close-on-exec"},
    {SOCK_STREAM, 0, F_GETFL, "no non-blocking"},
    {SOCK_STREAM | SOCK_NONBLOCK, O_NONBLOCK, F_GETFL, "non-blocking"},
};

// 执行单个 flag 检查。
static int run_case(const struct test_case *tc)
{
    // 创建 IPv4 stream socket，type 中可能带 SOCK_CLOEXEC 或 SOCK_NONBLOCK。
    int fd = socket(PF_INET, tc->type, 0);
    if (fd == -1) {
        fprintf(stderr, "socket() failed: %s\n", strerror(errno));
        return 1;
    }

    // 读取描述符或文件状态 flag。
    int res = fcntl(fd, tc->fl_cmd);
    if (res == -1) {
        fprintf(stderr, "fcntl() failed: %s\n", strerror(errno));
        close(fd);
        return 1;
    }

    // 检查期望置位的 flag 是否存在。
    int failed = 0;
    if (tc->flag != 0 && (res & tc->flag) == 0) {
        fprintf(stderr, "socket() failed to set %s flag\n", tc->des);
        failed = 1;
    }

    // 检查期望未置位的 flag 是否保持未置位。
    if (tc->flag == 0 && (res & tc->flag) != 0) {
        fprintf(stderr, "socket() failed to set %s flag\n", tc->des);
        failed = 1;
    }

    // 当前 flag 状态符合预期。
    if (!failed)
        printf("socket() passed to set %s flag\n", tc->des);

    // 关闭 socket。
    close(fd);
    return failed;
}

// 入口函数遍历所有 flag 用例。
int main(void)
{
    // 逐项运行原测例表。
    int failed = 0;
    for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
        if (run_case(&cases[i]) != 0)
            failed = 1;
    }

    return failed ? 1 : 0;
}
