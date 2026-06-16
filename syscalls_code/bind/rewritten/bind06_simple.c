// 本测例用有边界的线程循环模拟原 bind06 对 AF_PACKET socket 的 bind()/网卡状态切换竞态检查，确认不会触发 CVE-2018-18559 相关异常。

#include "bind_simple_common.h"

#ifdef __linux__

#include <linux/if_packet.h>
#include <net/if.h>
#include <sched.h>
#include <sys/ioctl.h>

// 竞态循环次数，替代 LTP 的 fuzzy sync 运行框架。
#define RACE_LOOPS 10000

// 主线程和 bind 线程共享的 packet socket fd。
static volatile int shared_fd = -1;

// 通知 bind 线程停止的标志。
static volatile int stop_thread;

// 绑定到 loopback ifindex 的 AF_PACKET 地址。
static struct sockaddr_ll addr1;

// 不带 ifindex 的 AF_PACKET 地址，用于第二次 bind。
static struct sockaddr_ll addr2;

// 在独立网络命名空间中运行，避免修改宿主 loopback 状态。
static int enter_private_netns(void)
{
    // 没有权限创建网络命名空间时跳过，避免在宿主网络上做破坏性操作。
    if (unshare(CLONE_NEWNET) != 0) {
        fprintf(stderr, "SKIP: bind06 requires a private network namespace: %s\n",
                strerror(errno));
        return EXIT_SKIP;
    }

    return 0;
}

// 准备 AF_PACKET 地址中的 loopback ifindex。
static int setup_packet_addresses(void)
{
    // 创建一个临时 AF_PACKET socket 用来查询 lo 的 ifindex。
    int fd = socket(AF_PACKET, SOCK_DGRAM, PF_PACKET);
    if (fd < 0) {
        fprintf(stderr, "SKIP: AF_PACKET socket is not available: %s\n",
                strerror(errno));
        return EXIT_SKIP;
    }

    // 查询 loopback 的接口索引。
    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    strncpy(ifr.ifr_name, "lo", sizeof(ifr.ifr_name) - 1);
    if (ioctl(fd, SIOCGIFINDEX, &ifr) != 0) {
        fprintf(stderr, "SIOCGIFINDEX failed: %s\n", strerror(errno));
        close(fd);
        return EXIT_FAILURE;
    }
    close(fd);

    // 填充两个 bind() 使用的 sockaddr_ll。
    memset(&addr1, 0, sizeof(addr1));
    memset(&addr2, 0, sizeof(addr2));
    addr1.sll_family = AF_PACKET;
    addr1.sll_ifindex = ifr.ifr_ifindex;
    addr2.sll_family = AF_PACKET;
    return 0;
}

// 对共享 packet socket 连续执行两次 bind()。
static void do_bind_pair(int fd)
{
    // fd 可能刚被主线程关闭，失败属于竞态窗口的一部分。
    if (fd < 0)
        return;

    // 第一轮绑定到 loopback ifindex。
    bind(fd, (struct sockaddr *)&addr1, sizeof(addr1));

    // 第二轮绑定到不指定 ifindex 的地址。
    bind(fd, (struct sockaddr *)&addr2, sizeof(addr2));
}

// bind 线程反复在主线程暴露的 fd 上执行 bind()。
static void *bind_thread(void *arg)
{
    // 不使用 arg，只通过共享 fd 和停止标志协作。
    (void)arg;

    // 有边界循环避免测例永久运行。
    for (int i = 0; i < RACE_LOOPS && !stop_thread; i++) {
        int fd = shared_fd;
        do_bind_pair(fd);
        sched_yield();
    }

    return NULL;
}

// 把 loopback 接口设置为指定 flags。
static int set_loopback_flags(int fd, short flags)
{
    // 构造 ioctl 参数，只操作当前私有网络命名空间里的 lo。
    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    strncpy(ifr.ifr_name, "lo", sizeof(ifr.ifr_name) - 1);
    ifr.ifr_flags = flags;

    // 权限不足时由调用者决定是否跳过。
    return ioctl(fd, SIOCSIFFLAGS, &ifr);
}

// 执行 AF_PACKET bind 竞态测例。
static int run_test(void)
{
    // 启动 bind 线程。
    pthread_t tid;
    stop_thread = 0;
    if (pthread_create(&tid, NULL, bind_thread, NULL) != 0) {
        fprintf(stderr, "pthread_create failed\n");
        return EXIT_FAILURE;
    }

    // 主线程循环创建 socket，并在 bind 线程竞争时切换 lo 状态。
    int skipped = 0;
    for (int i = 0; i < RACE_LOOPS; i++) {
        int fd = socket(AF_PACKET, SOCK_DGRAM, PF_PACKET);
        if (fd < 0) {
            skipped = EXIT_SKIP;
            break;
        }

        shared_fd = fd;
        if (set_loopback_flags(fd, 0) != 0) {
            fprintf(stderr, "SKIP: SIOCSIFFLAGS requires CAP_NET_ADMIN: %s\n",
                    strerror(errno));
            skipped = EXIT_SKIP;
            close(fd);
            shared_fd = -1;
            break;
        }

        set_loopback_flags(fd, IFF_UP);
        shared_fd = -1;
        close(fd);
    }

    // 停止 bind 线程并等待退出。
    stop_thread = 1;
    pthread_join(tid, NULL);

    // 权限或能力不足时按 skip 返回。
    if (skipped)
        return skipped;

    // 保留原测例最终成功输出。
    printf("Nothing bad happened (yet)\n");
    return 0;
}

// 入口函数先隔离网络命名空间，再执行竞态检查。
int main(void)
{
    // 进入私有网络命名空间，失败时跳过。
    int ret = enter_private_netns();
    if (ret != 0)
        return ret;

    // 准备 packet 地址。
    ret = setup_packet_addresses();
    if (ret != 0)
        return ret;

    // 执行测试主体。
    return run_test();
}

#else

// 非 Linux 系统没有 AF_PACKET，保留一个可编译的跳过入口。
int main(void)
{
    // bind06 是 Linux AF_PACKET 专属测例。
    fprintf(stderr, "SKIP: bind06 requires Linux AF_PACKET support\n");
    return EXIT_SKIP;
}

#endif
