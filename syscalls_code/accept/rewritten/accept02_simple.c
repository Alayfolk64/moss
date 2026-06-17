// 本测例验证 accept() 得到的新 TCP socket 不应继承监听 socket 的 multicast group 信息；若 clone socket 能 leave 同一 group，则说明可能复现 CVE-2017-8890。

#ifdef __linux__

#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

// 原测例使用的 multicast group 地址。
#define MULTICASTIP "224.0.0.0"

// 原测例使用的本地监听地址。
#define LOCALHOSTIP "127.0.0.1"

// 用 2 表示环境不支持该 Linux CVE 测例。
#define EXIT_SKIP 2

// 监听 socket fd。
static int server_sockfd = -1;

// accept() 返回的新 socket fd。
static int clone_server_sockfd = -1;

// 客户端 socket fd。
static int client_sockfd = -1;

// 监听端地址。
static struct sockaddr_in server_addr;

// 客户端本地地址和 accept() 输出地址。
static struct sockaddr_in client_addr;

// multicast group 请求结构。
static struct group_req mc_group;

// 关闭可能打开的 socket。
static void cleanup(void)
{
    // 按 fd 是否有效逐个关闭。
    if (clone_server_sockfd >= 0)
        close(clone_server_sockfd);
    if (client_sockfd >= 0)
        close(client_sockfd);
    if (server_sockfd >= 0)
        close(server_sockfd);
}

// 初始化 socket 地址和 multicast group 参数。
static void setup_addresses(void)
{
    // 准备 multicast group 请求。
    memset(&mc_group, 0, sizeof(mc_group));
    mc_group.gr_interface = 0;
    struct sockaddr_in *mc_group_addr = (struct sockaddr_in *)&mc_group.gr_group;
    mc_group_addr->sin_family = AF_INET;
    inet_aton(MULTICASTIP, &mc_group_addr->sin_addr);

    // 准备监听地址，端口 0 让内核自动分配。
    memset(&server_addr, 0, sizeof(server_addr));
    server_addr.sin_family = AF_INET;
    inet_aton(LOCALHOSTIP, &server_addr.sin_addr);
    server_addr.sin_port = 0;

    // 准备客户端本地地址。
    memset(&client_addr, 0, sizeof(client_addr));
    client_addr.sin_family = AF_INET;
    client_addr.sin_addr.s_addr = htonl(INADDR_ANY);
    client_addr.sin_port = 0;
}

// 创建监听 socket，并加入 multicast group。
static int setup_server(void)
{
    // 创建 TCP server socket。
    server_sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (server_sockfd < 0) {
        fprintf(stderr, "socket() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 允许快速重用本地地址。
    int op = 1;
    if (setsockopt(server_sockfd, SOL_SOCKET, SO_REUSEADDR, &op, sizeof(op)) != 0) {
        fprintf(stderr, "setsockopt(SO_REUSEADDR) failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 在监听 socket 上加入 multicast group，这是 CVE 场景的前置条件。
    if (setsockopt(server_sockfd, SOL_IP, MCAST_JOIN_GROUP,
                   &mc_group, sizeof(mc_group)) != 0) {
        fprintf(stderr, "SKIP: MCAST_JOIN_GROUP failed: %s\n", strerror(errno));
        return EXIT_SKIP;
    }

    // bind 到 loopback 随机端口并开始监听。
    if (bind(server_sockfd, (struct sockaddr *)&server_addr,
             sizeof(server_addr)) != 0) {
        fprintf(stderr, "bind() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    if (listen(server_sockfd, 1) != 0) {
        fprintf(stderr, "listen() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 读取内核分配的端口并保留原测例信息输出文本。
    socklen_t addr_len = sizeof(server_addr);
    if (getsockname(server_sockfd, (struct sockaddr *)&server_addr,
                    &addr_len) != 0) {
        fprintf(stderr, "getsockname() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    printf("Starting listener on port: %d\n", ntohs(server_addr.sin_port));
    return 0;
}

// 创建客户端并连接到监听端。
static int connect_client(void)
{
    // 创建客户端 TCP socket。
    client_sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (client_sockfd < 0) {
        fprintf(stderr, "socket() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 绑定客户端到任意本地端口。
    if (bind(client_sockfd, (struct sockaddr *)&client_addr,
             sizeof(client_addr)) != 0) {
        fprintf(stderr, "bind() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 连接监听 socket，使 accept() 有一个待处理连接。
    if (connect(client_sockfd, (struct sockaddr *)&server_addr,
                sizeof(server_addr)) != 0) {
        fprintf(stderr, "connect() failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    return 0;
}

// 接受连接并检查 clone socket 是否错误继承 multicast group。
static int accept_and_check_clone(void)
{
    // 接受客户端连接。
    socklen_t addr_len = sizeof(client_addr);
    clone_server_sockfd = accept(server_sockfd, (struct sockaddr *)&client_addr,
                                 &addr_len);
    if (clone_server_sockfd < 0) {
        fprintf(stderr, "Could not accept connection: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 尝试在 clone socket 上离开 group；成功表示 group 被错误复制。
    errno = 0;
    int ret = setsockopt(clone_server_sockfd, SOL_IP, MCAST_LEAVE_GROUP,
                         &mc_group, sizeof(mc_group));
    int saved_errno = errno;
    close(clone_server_sockfd);
    clone_server_sockfd = -1;

    // clone socket 能 leave group，说明出现原 CVE 所描述的复制行为。
    if (ret != -1) {
        fprintf(stderr, "Multicast group was copied!\n");
        return EXIT_FAILURE;
    }

    // EADDRNOTAVAIL 表示 clone socket 没有该 group，符合预期。
    if (saved_errno == EADDRNOTAVAIL) {
        printf("Multicast group was not copied\n");
        return 0;
    }

    // 其他 errno 不是本测例期望的结果。
    fprintf(stderr, "setsockopt() failed unexpectedly: %s\n",
            strerror(saved_errno));
    return EXIT_FAILURE;
}

// 入口函数按 setup、connect、accept/check、cleanup 的顺序运行。
int main(void)
{
    // 准备地址结构。
    setup_addresses();

    // 准备服务端。
    int ret = setup_server();
    if (ret != 0) {
        cleanup();
        return ret;
    }

    // 连接客户端并执行 clone socket 检查。
    ret = connect_client();
    if (ret == 0)
        ret = accept_and_check_clone();

    // 清理 socket。
    cleanup();
    return ret;
}

#else

#include <stdio.h>

// 用 2 表示环境不支持该 Linux CVE 测例。
#define EXIT_SKIP 2

// 非 Linux 系统没有该 Linux multicast CVE 场景，保留可编译入口。
int main(void)
{
    // accept02 是 Linux multicast group socket 语义测例。
    fprintf(stderr, "SKIP: accept02 requires Linux multicast group socket options\n");
    return EXIT_SKIP;
}

#endif
