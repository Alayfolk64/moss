// 本测例验证 AF_UNIX SOCK_STREAM socket 绑定后不能再次绑定到另一路径，并且已被一个 socket 绑定的路径不能再绑定到第二个 socket。

#include "bind_simple_common.h"

// 第一个 UNIX socket 路径。
#define SNAME_A "socket.1"

// 第二个 UNIX socket 路径。
#define SNAME_B "socket.2"

// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/bind03_XXXXXX";

// 入口函数执行 UNIX socket 重复绑定检查。
int main(void)
{
    // 创建临时目录并准备两个 UNIX socket。
    simple_enter_temp_dir(temp_dir);
    int sock1 = socket(PF_UNIX, SOCK_STREAM, 0);
    int sock2 = socket(PF_UNIX, SOCK_STREAM, 0);
    if (sock1 < 0 || sock2 < 0) {
        fprintf(stderr, "socket failed: %s\n", strerror(errno));
        return 1;
    }

    // 初始化两个 pathname sockaddr_un。
    struct sockaddr_un sun1, sun2;
    memset(&sun1, 0, sizeof(sun1));
    memset(&sun2, 0, sizeof(sun2));
    sun1.sun_family = AF_UNIX;
    sun2.sun_family = AF_UNIX;
    snprintf(sun1.sun_path, sizeof(sun1.sun_path), "%s", SNAME_A);
    snprintf(sun2.sun_path, sizeof(sun2.sun_path), "%s", SNAME_B);

    // 第一个 socket 先绑定到 SNAME_A。
    if (bind(sock1, (struct sockaddr *)&sun1, sizeof(sun1)) != 0) {
        fprintf(stderr, "bind setup failed: %s\n", strerror(errno));
        return 1;
    }

    // 已绑定 socket 再绑定到 SNAME_B，预期 EINVAL。
    int failed = 0;
    errno = 0;
    if (bind(sock1, (struct sockaddr *)&sun2, sizeof(sun2)) == -1 && errno == EINVAL) {
        printf("re-bind() socket\n");
    } else {
        fprintf(stderr, "re-bind() socket failed unexpectedly: %s\n", strerror(errno));
        failed = 1;
    }

    // 第二个 socket 绑定到已有 pathname，预期 EADDRINUSE。
    errno = 0;
    if (bind(sock2, (struct sockaddr *)&sun1, sizeof(sun1)) == -1 && errno == EADDRINUSE) {
        printf("bind() with bound pathname\n");
    } else {
        fprintf(stderr, "bind() with bound pathname failed unexpectedly: %s\n", strerror(errno));
        failed = 1;
    }

    // 新内核不应在失败路径遗留 SNAME_B。
    if (unlink(SNAME_B) == -1 && errno == ENOENT)
        printf("check exist of SNAME_B\n");
    else
        unlink(SNAME_B);

    // 清理 socket 和路径。
    close(sock1);
    close(sock2);
    unlink(SNAME_A);
    simple_leave_temp_dir(temp_dir);
    return failed ? 1 : 0;
}
