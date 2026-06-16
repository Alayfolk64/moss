// 本测例验证 socket(2) 在不同 domain/type/protocol 组合下返回原测例期望的结果或 errno，包括无效 domain、无效 type、UNIX/UDP/TCP 成功路径和不支持的协议组合。

#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

// 单个 socket 测试用例，保存输入参数、期望返回值、期望 errno 和原始描述文本。
struct test_case {
    int domain;
    int type;
    int proto;
    int retval;
    int experrno;
    const char *desc;
};

// 原 socket01 覆盖的测试矩阵。
static struct test_case cases[] = {
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

// 执行一个 socket 测试用例，并按原输出文本报告结果。
static int run_case(const struct test_case *tc)
{
    // 清空 errno 后调用 socket，成功时把返回值归一化为 0，与原测例逻辑一致。
    errno = 0;
    int fd = socket(tc->domain, tc->type, tc->proto);
    int saved_errno = errno;
    int normalized_ret = fd >= 0 ? 0 : -1;

    // 成功创建的 socket 需要关闭，避免描述符泄漏。
    if (fd >= 0)
        close(fd);

    // 检查返回值是否符合期望。
    if (normalized_ret != tc->retval) {
        fprintf(stderr, "%s returned %d (expected %d)\n",
                tc->desc, fd, tc->retval);
        return 1;
    }

    // 失败路径检查 errno，成功路径期望 errno 为 0。
    if (tc->retval < 0 && saved_errno != tc->experrno) {
        fprintf(stderr, "expected %s(%d): %s\n",
                strerror(tc->experrno), tc->experrno, strerror(saved_errno));
        return 1;
    }

    // 当前用例符合原测例期望。
    printf("%s successful\n", tc->desc);
    return 0;
}

// 入口函数遍历所有 socket 参数组合。
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
