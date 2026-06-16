// 本测例验证 mmap 成功后，/proc/self/maps 中对应映射的权限位和 mmap 的 prot/flags 参数一致；该检查依赖 Linux 的 /proc/self/maps。

#include "mmap_simple_common.h"
#include <inttypes.h>

#ifdef __linux__
// 单个权限组合测试用例，保存 mmap 参数和期望的 maps 权限字符串。
struct tcase {
    int prot;
    int flags;
    const char *exp_perms;
};

// 覆盖原测例列出的 PROT 和 MAP_PRIVATE/MAP_SHARED 组合。
static struct tcase tcases[] = {
    {PROT_NONE, MAP_ANONYMOUS | MAP_PRIVATE, "---p"},
    {PROT_NONE, MAP_ANONYMOUS | MAP_SHARED, "---s"},
    {PROT_READ, MAP_ANONYMOUS | MAP_PRIVATE, "r--p"},
    {PROT_READ, MAP_ANONYMOUS | MAP_SHARED, "r--s"},
    {PROT_WRITE, MAP_ANONYMOUS | MAP_PRIVATE, "-w-p"},
    {PROT_WRITE, MAP_ANONYMOUS | MAP_SHARED, "-w-s"},
    {PROT_READ | PROT_WRITE, MAP_ANONYMOUS | MAP_PRIVATE, "rw-p"},
    {PROT_READ | PROT_WRITE, MAP_ANONYMOUS | MAP_SHARED, "rw-s"},
    {PROT_READ | PROT_EXEC, MAP_ANONYMOUS | MAP_PRIVATE, "r-xp"},
    {PROT_READ | PROT_EXEC, MAP_ANONYMOUS | MAP_SHARED, "r-xs"},
    {PROT_WRITE | PROT_EXEC, MAP_ANONYMOUS | MAP_PRIVATE, "-wxp"},
    {PROT_WRITE | PROT_EXEC, MAP_ANONYMOUS | MAP_SHARED, "-wxs"},
    {PROT_READ | PROT_WRITE | PROT_EXEC, MAP_ANONYMOUS | MAP_PRIVATE, "rwxp"},
    {PROT_READ | PROT_WRITE | PROT_EXEC, MAP_ANONYMOUS | MAP_SHARED, "rwxs"},
};

// 在 /proc/self/maps 中查找目标地址所在行，并取出权限字段。
static int read_mapping_perms(void *addr, char perms[8])
{
#ifndef __linux__
    // 非 Linux 没有 /proc/self/maps，无法复现原测例。
    (void)addr;
    (void)perms;
    fprintf(stderr, "SKIP: /proc/self/maps is required\n");
    return EXIT_SKIP;
#else
    // 打开当前进程的 maps 文件。
    FILE *fp = fopen("/proc/self/maps", "r");
    if (!fp) {
        fprintf(stderr, "Open /proc/self/maps failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 逐行解析地址范围和权限字段。
    char line[1024];
    uintptr_t target = (uintptr_t)addr;
    while (fgets(line, sizeof(line), fp)) {
        uintptr_t start, end;
        char local_perms[8];
        if (sscanf(line, "%" SCNxPTR "-%" SCNxPTR " %7s", &start, &end, local_perms) == 3) {
            if (target >= start && target < end) {
                strcpy(perms, local_perms);
                fclose(fp);
                return 0;
            }
        }
    }

    // 没找到对应映射视为失败。
    fclose(fp);
    fprintf(stderr, "mapping permissions in /proc mismatched, expected: unknown, found: missing\n");
    return EXIT_FAILURE;
#endif
}

// 执行单个权限组合检查。
static int run_case(const struct tcase *tc)
{
    // 获取页大小，并先创建两页保护映射防止与已有映射合并。
    size_t pagesize = (size_t)simple_page_size();
    int guard_flag = (tc->flags & MAP_PRIVATE) ? MAP_SHARED : MAP_PRIVATE;
    char *addr1 = mmap(NULL, pagesize * 2, PROT_NONE,
                       MAP_ANONYMOUS | guard_flag, -1, 0);
    if (addr1 == MAP_FAILED)
        simple_die_errno("mmap");

    // 在第二页用待测权限和固定地址重新映射。
    char *addr2 = mmap(addr1 + pagesize, pagesize, tc->prot,
                       tc->flags | MAP_FIXED, -1, 0);
    if (addr2 == MAP_FAILED) {
        fprintf(stderr, "mmap: %s\n", strerror(errno));
        munmap(addr1, pagesize * 2);
        return EXIT_FAILURE;
    }

    // 从 /proc/self/maps 读取实际权限字段。
    char perms[8] = {0};
    int rc = read_mapping_perms(addr2, perms);
    if (rc == EXIT_SKIP || rc == EXIT_FAILURE) {
        munmap(addr1, pagesize * 2);
        return rc;
    }

    // 比较实际权限和期望权限，并保留原输出文本。
    int failed = 0;
    if (!strcmp(perms, tc->exp_perms)) {
        printf("mapping permissions in /proc matched: %s\n", perms);
    } else {
        fprintf(stderr, "mapping permissions in /proc mismatched, expected: %s, found: %s\n",
                tc->exp_perms, perms);
        failed = 1;
    }

    // 解除两页映射。
    munmap(addr1, pagesize * 2);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
#endif

// 入口函数遍历所有权限组合。
int main(void)
{
    // 非 Linux 主机直接跳过，因为缺少 /proc/self/maps。
#ifndef __linux__
    fprintf(stderr, "SKIP: /proc/self/maps is required\n");
    return EXIT_SKIP;
#else
    // 逐项运行权限映射测试。
    int failed = 0;
    for (size_t i = 0; i < SIMPLE_ARRAY_SIZE(tcases); i++) {
        int rc = run_case(&tcases[i]);
        if (rc == EXIT_SKIP)
            return EXIT_SKIP;
        if (rc != EXIT_SUCCESS)
            failed = 1;
    }

    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
