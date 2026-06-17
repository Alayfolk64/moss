// 本测例验证多种标准 stream 协议的 socket 可以 bind、listen、accept，并与客户端完成一次简单请求/响应通信。

#include "bind_simple_common.h"

// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/bind04_XXXXXX";

// UNIX pathname stream 地址。
static struct sockaddr_un unix_addr = {
    .sun_family = AF_UNIX,
    .sun_path = MAIN_SOCKET_FILE,
};

// UNIX abstract stream 地址。
static struct sockaddr_un abstract_addr = {
    .sun_family = AF_UNIX,
    .sun_path = ABSTRACT_SOCKET_PATH,
};

// IPv4 loopback 地址。
static struct sockaddr_in ipv4_addr;

// IPv4 wildcard 地址。
static struct sockaddr_in ipv4_any_addr;

// IPv6 loopback 地址。
static struct sockaddr_in6 ipv6_addr;

// IPv6 wildcard 地址。
static struct sockaddr_in6 ipv6_any_addr;

// 原 bind04 覆盖的 stream 协议列表。
static struct bind_test_case testcase_list[] = {
    {SOCK_STREAM, 0, (struct sockaddr *)&unix_addr, sizeof(unix_addr), "AF_UNIX pathname stream"},
    {SOCK_SEQPACKET, 0, (struct sockaddr *)&unix_addr, sizeof(unix_addr), "AF_UNIX pathname seqpacket"},
    {SOCK_STREAM, 0, (struct sockaddr *)&abstract_addr, sizeof(abstract_addr), "AF_UNIX abstract stream"},
    {SOCK_SEQPACKET, 0, (struct sockaddr *)&abstract_addr, sizeof(abstract_addr), "AF_UNIX abstract seqpacket"},
    {SOCK_STREAM, 0, (struct sockaddr *)&ipv4_addr, sizeof(ipv4_addr), "IPv4 loop TCP variant 1"},
    {SOCK_STREAM, IPPROTO_TCP, (struct sockaddr *)&ipv4_addr, sizeof(ipv4_addr), "IPv4 loop TCP variant 2"},
    {SOCK_STREAM, IPPROTO_SCTP, (struct sockaddr *)&ipv4_addr, sizeof(ipv4_addr), "IPv4 loop SCTP"},
    {SOCK_STREAM, 0, (struct sockaddr *)&ipv4_any_addr, sizeof(ipv4_any_addr), "IPv4 any TCP variant 1"},
    {SOCK_STREAM, IPPROTO_TCP, (struct sockaddr *)&ipv4_any_addr, sizeof(ipv4_any_addr), "IPv4 any TCP variant 2"},
    {SOCK_STREAM, IPPROTO_SCTP, (struct sockaddr *)&ipv4_any_addr, sizeof(ipv4_any_addr), "IPv4 any SCTP"},
    {SOCK_STREAM, 0, (struct sockaddr *)&ipv6_addr, sizeof(ipv6_addr), "IPv6 loop TCP variant 1"},
    {SOCK_STREAM, IPPROTO_TCP, (struct sockaddr *)&ipv6_addr, sizeof(ipv6_addr), "IPv6 loop TCP variant 2"},
    {SOCK_STREAM, IPPROTO_SCTP, (struct sockaddr *)&ipv6_addr, sizeof(ipv6_addr), "IPv6 loop SCTP"},
    {SOCK_STREAM, 0, (struct sockaddr *)&ipv6_any_addr, sizeof(ipv6_any_addr), "IPv6 any TCP variant 1"},
    {SOCK_STREAM, IPPROTO_TCP, (struct sockaddr *)&ipv6_any_addr, sizeof(ipv6_any_addr), "IPv6 any TCP variant 2"},
    {SOCK_STREAM, IPPROTO_SCTP, (struct sockaddr *)&ipv6_any_addr, sizeof(ipv6_any_addr), "IPv6 any SCTP"},
};

// 传给客户端线程的参数。
struct peer_args {
    struct bind_test_case tc;
};

// 客户端线程连接服务端，读取请求索引，再返回对应描述字符串。
static void *peer_thread(void *arg)
{
    // 取出连接地址和协议参数。
    struct peer_args *args = (struct peer_args *)arg;
    const struct bind_test_case *tc = &args->tc;

    // 创建客户端 socket。
    int sock = socket(tc->address->sa_family, tc->type, tc->protocol);
    if (sock < 0)
        return NULL;
    simple_set_socket_timeout(sock);

    // 连接到服务端地址。
    if (connect(sock, tc->address, tc->addrlen) != 0) {
        close(sock);
        return NULL;
    }

    // 读取服务端发送的随机用例索引。
    unsigned int request;
    if (simple_read_full(sock, &request, sizeof(request)) != 0) {
        close(sock);
        return NULL;
    }

    // 根据索引选择响应字符串。
    const char *response;
    if (request < SIMPLE_ARRAY_SIZE(testcase_list))
        response = testcase_list[request].description;
    else
        response = "Invalid request value";

    // 发送 NUL 结尾的响应字符串。
    simple_write_full(sock, response, strlen(response) + 1);
    close(sock);
    return NULL;
}

// 初始化 IPv4/IPv6 测试地址。
static void setup_addresses(void)
{
    // IPv4 loopback 与 wildcard 地址都使用端口 0。
    init_ipv4(&ipv4_addr, IPV4_ADDRESS, 0);
    init_ipv4(&ipv4_any_addr, NULL, 0);

    // IPv6 loopback 与 wildcard 地址都使用端口 0。
    init_ipv6(&ipv6_addr, &in6addr_loopback, 0);
    init_ipv6(&ipv6_any_addr, &in6addr_any, 0);
}

// 执行一个 stream bind 通信场景。
static int run_case(size_t n)
{
    // 获取当前测试用例并输出原测例信息。
    struct bind_test_case *tc = &testcase_list[n];
    printf("Testing %s\n", tc->description);

    // 非 Linux 系统不支持 abstract UNIX socket，直接跳过该环境专属场景。
    if (simple_is_abstract_unix(tc->address) && !simple_abstract_unix_supported()) {
        fprintf(stderr, "SKIP: socket for %s failed: Operation not supported\n",
                tc->description);
        return 0;
    }

    // 创建监听 socket；不支持的协议组合跳过。
    int listen_sock = socket(tc->address->sa_family, tc->type, tc->protocol);
    if (listen_sock < 0) {
        fprintf(stderr, "SKIP: socket for %s failed: %s\n", tc->description, strerror(errno));
        return 0;
    }
    simple_set_socket_timeout(listen_sock);

    // 绑定监听地址。
    if (bind(listen_sock, tc->address, tc->addrlen) != 0) {
        fprintf(stderr, "bind(): %s\n", strerror(errno));
        close(listen_sock);
        return 1;
    }

    // 解析可连接地址，wildcard 地址会被转换为 loopback。
    struct sockaddr_storage listen_addr;
    struct bind_test_case tc_copy = *tc;
    tc_copy.addrlen = get_connect_address(listen_sock, &listen_addr);
    tc_copy.address = (struct sockaddr *)&listen_addr;

    // 开始监听并启动客户端线程。
    if (listen(listen_sock, 1) != 0) {
        fprintf(stderr, "listen failed: %s\n", strerror(errno));
        close(listen_sock);
        return 1;
    }
    pthread_t thread_id;
    struct peer_args args = {.tc = tc_copy};
    if (pthread_create(&thread_id, NULL, peer_thread, &args) != 0) {
        fprintf(stderr, "pthread_create failed\n");
        close(listen_sock);
        return 1;
    }

    // 接受客户端连接。
    struct sockaddr_storage remote_addr;
    socklen_t remote_len = sizeof(remote_addr);
    int sock = accept(listen_sock, (struct sockaddr *)&remote_addr, &remote_len);
    if (sock < 0) {
        fprintf(stderr, "accept failed: %s\n", strerror(errno));
        pthread_join(thread_id, NULL);
        close(listen_sock);
        return 1;
    }
    simple_set_socket_timeout(sock);

    // 发送一个固定请求索引，避免随机性影响可读性。
    unsigned int request = (unsigned int)(n % SIMPLE_ARRAY_SIZE(testcase_list));
    if (simple_write_full(sock, &request, sizeof(request)) != 0) {
        fprintf(stderr, "write request failed: %s\n", strerror(errno));
        close(sock);
        pthread_join(thread_id, NULL);
        close(listen_sock);
        return 1;
    }

    // 读取客户端返回的描述字符串。
    char buffer[BUFFER_SIZE];
    ssize_t size = read(sock, buffer, BUFFER_SIZE - 1);
    if (size < 0) {
        fprintf(stderr, "read response failed: %s\n", strerror(errno));
        close(sock);
        pthread_join(thread_id, NULL);
        close(listen_sock);
        return 1;
    }
    buffer[size] = '\0';
    const char *exp_data = testcase_list[request].description;

    // 比较响应内容，并保留原输出文本。
    int failed = 0;
    if (!strcmp(buffer, exp_data))
        printf("Communication successful\n");
    else {
        fprintf(stderr, "Received invalid data. Expected: \"%s\". Received: \"%s\"\n",
                exp_data, buffer);
        failed = 1;
    }

    // 清理连接、监听 socket 和 UNIX pathname socket 文件。
    close(sock);
    close(listen_sock);
    pthread_join(thread_id, NULL);
    if (tc->address->sa_family == AF_UNIX) {
        struct sockaddr_un *tmp_addr = (struct sockaddr_un *)tc->address;
        if (tmp_addr->sun_path[0])
            unlink(tmp_addr->sun_path);
    }
    return failed;
}

// 入口函数遍历所有 stream bind 通信场景。
int main(void)
{
    // 准备临时目录和地址。
    simple_enter_temp_dir(temp_dir);
    setup_addresses();

    // 逐项运行。
    int failed = 0;
    for (size_t i = 0; i < SIMPLE_ARRAY_SIZE(testcase_list); i++) {
        if (run_case(i) != 0)
            failed = 1;
    }

    // 清理临时目录。
    simple_leave_temp_dir(temp_dir);
    return failed ? 1 : 0;
}
