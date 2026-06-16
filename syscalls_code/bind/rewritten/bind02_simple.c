// 本测例验证非 root 用户 bind() 到特权 TCP 端口时应失败并返回 EACCES；测例需要先以 root 身份切换到 nobody 用户。

#include "bind_simple_common.h"

#include <grp.h>

// 原测例使用的特权 TCP 端口。
#define TCP_PRIVILEGED_PORT 463

// 原测例切换到的非特权用户名。
#define TEST_USERNAME "nobody"

// 切换到 nobody 用户和其主组。
static int switch_to_nobody(void)
{
    // 原 LTP 测例声明 needs_root，非 root 环境无法可靠切换身份。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: bind02 requires root to switch credentials\n");
        return EXIT_SKIP;
    }

    // 查找 nobody 用户和对应组。
    struct passwd *pw = getpwnam(TEST_USERNAME);
    if (!pw) {
        fprintf(stderr, "getpwnam(%s) failed\n", TEST_USERNAME);
        return EXIT_FAILURE;
    }
    struct group *gr = getgrgid(pw->pw_gid);
    if (!gr) {
        fprintf(stderr, "getgrgid(%ld) failed\n", (long)pw->pw_gid);
        return EXIT_FAILURE;
    }

    // 保留原测例的信息输出文本。
    printf("Switching credentials to user: %s, group: %s\n",
           pw->pw_name, gr->gr_name);

    // 先切组再切用户，避免丢失设置组的权限。
    if (setegid(gr->gr_gid) != 0) {
        fprintf(stderr, "setegid() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    if (seteuid(pw->pw_uid) != 0) {
        fprintf(stderr, "seteuid() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    return 0;
}

// 执行特权端口 bind() 检查。
static int run_test(void)
{
    // 创建 IPv4 stream socket。
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0)
        simple_die_errno("socket() failed");

    // 构造 INADDR_ANY:TCP_PRIVILEGED_PORT 地址。
    struct sockaddr_in servaddr;
    init_ipv4(&servaddr, NULL, htons(TCP_PRIVILEGED_PORT));

    // bind() 预期因为非特权用户绑定低端口而失败。
    errno = 0;
    int ret = bind(sockfd, (struct sockaddr *)&servaddr, sizeof(servaddr));
    int saved_errno = errno;
    close(sockfd);

    // 检查是否得到 EACCES。
    if (ret == -1 && saved_errno == EACCES) {
        printf("bind()\n");
        return 0;
    }

    // 报告非预期结果；现代 Linux 可能通过 sysctl 放开低端口限制。
    fprintf(stderr, "bind(): returned %d, errno %d (expected %d)\n",
            ret, saved_errno, EACCES);
    return 1;
}

// 入口函数先切换身份，再运行 bind() 权限测例。
int main(void)
{
    // 切换到 nobody；skip 直接返回 2。
    int ret = switch_to_nobody();
    if (ret != 0)
        return ret;

    // 执行测试主体。
    return run_test();
}
