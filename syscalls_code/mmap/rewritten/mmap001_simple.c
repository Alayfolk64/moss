// 本测例验证 mmap 可以映射一个较大的文件、逐字节写入映射区域、msync 同步并 munmap；可用 -m 指定映射页数，默认 1000 页。

#include "mmap_simple_common.h"

// 默认映射页数，来自原 mmap001 测例。
#define DEFAULT_PAGES 1000U

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap001_XXXXXX";

// 解析 -m 参数并返回要映射的页数。
static unsigned int parse_pages(int argc, char **argv)
{
    // 默认使用原测例的 1000 页。
    unsigned int pages = DEFAULT_PAGES;

    // 逐项扫描命令行，只保留原测例的 -m x 选项。
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-m") && i + 1 < argc) {
            char *end = NULL;
            unsigned long value = strtoul(argv[++i], &end, 10);

            // 非正数或非法字符串保持原测例的错误信息。
            if (value < 1 || !end || *end != '\0') {
                fprintf(stderr, "Invalid arg for -m: %s\n", argv[i]);
                exit(EXIT_FAILURE);
            }

            pages = (unsigned int)value;
        } else if (!strcmp(argv[i], "-h") || !strcmp(argv[i], "--help")) {
            printf("  -m x    size of mmap in pages (default 1000)\n");
            exit(EXIT_SUCCESS);
        }
    }

    return pages;
}

// 清理临时文件和临时目录。
static void cleanup(const char *filename)
{
    // 删除测试文件，失败时忽略，便于处理部分失败路径。
    if (filename)
        unlink(filename);

    // 删除临时目录。
    simple_leave_temp_dir(temp_dir);
}

// 入口函数按原测例顺序执行 open、lseek、write、mmap、写内存、msync 和 munmap。
int main(int argc, char **argv)
{
    // 根据命令行和页大小计算映射长度。
    unsigned int pages = parse_pages(argc, argv);
    long page_size = simple_page_size();
    size_t memsize = (size_t)pages * (size_t)page_size;

    // 进入临时目录并生成和原测例相同风格的测试文件名。
    simple_enter_temp_dir(temp_dir);
    char filename[64];
    snprintf(filename, sizeof(filename), "testfile.%ld", (long)getpid());

    // 保留原测例的 mmap 大小提示。
    printf("mmap()ing file of %u pages or %zu bytes\n", pages, memsize);

    // 创建测试文件。
    int fd = open(filename, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (fd == -1) {
        fprintf(stderr, "opening %s failed: %s\n", filename, strerror(errno));
        cleanup(filename);
        return EXIT_FAILURE;
    }

    // 扩展文件到映射长度，模拟原测例 lseek 到 memsize 后写入一个 NUL 字节。
    if (lseek(fd, (off_t)memsize, SEEK_SET) != (off_t)memsize) {
        fprintf(stderr, "lseek failed: %s\n", strerror(errno));
        close(fd);
        cleanup(filename);
        return EXIT_FAILURE;
    }

    // 写入最后一个字节，使文件实际可映射。
    if (write(fd, "\0", 1) != 1) {
        fprintf(stderr, "writing to %s failed: %s\n", filename, strerror(errno));
        close(fd);
        cleanup(filename);
        return EXIT_FAILURE;
    }

    // 以 MAP_SHARED + PROT_WRITE 映射整个文件。
    char *array = mmap(0, memsize, PROT_WRITE, MAP_SHARED, fd, 0);
    if (array == MAP_FAILED) {
        fprintf(stderr, "mmapping %s failed: %s\n", filename, strerror(errno));
        close(fd);
        cleanup(filename);
        return EXIT_FAILURE;
    }
    printf("mmap() completed successfully.\n");

    // 保留原测例的触摸映射内存提示。
    printf("touching mmaped memory\n");

    // 逐字节写入映射区域；如果映射无效，通常会在这里触发崩溃。
    for (size_t i = 0; i < memsize; i++)
        array[i] = (char)i;
    printf("we're still here, mmaped area must be good\n");

    // 同步映射页，并按原消息报告结果。
    int failed = 0;
    if (msync(array, memsize, MS_SYNC) == -1) {
        fprintf(stderr, "synchronizing mmapped page failed: %s\n", strerror(errno));
        failed = 1;
    } else {
        printf("synchronizing mmapped page passed\n");
    }

    // 解除映射，并按原消息报告结果。
    if (munmap(array, memsize) == -1) {
        fprintf(stderr, "munmapping %s failed: %s\n", filename, strerror(errno));
        failed = 1;
    } else {
        printf("munmapping %s successful\n", filename);
    }

    // 关闭并删除测试文件。
    close(fd);
    cleanup(filename);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
