// 本测例验证多种标准 datagram 协议的 socket 可以 bind，并通过 connect/send/recv 完成一次简单请求/响应通信。

#include "bind_simple_common.h"

// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/bind05_XXXXXX";

// UNIX pathname datagram 地址。
static struct sockaddr_un unix_addr = {
    .sun_family = AF_UNIX,
    .sun_path = MAIN_SOCKET_FILE,
};

// UNIX abstract datagram 地址。
static struct sockaddr_un abstract_addr = {
    .sun_family = AF_UNIX,
    .sun_path = ABSTRACT_SOCKET_PATH,
};

// UNIX datagram peer 地址。
static struct sockaddr_un peer_addr = {
    .sun_family = AF_UNIX,
    .sun_path = PEER_SOCKET_FILE,
};

// IPv4 loopback 地址。
static struct sockaddr_in ipv4_addr;

// IPv4 wildcard 地址。
static struct sockaddr_in ipv4_any_addr;

// IPv6 loopback 地址。
static struct sockaddr_in6 ipv6_addr;

// IPv6 wildcard 地址。
static struct sockaddr_in6 ipv6_any_addr;

// 原 bind05 覆盖的 datagram 协议列表。
static struct bind_test_case testcase_list[] = {
    {SOCK_DGRAM, 0, (struct sockaddr *)&unix_addr, sizeof(unix_addr), "AF_UNIX pathname datagram"},
    {SOCK_DGRAM, 0, (struct sockaddr *)&abstract_addr, sizeof(abstract_addr), "AF_UNIX abstract datagram"},
    {SOCK_DGRAM, 0, (struct sockaddr *)&ipv4_addr, sizeof(ipv4_addr), "IPv4 loop UDP variant 1"},
    {SOCK_DGRAM, IPPROTO_UDP, (struct sockaddr *)&ipv4_addr, sizeof(ipv4_addr), "IPv4 loop UDP variant 2"},
    {SOCK_DGRAM, IPPROTO_UDPLITE, (struct sockaddr *)&ipv4_addr, sizeof(ipv4_addr), "IPv4 loop UDP-Lite"},
    {SOCK_DGRAM, 0, (struct sockaddr *)&ipv4_any_addr, sizeof(ipv4_any_addr), "IPv4 any UDP variant 1"},
    {SOCK_DGRAM, IPPROTO_UDP, (struct sockaddr *)&ipv4_any_addr, sizeof(ipv4_any_addr), "IPv4 any UDP variant 2"},
    {SOCK_DGRAM, IPPROTO_UDPLITE, (struct sockaddr *)&ipv4_any_addr, sizeof(ipv4_any_addr), "IPv4 any UDP-Lite"},
    {SOCK_DGRAM, 0, (struct sockaddr *)&ipv6_addr, sizeof(ipv6_addr), "IPv6 loop UDP variant 1"},
    {SOCK_DGRAM, IPPROTO_UDP, (struct sockaddr *)&ipv6_addr, sizeof(ipv6_addr), "IPv6 loop UDP variant 2"},
    {SOCK_DGRAM, IPPROTO_UDPLITE, (struct sockaddr *)&ipv6_addr, sizeof(ipv6_addr), "IPv6 loop UDP-Lite"},
    {SOCK_DGRAM, 0, (struct sockaddr *)&ipv6_any_addr, sizeof(ipv6_any_addr), "IPv6 any UDP variant 1"},
    {SOCK_DGRAM, IPPROTO_UDP, (struct sockaddr *)&ipv6_any_addr, sizeof(ipv6_any_addr), "IPv6 any UDP variant 2"},
    {SOCK_DGRAM, IPPROTO_UDPLITE, (struct sockaddr *)&ipv6_any_addr, sizeof(ipv6_any_addr), "IPv6 any UDP-Lite"},
};

// 传给 datagram peer 线程的参数。
struct peer_args {
    struct bind_test_case tc;
};

// datagram peer 线程负责发送初始请求、读取索引、返回描述字符串。
static void *peer_thread(void *arg)
{
    // 取出连接地址和协议参数。
    struct peer_args *args = (struct peer_args *)arg;
    const struct bind_test_case *tc = &args->tc;

    // 创建 peer socket。
    int sock = socket(tc->address->sa_family, tc->type, tc->protocol);
    if (sock < 0)
        return NULL;
    simple_set_socket_timeout(sock);

    // AF_UNIX datagram 双向通信要求 peer 也绑定一个路径。
    if (tc->address->sa_family == AF_UNIX) {
        unlink(PEER_SOCKET_FILE);
        if (bind(sock, (struct sockaddr *)&peer_addr, sizeof(peer_addr)) != 0) {
            close(sock);
            return NULL;
        }
    }

    // 连接服务端地址。
    if (connect(sock, tc->address, tc->addrlen) != 0) {
        close(sock);
        return NULL;
    }

    // 先发送一个占位请求，让服务端得到 peer 地址。
    unsigned int request = 0;
    if (simple_write_full(sock, &request, sizeof(request)) != 0) {
        close(sock);
        return NULL;
    }

    // 读取服务端返回的请求索引。
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

    // 发送响应。
    simple_write_full(sock, response, strlen(response) + 1);
    close(sock);
    if (tc->address->sa_family == AF_UNIX)
        unlink(PEER_SOCKET_FILE);
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

// 执行单个 datagram bind 通信场景。
static int run_case(size_t n)
{
    // 获取当前用例并输出原测例信息。
    struct bind_test_case *tc = &testcase_list[n];
    printf("Testing %s\n", tc->description);

    // 非 Linux 系统不支持 abstract UNIX socket，直接跳过该环境专属场景。
    if (simple_is_abstract_unix(tc->address) && !simple_abstract_unix_supported()) {
        fprintf(stderr, "SKIP: socket for %s failed: Operation not supported\n",
                tc->description);
        return 0;
    }

    // 创建服务端 datagram socket，不支持的协议组合跳过。
    int sock = socket(tc->address->sa_family, tc->type, tc->protocol);
    if (sock < 0) {
        fprintf(stderr, "SKIP: socket for %s failed: %s\n", tc->description, strerror(errno));
        return 0;
    }
    simple_set_socket_timeout(sock);

    // 绑定服务端地址。
    if (bind(sock, tc->address, tc->addrlen) != 0) {
        fprintf(stderr, "bind(): %s\n", strerror(errno));
        close(sock);
        return 1;
    }

    // 解析可连接地址，wildcard 地址会被转换为 loopback。
    struct sockaddr_storage listen_addr;
    struct bind_test_case tc_copy = *tc;
    tc_copy.addrlen = get_connect_address(sock, &listen_addr);
    tc_copy.address = (struct sockaddr *)&listen_addr;

    // 启动 peer 线程。
    pthread_t thread_id;
    struct peer_args args = {.tc = tc_copy};
    if (pthread_create(&thread_id, NULL, peer_thread, &args) != 0) {
        fprintf(stderr, "pthread_create failed\n");
        close(sock);
        return 1;
    }

    // 接收 peer 初始请求并记录 peer 地址。
    struct sockaddr_storage remote_addr;
    socklen_t remote_len = sizeof(remote_addr);
    unsigned int rand_index;
    ssize_t size = recvfrom(sock, &rand_index, sizeof(rand_index), 0,
                            (struct sockaddr *)&remote_addr, &remote_len);
    if (size != (ssize_t)sizeof(rand_index)) {
        fprintf(stderr, "Error while waiting for connection: %s\n", strerror(errno));
        close(sock);
        pthread_join(thread_id, NULL);
        return 1;
    }

    // 发送固定请求索引，避免随机性影响可读性。
    rand_index = (unsigned int)(n % SIMPLE_ARRAY_SIZE(testcase_list));
    if (sendto(sock, &rand_index, sizeof(rand_index), 0,
               (struct sockaddr *)&remote_addr, remote_len) != (ssize_t)sizeof(rand_index)) {
        fprintf(stderr, "sendto failed: %s\n", strerror(errno));
        close(sock);
        pthread_join(thread_id, NULL);
        return 1;
    }

    // 读取 peer 响应字符串。
    char buffer[BUFFER_SIZE];
    size = read(sock, buffer, BUFFER_SIZE - 1);
    if (size < 0) {
        fprintf(stderr, "read response failed: %s\n", strerror(errno));
        close(sock);
        pthread_join(thread_id, NULL);
        return 1;
    }
    buffer[size] = '\0';
    const char *exp_data = testcase_list[rand_index].description;

    // 比较响应内容，并保留原输出文本。
    int failed = 0;
    if (!strcmp(buffer, exp_data))
        printf("Communication successful\n");
    else {
        fprintf(stderr, "Received invalid data. Expected: \"%s\". Received: \"%s\"\n",
                exp_data, buffer);
        failed = 1;
    }

    // 清理 socket、线程和 UNIX pathname socket 文件。
    close(sock);
    pthread_join(thread_id, NULL);
    if (tc->address->sa_family == AF_UNIX) {
        struct sockaddr_un *tmp_addr = (struct sockaddr_un *)tc->address;
        if (tmp_addr->sun_path[0])
            unlink(tmp_addr->sun_path);
    }
    return failed;
}

// 入口函数遍历所有 datagram bind 通信场景。
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
