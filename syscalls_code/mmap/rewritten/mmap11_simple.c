// 本测例验证大量 detached 线程退出时，libc 在线程栈清理路径中调用的 munmap 不应因为 max_map_count 等原因异常失败。

#include "mmap_simple_common.h"

// 原测例中每个工作线程分配的内存大小。
#define MAL_SIZE (100 * 1024)

// 独立版中固定创建的线程轮数，替代 LTP 的 TEST_LOOPING。
#define THREAD_LOOPS 128

// 分配并触摸内存的线程函数。
static void *wait_thread(void *args)
{
    // 参数未使用，保留原函数接口。
    (void)args;

    // 分配一块内存并写入，制造普通线程退出时的清理路径。
    void *addr = malloc(MAL_SIZE);
    if (addr) {
        memset(addr, 1, MAL_SIZE);
        free(addr);
    }

    // 睡眠一小段时间，让 detached 线程数量有机会累积。
    sleep(1);
    return NULL;
}

// 空线程函数，用来覆盖原测例的第二类线程。
static void *wait_thread2(void *args)
{
    // 参数未使用，线程立即退出。
    (void)args;
    return NULL;
}

// 入口函数反复创建 detached 线程并检查 pthread_create 是否异常失败。
int main(void)
{
    // 原测例显式要求 root；这里保留该环境要求。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: root privileges are required\n");
        return EXIT_SKIP;
    }

    // 初始化 detached 线程属性。
    pthread_attr_t attr;
    int ret = pthread_attr_init(&attr);
    if (ret) {
        errno = ret;
        fprintf(stderr, "pthread_attr_init: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    ret = pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    if (ret) {
        errno = ret;
        fprintf(stderr, "pthread_attr_setdetachstate: %s\n", strerror(errno));
        pthread_attr_destroy(&attr);
        return EXIT_FAILURE;
    }

    // 创建两类线程，保留原测例在失败前输出 [count] 的行为。
    int count = 0;
    int failed = 0;
    for (int i = 0; i < THREAD_LOOPS; i++) {
        pthread_t th;
        ret = pthread_create(&th, &attr, wait_thread, NULL);
        if (ret) {
            errno = ret;
            printf("[%d] \n", count);
            fprintf(stderr, "pthread_create: %s\n", strerror(errno));
            failed = 1;
            break;
        }
        count++;

        ret = pthread_create(&th, &attr, wait_thread2, NULL);
        if (ret) {
            errno = ret;
            printf("[%d] \n", count);
            fprintf(stderr, "pthread_create: %s\n", strerror(errno));
            failed = 1;
            break;
        }
        count++;
    }

    // 销毁属性并等待 detached 线程自然退出。
    pthread_attr_destroy(&attr);
    sleep(2);

    // 成功完成时输出原测例消息。
    if (!failed)
        printf("test completed.\n");
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
