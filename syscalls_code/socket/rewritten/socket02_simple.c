// 本测例验证 socket() 创建 socket 时 SOCK_CLOEXEC 和 SOCK_NONBLOCK 标志是否正确反映到 fcntl(F_GETFD/F_GETFL) 查询结果中。

#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

// 统计数组元素个数，用来遍历原 socket02 的测试表。
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))

// 某些系统头没有 SOCK_CLOEXEC；补零只用于编译，运行时会跳过对应场景。
#ifndef SOCK_CLOEXEC
#define SOCK_CLOEXEC 0
// 标记当前系统无法测试 SOCK_CLOEXEC。
#define HAVE_SOCK_CLOEXEC 0
#else
// 标记当前系统可以测试 SOCK_CLOEXEC。
#define HAVE_SOCK_CLOEXEC 1
#endif

// 某些系统头没有 SOCK_NONBLOCK；补零只用于编译，运行时会跳过对应场景。
#ifndef SOCK_NONBLOCK
#define SOCK_NONBLOCK 0
// 标记当前系统无法测试 SOCK_NONBLOCK。
#define HAVE_SOCK_NONBLOCK 0
#else
// 标记当前系统可以测试 SOCK_NONBLOCK。
#define HAVE_SOCK_NONBLOCK 1
#endif

// 单个 socket flag 测试场景，mask 表示要检查的 fcntl 位，expected 表示期望状态。
struct test_case {
    int type;
    int expected;
    int command;
    int mask;
    int supported;
    const char *des;
};

// 原 socket02 覆盖的 close-on-exec 和 non-blocking 标志场景。
static const struct test_case test_cases[] = {
    {SOCK_STREAM, 0, F_GETFD, FD_CLOEXEC, 1, "no close-on-exec"},
    {SOCK_STREAM | SOCK_CLOEXEC, FD_CLOEXEC, F_GETFD, FD_CLOEXEC,
     HAVE_SOCK_CLOEXEC, "close-on-exec"},
    {SOCK_STREAM, 0, F_GETFL, O_NONBLOCK, 1, "no non-blocking"},
    {SOCK_STREAM | SOCK_NONBLOCK, O_NONBLOCK, F_GETFL, O_NONBLOCK,
     HAVE_SOCK_NONBLOCK, "non-blocking"},
};

// 执行一个 socket flag 场景，并保留原测例的英文输出文本。
static int verify_socket_flag(const struct test_case *tc)
{
    // 如果系统头不支持该 flag，跳过这个 Linux 新 flag 场景。
    if (!tc->supported) {
        fprintf(stderr, "SKIP: socket() flag for %s is not available\n",
                tc->des);
        return 0;
    }

    // 创建带指定 type/flag 的 IPv4 stream socket。
    int fd = socket(PF_INET, tc->type, 0);
    if (fd == -1) {
        fprintf(stderr, "socket() failed: %s\n", strerror(errno));
        return 1;
    }

    // 用 fcntl 读取 fd 标志或文件状态标志。
    int res = fcntl(fd, tc->command);
    if (res == -1) {
        fprintf(stderr, "fcntl() failed: %s\n", strerror(errno));
        close(fd);
        return 1;
    }

    // 检查目标 flag 是否和期望一致。
    int failed = 0;
    if ((res & tc->mask) != tc->expected) {
        fprintf(stderr, "socket() failed to set %s flag\n", tc->des);
        failed = 1;
    } else {
        printf("socket() passed to set %s flag\n", tc->des);
    }

    // 关闭 socket，避免影响后续场景。
    close(fd);
    return failed;
}

// 入口函数遍历原 socket02 的全部 flag 场景。
int main(void)
{
    // 逐项运行并汇总失败状态。
    int failed = 0;
    for (size_t i = 0; i < ARRAY_SIZE(test_cases); i++) {
        if (verify_socket_flag(&test_cases[i]) != 0)
            failed = 1;
    }

    // 任一场景失败时返回非零。
    return failed ? 1 : 0;
}
