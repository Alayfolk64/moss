// 本测例验证 MAP_LOCKED 匿名映射成功后，/proc/self/status 中 VmLck 增量应等于映射大小；运行需要 Linux /proc 和足够的锁页权限，原测例要求 root。

#include "mmap_simple_common.h"

// 原测例使用的文件名文本，虽然实际映射是匿名映射，失败消息仍保留它。
#define TEMPFILE "mmapfile"

// 原测例映射大小为 1MB。
#define MMAPSIZE (1UL << 20)

// /proc/self/status 单行读取缓冲区长度。
#define LINELEN 256

// 从 /proc/self/status 读取 VmLck，单位为 KB。
static int getvmlck(unsigned int *lock_sz)
{
#ifndef __linux__
    // 非 Linux 没有 /proc/self/status，无法复现原测例。
    (void)lock_sz;
    fprintf(stderr, "SKIP: /proc/self/status is required\n");
    return EXIT_SKIP;
#else
    // 打开进程状态文件。
    FILE *fstatus = fopen("/proc/self/status", "r");
    if (fstatus == NULL) {
        fprintf(stderr, "Open dev status failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 查找 VmLck 行。
    char line[LINELEN] = {0};
    while (fgets(line, LINELEN, fstatus) != NULL) {
        if (strstr(line, "VmLck") != NULL)
            break;
    }

    // 从 VmLck 行解析锁定大小。
    int ret = sscanf(line, "%*[^0-9]%u%*[^0-9]", lock_sz);
    fclose(fstatus);
    if (ret != 1) {
        fprintf(stderr, "Get lock size failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    return EXIT_SUCCESS;
#endif
}

// 入口函数创建 MAP_LOCKED 映射并比较 VmLck 变化。
int main(void)
{
    // 原测例要求 root；非 root 通常会受到 memlock 限制。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: root privileges are required\n");
        return EXIT_SKIP;
    }

    // 获取映射前的锁页大小。
    unsigned int sz_before;
    int rc = getvmlck(&sz_before);
    if (rc != EXIT_SUCCESS)
        return rc;

    // 创建 MAP_LOCKED 匿名映射。
    char *addr = mmap(NULL, MMAPSIZE, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_LOCKED | MAP_ANONYMOUS, -1, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mmap of %s failed: %s\n", TEMPFILE, strerror(errno));
        return EXIT_FAILURE;
    }

    // 获取映射后的锁页大小。
    unsigned int sz_after;
    rc = getvmlck(&sz_after);
    if (rc != EXIT_SUCCESS) {
        munmap(addr, MMAPSIZE);
        return rc;
    }

    // 检查 VmLck 增量是否正好等于映射大小。
    unsigned int sz_ch = sz_after - sz_before;
    int failed = 0;
    if (sz_ch == MMAPSIZE / 1024) {
        printf("Functionality of mmap() successful\n");
    } else {
        fprintf(stderr, "Expected %luK locked, get %uK locked\n",
                MMAPSIZE / 1024, sz_ch);
        failed = 1;
    }

    // 解除映射，并保留原 munmap 失败文本。
    if (munmap(addr, MMAPSIZE) != 0) {
        fprintf(stderr, "munmap failed: %s\n", strerror(errno));
        failed = 1;
    }

    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
