// 本测例验证已经绑定的 AF_UNIX/SOCK_STREAM socket 不能再次 bind，并验证已有 pathname 不能被第二个 socket 重新 bind。

#include "bind_simple_common.h"

#include <sys/utsname.h>

// 第一个 UNIX socket pathname。
#define SNAME_A "socket.1"

// 第二个 UNIX socket pathname，用于验证失败路径是否遗留节点。
#define SNAME_B "socket.2"

// 临时目录模板，避免 pathname socket 文件污染源码目录。
static char temp_dir[] = "/tmp/bind03_XXXXXX";

// 已经绑定到 SNAME_A 的 socket。
static int sock1 = -1;

// 用来尝试绑定同一 pathname 的第二个 socket。
static int sock2 = -1;

// 第一个 socket 地址。
static struct sockaddr_un sun1;

// 第二个 socket 地址。
static struct sockaddr_un sun2;

// 判断当前 Linux 内核版本是否至少为指定版本。
static int linux_kernel_at_least(int major, int minor, int patch)
{
    // 非 Linux 或无法解析版本时按新内核处理，只检查不应遗留 SNAME_B。
    struct utsname uts;
    if (uname(&uts) != 0)
        return 1;

    // 解析 release 前三个数字。
    int got_major = 0;
    int got_minor = 0;
    int got_patch = 0;
    if (sscanf(uts.release, "%d.%d.%d", &got_major, &got_minor, &got_patch) < 2)
        return 1;

    // 逐级比较版本号。
    if (got_major != major)
        return got_major > major;
    if (got_minor != minor)
        return got_minor > minor;
    return got_patch >= patch;
}

// 准备两个 AF_UNIX stream socket，并把第一个绑定到 SNAME_A。
static int setup(void)
{
    // 进入临时目录，模拟原测例的 needs_tmpdir。
    simple_enter_temp_dir(temp_dir);

    // 创建两个 AF_UNIX stream socket。
    sock1 = socket(PF_UNIX, SOCK_STREAM, 0);
    if (sock1 < 0)
        simple_die_errno("socket sock1 failed");
    sock2 = socket(PF_UNIX, SOCK_STREAM, 0);
    if (sock2 < 0)
        simple_die_errno("socket sock2 failed");

    // 构造两个 pathname 地址。
    memset(&sun1, 0, sizeof(sun1));
    memset(&sun2, 0, sizeof(sun2));
    sun1.sun_family = AF_UNIX;
    sun2.sun_family = AF_UNIX;
    if (snprintf(sun1.sun_path, sizeof(sun1.sun_path), "%s", SNAME_A) < (int)strlen(SNAME_A)) {
        fprintf(stderr, "sprintf failed\n");
        return 1;
    }
    if (snprintf(sun2.sun_path, sizeof(sun2.sun_path), "%s", SNAME_B) < (int)strlen(SNAME_B)) {
        fprintf(stderr, "sprintf failed\n");
        return 1;
    }

    // 第一个 socket 先成功绑定到 SNAME_A。
    if (bind(sock1, (struct sockaddr *)&sun1, sizeof(sun1)) != 0)
        simple_die_errno("initial bind failed");

    return 0;
}

// 清理 socket、pathname 节点和临时目录。
static void cleanup(void)
{
    // 关闭两个 socket。
    if (sock1 >= 0)
        close(sock1);
    if (sock2 >= 0)
        close(sock2);

    // 删除可能存在的 pathname socket 文件。
    unlink(SNAME_A);
    unlink(SNAME_B);
    simple_leave_temp_dir(temp_dir);
}

// 检查一次失败的 bind/unlink 调用是否返回指定 errno。
static int expect_fail_errno(int ret, int saved_errno, int experrno,
                             const char *desc)
{
    // 返回值和 errno 都符合预期时输出原测例描述文本。
    if (ret == -1 && saved_errno == experrno) {
        printf("%s\n", desc);
        return 0;
    }

    // 不符合预期时报告实际返回值和 errno。
    fprintf(stderr, "%s: returned %d, errno %d (expected %d)\n",
            desc, ret, saved_errno, experrno);
    return 1;
}

// 执行 bind03 的两个 bind 失败场景和 SNAME_B 遗留检查。
static int run_test(void)
{
    // 已绑定的 stream UNIX socket 再次 bind() 应返回 EINVAL。
    errno = 0;
    int ret = bind(sock1, (struct sockaddr *)&sun2, sizeof(sun2));
    int saved_errno = errno;
    int failed = expect_fail_errno(ret, saved_errno, EINVAL, "re-bind() socket");

    // 已被 SNAME_A 占用的 pathname 不能绑定到第二个 socket，应返回 EADDRINUSE。
    errno = 0;
    ret = bind(sock2, (struct sockaddr *)&sun1, sizeof(sun1));
    saved_errno = errno;
    if (expect_fail_errno(ret, saved_errno, EADDRINUSE, "bind() with bound pathname") != 0)
        failed = 1;

    // 5.14 及之后的内核不应在失败路径遗留 SNAME_B。
    if (linux_kernel_at_least(5, 14, 0)) {
        errno = 0;
        ret = unlink(SNAME_B);
        saved_errno = errno;
        if (expect_fail_errno(ret, saved_errno, ENOENT, "check exist of SNAME_B") != 0)
            failed = 1;
    } else {
        unlink(SNAME_B);
    }

    return failed;
}

// 入口函数准备资源、运行测例并清理。
int main(void)
{
    // 初始化 socket 和 pathname。
    if (setup() != 0) {
        cleanup();
        return 1;
    }

    // 执行测试主体。
    int failed = run_test();

    // 清理临时资源。
    cleanup();
    return failed ? 1 : 0;
}
