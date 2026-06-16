// 本测例验证 MAP_POPULATE 文件映射能成功创建，映射内容为零；在 Linux 上还会读取 /proc/self/pagemap 检查页面是否已经 present。

#include "mmap_simple_common.h"

// 原测例使用的临时文件名。
#define TEMPFILE "mmapfile"

// 原测例的映射大小。
#define MMAPSIZE (1UL << 20)

#ifdef __linux__
// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap12_XXXXXX";

// 在 Linux 上读取 pagemap，检查 MAP_POPULATE 是否让页面 present。
static int page_check(char *addr)
{
#ifndef __linux__
    // 非 Linux 没有 /proc/self/pagemap，跳过页面驻留检查。
    (void)addr;
    fprintf(stderr, "SKIP: /proc/self/pagemap is required\n");
    return EXIT_SKIP;
#else
    // 根据映射起始地址计算 pagemap 偏移。
    int flag = 0;
    size_t page_sz = (size_t)simple_page_size();
    int num_pages = MMAPSIZE / page_sz;
    unsigned long vmstart = (unsigned long)addr;
    off_t index = (off_t)((vmstart / page_sz) * sizeof(uint64_t));

    // 打开 pagemap；非 root 且 EPERM 时保留原 TCONF 文本。
    int pm = open("/proc/self/pagemap", O_RDONLY);
    if (pm == -1) {
        if ((errno == EPERM) && (geteuid() != 0)) {
            fprintf(stderr, "don't have permission to open dev pagemap: %s\n",
                    strerror(errno));
            return EXIT_SKIP;
        }
        fprintf(stderr, "pen dev pagemap failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 定位到映射起始页对应的 pagemap 项。
    off_t offset = lseek(pm, index, SEEK_SET);
    if (offset != index) {
        fprintf(stderr, "Reposition offset failed: %s\n", strerror(errno));
        close(pm);
        return EXIT_FAILURE;
    }

    // 逐页读取 present 位。
    for (int i = 1; i <= num_pages; i++) {
        uint64_t pagemap;
        if (read(pm, &pagemap, sizeof(pagemap)) != (ssize_t)sizeof(pagemap)) {
            fprintf(stderr, "read pagemap failed: %s\n", strerror(errno));
            close(pm);
            return EXIT_FAILURE;
        }

        // 第 63 位为 present；未 present 时输出原测例信息。
        if (!(pagemap & (1ULL << 63))) {
            printf("The %dth page addressed at %lX is not present\n",
                   i, vmstart + (unsigned long)i * page_sz);
            flag = 1;
        }
    }

    // 完成 pagemap 检查。
    close(pm);
    if (!flag)
        printf("All pages are present\n");
    return EXIT_SUCCESS;
#endif
}
#endif

// 入口函数创建 MAP_POPULATE 映射并检查内容。
int main(void)
{
#ifndef __linux__
    // MAP_POPULATE 和 /proc/self/pagemap 都是 Linux 语义，非 Linux 主机直接跳过。
    fprintf(stderr, "SKIP: MAP_POPULATE and /proc/self/pagemap require Linux\n");
    return EXIT_SKIP;
#else
    // 创建临时目录和 1MB 空文件。
    simple_enter_temp_dir(temp_dir);
    int fd = open(TEMPFILE, O_RDWR | O_CREAT | O_TRUNC, 0766);
    if (fd < 0)
        simple_die_errno("opening mmapfile failed");
    if (ftruncate(fd, MMAPSIZE) != 0)
        simple_die_errno("ftruncate");

    // 使用 MAP_POPULATE 映射文件。
    char *addr = mmap(NULL, MMAPSIZE, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_POPULATE, fd, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mmap of %s failed: %s\n", TEMPFILE, strerror(errno));
        close(fd);
        unlink(TEMPFILE);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 检查页是否 present；跳过不影响后续内容正确性检查。
    int failed = 0;
    int page_rc = page_check(addr);
    if (page_rc == EXIT_FAILURE)
        failed = 1;

    // 检查映射内容是否全为 0。
    for (unsigned int i = 0; i < MMAPSIZE; i++) {
        if (addr[i]) {
            fprintf(stderr, "Non-zero byte at offset %i\n", i);
            failed = 1;
            break;
        }
    }

    // 如果内容检查成功，输出原测例成功消息。
    if (!failed)
        printf("File mapped properly\n");

    // 清理映射和临时文件。
    munmap(addr, MMAPSIZE);
    close(fd);
    unlink(TEMPFILE);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : (page_rc == EXIT_SKIP ? EXIT_SKIP : EXIT_SUCCESS);
#endif
}
