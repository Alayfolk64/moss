// 本测例验证已经 mmap 的文件在被 ftruncate 缩小、扩大或截断为 0 时，ftruncate 调用本身都能成功完成。

#include "mmap_simple_common.h"

// 原测例的文件大小为 64KB。
#define MAPSIZE (64 * 1024)

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap09_XXXXXX";

// 单个 ftruncate 场景，保存新大小和原始描述文本。
struct test_case {
    off_t newsize;
    const char *desc;
};

// 原测例中的三个 ftruncate 场景。
static struct test_case tcases[] = {
    {MAPSIZE - 8192, "ftruncate mmaped file to a smaller size"},
    {MAPSIZE + 1024, "ftruncate mmaped file to a larger size"},
    {0, "ftruncate mmaped file to 0 size"},
};

// 入口函数准备映射文件并逐项执行 ftruncate。
int main(void)
{
    // 创建临时目录和测试文件。
    simple_enter_temp_dir(temp_dir);
    int fd = open("mmaptest", O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (fd < 0)
        simple_die_errno("open /tmp/mmaptest");

    // 将文件扩展到初始大小。
    if (ftruncate(fd, MAPSIZE) != 0)
        simple_die_errno("ftruncate");

    // 映射文件并用 A 填充，保持原测例的 mmaped file 状态。
    char *maddr = mmap(0, MAPSIZE, PROT_READ | PROT_WRITE,
                       MAP_FILE | MAP_SHARED, fd, 0);
    if (maddr == MAP_FAILED)
        simple_die_errno("mmap");
    memset(maddr, 'A', MAPSIZE);

    // 逐个执行 ftruncate 场景。
    int failed = 0;
    for (size_t i = 0; i < SIMPLE_ARRAY_SIZE(tcases); i++) {
        if (ftruncate(fd, tcases[i].newsize) == 0) {
            printf("%s\n", tcases[i].desc);
        } else {
            fprintf(stderr, "%s: %s\n", tcases[i].desc, strerror(errno));
            failed = 1;
        }
    }

    // 清理映射、文件和临时目录。
    munmap(maddr, MAPSIZE);
    close(fd);
    unlink("mmaptest");
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
