// 本测例验证非 root 用户绑定 TCP 低端口时 bind(2) 应失败并返回 EACCES；运行需要 root 以便切换到 nobody。

#include "bind_simple_common.h"
#include <grp.h>

// 原测例使用的特权端口。
#define TCP_PRIVILEGED_PORT 463

// 原测例切换到的用户名。
#define TEST_USERNAME "nobody"

// 入口函数切换身份并执行低端口 bind 检查。
int main(void)
{
    // 需要 root 才能切换到 nobody。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: root privileges are required\n");
        return EXIT_SKIP;
    }

    // 查找 nobody 用户和主组。
    struct passwd *pw = getpwnam(TEST_USERNAME);
    if (!pw) {
        fprintf(stderr, "SKIP: nobody user is required\n");
        return EXIT_SKIP;
    }
    struct group *gr = getgrgid(pw->pw_gid);
    if (!gr) {
        fprintf(stderr, "SKIP: nobody group is required\n");
        return EXIT_SKIP;
    }

    // 输出原测例的身份切换信息。
    printf("Switching credentials to user: %s, group: %s\n",
           pw->pw_name, gr->gr_name);

    // 切换有效 gid/uid。
    if (setegid(gr->gr_gid) != 0 || seteuid(pw->pw_uid) != 0) {
        fprintf(stderr, "seteuid/setegid failed: %s\n", strerror(errno));
        return 1;
    }

    // 创建 TCP socket 并准备低端口地址。
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0) {
        fprintf(stderr, "socket failed: %s\n", strerror(errno));
        return 1;
    }
    struct sockaddr_in servaddr;
    memset(&servaddr, 0, sizeof(servaddr));
    servaddr.sin_family = AF_INET;
    servaddr.sin_port = htons(TCP_PRIVILEGED_PORT);
    servaddr.sin_addr.s_addr = htonl(INADDR_ANY);

    // 调用 bind，预期非 root 绑定特权端口失败。
    errno = 0;
    int ret = bind(sockfd, (struct sockaddr *)&servaddr, sizeof(servaddr));
    int saved_errno = errno;
    close(sockfd);

    // 检查 EACCES。
    if (ret == -1 && saved_errno == EACCES) {
        printf("bind() failed as expected\n");
        return 0;
    }

    // 某些系统允许非 root 绑定低端口，这时按环境差异报告。
    fprintf(stderr, "bind() failed unexpectedly; expected: %d - %s, got: %d - %s\n",
            EACCES, strerror(EACCES), saved_errno, strerror(saved_errno));
    return 1;
}
