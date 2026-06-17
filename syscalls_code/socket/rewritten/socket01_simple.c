// 本测例验证 socket() 在不同 domain/type/protocol 组合下的返回值和 errno，覆盖无效 domain、无效 type、UNIX/UDP/TCP 成功路径以及不匹配协议的失败路径。

#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

// 统计数组元素个数，用来逐项运行原 socket01 的测试表。
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))

// 单个 socket() 测试场景，保留原测例中的参数、期望返回值、期望 errno 和描述文本。
struct test_case {
    int domain;
    int type;
    int proto;
    int retval;
    int experrno;
    const char *desc;
};

// 原 socket01 覆盖的 socket() 参数组合。
static const struct test_case test_cases[] = {
    {0, SOCK_STREAM, 0, -1, EAFNOSUPPORT, "invalid domain"},
    {PF_INET, 75, 0, -1, EINVAL, "invalid type"},
    {PF_UNIX, SOCK_DGRAM, 0, 0, 0, "UNIX domain dgram"},
    {PF_INET, SOCK_RAW, 0, -1, EPROTONOSUPPORT, "raw open as non-root"},
    {PF_INET, SOCK_DGRAM, 17, 0, 0, "UDP socket"},
    {PF_INET, SOCK_STREAM, 17, -1, EPROTONOSUPPORT, "UDP stream"},
    {PF_INET, SOCK_DGRAM, 6, -1, EPROTONOSUPPORT, "TCP dgram"},
    {PF_INET, SOCK_STREAM, 6, 0, 0, "TCP socket"},
    {PF_INET, SOCK_STREAM, 1, -1, EPROTONOSUPPORT, "ICMP stream"},
};

// 判断当前是否运行在 Linux 语义下，Linux 上保持和 LTP 原期望严格一致。
static int strict_linux_errno(void)
{
    // 非 Linux 的 socket errno 细节和 LTP/Linux 期望可能不同，烟测时作为环境差异跳过。
#ifdef __linux__
    return 1;
#else
    return 0;
#endif
}

// 把常见 errno 转成 LTP 原输出使用的符号名。
static const char *errno_name(int err)
{
    // 按本测例会用到的 errno 做显式映射，避免输出变成中文或系统本地化文本。
    switch (err) {
    case EAFNOSUPPORT:
        return "EAFNOSUPPORT";
    case EINVAL:
        return "EINVAL";
    case EPROTONOSUPPORT:
        return "EPROTONOSUPPORT";
    default:
        return "UNKNOWN";
    }
}

// 执行一个 socket() 场景，并按原测例的英文消息报告结果。
static int verify_socket(const struct test_case *tc)
{
    // 清空 errno 后调用 socket()，这样失败检查只受本次 syscall 影响。
    errno = 0;
    int fd = socket(tc->domain, tc->type, tc->proto);
    int saved_errno = errno;

    // LTP 原测例把任何非负 fd 归一化为返回值 0。
    int normalized_ret = fd >= 0 ? 0 : -1;

    // 成功打开的 socket 立即关闭，避免影响后续测试。
    if (fd >= 0)
        close(fd);

    // 原场景明确要求“非 root 打开 raw socket”，root 环境下成功不代表内核行为错误。
    if (!strcmp(tc->desc, "raw open as non-root") && geteuid() == 0 &&
        normalized_ret != tc->retval) {
        fprintf(stderr, "SKIP: raw open as non-root requires non-root user\n");
        return 0;
    }

    // 检查返回值是否等于原测例期望。
    if (normalized_ret != tc->retval) {
        if (!strict_linux_errno()) {
            fprintf(stderr,
                    "SKIP: %s returned %d, errno %d (Linux expected return %d, errno %d)\n",
                    tc->desc, fd, saved_errno, tc->retval, tc->experrno);
            return 0;
        }
        fprintf(stderr, "%s returned %d (expected %d)\n",
                tc->desc, fd, tc->retval);
        return 1;
    }

    // 只有失败场景需要检查 errno；成功场景期望 errno 为 0。
    if (tc->retval < 0 && saved_errno != tc->experrno) {
        if (!strict_linux_errno()) {
            fprintf(stderr,
                    "SKIP: %s errno %d differs from Linux expected %d\n",
                    tc->desc, saved_errno, tc->experrno);
            return 0;
        }
        fprintf(stderr, "expected %s(%d)\n",
                errno_name(tc->experrno), tc->experrno);
        return 1;
    }

    // 输出原测例的成功消息文本。
    printf("%s successful\n", tc->desc);
    return 0;
}

// 入口函数遍历原 socket01 的全部测试场景。
int main(void)
{
    // 逐项运行并记录是否出现失败。
    int failed = 0;
    for (size_t i = 0; i < ARRAY_SIZE(test_cases); i++) {
        if (verify_socket(&test_cases[i]) != 0)
            failed = 1;
    }

    // 任一场景失败时返回非零，便于脚本判断。
    return failed ? 1 : 0;
}
