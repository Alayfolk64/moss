// 本测例验证 mmap 使用 MAP_SHARED_VALIDATE 搭配未知 flag 时应失败，并将 errno 设置为 EOPNOTSUPP。

#include "mmap_simple_common.h"

// 原测例使用的临时文件名。
#define TEST_FILE "file_to_mmap"

// 原测例的测试文件大小。
#define TEST_FILE_SIZE 1024

// 原测例用于触发 MAP_SHARED_VALIDATE 严格校验的非法 flag。
#define INVALID_FLAG (1 << 10)

#ifdef __linux__
// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap20_XXXXXX";
#endif

// 入口函数创建测试文件并执行 MAP_SHARED_VALIDATE 失败检查。
int main(void)
{
#ifndef __linux__
    // MAP_SHARED_VALIDATE 是 Linux 语义，非 Linux 主机跳过。
    fprintf(stderr, "SKIP: MAP_SHARED_VALIDATE requires Linux\n");
    return EXIT_SKIP;
#else
    // 创建临时目录和测试文件。
    simple_enter_temp_dir(temp_dir);
    int fd = open(TEST_FILE, O_CREAT | O_RDWR | O_TRUNC, 0600);
    if (fd < 0)
        simple_die_errno("open");

    // 填充测试文件，保持原测例的文件大小和内容。
    if (simple_fill_fd(fd, 'a', TEST_FILE_SIZE) != 0) {
        fprintf(stderr, "Could not fill the testfile\n");
        close(fd);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 携带非法 flag 和 MAP_SHARED_VALIDATE 调用 mmap，预期失败。
    errno = 0;
    void *addr = mmap(NULL, TEST_FILE_SIZE, PROT_READ | PROT_WRITE,
                      INVALID_FLAG | MAP_SHARED_VALIDATE, fd, 0);
    int saved_errno = errno;

    // 检查 EOPNOTSUPP，并保留原测例输出文本。
    int failed = 0;
    if (addr != MAP_FAILED) {
        fprintf(stderr, "mmap() is successful, but it should have failed: %s\n",
                strerror(saved_errno));
        munmap(addr, TEST_FILE_SIZE);
        failed = 1;
    } else if (saved_errno == EOPNOTSUPP) {
        printf("mmap() failed with errno set to EOPNOTSUPP\n");
    } else {
        fprintf(stderr, "mmap() failed with unexpected error: %s\n",
                strerror(saved_errno));
        failed = 1;
    }

    // 清理文件和临时目录。
    close(fd);
    unlink(TEST_FILE);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
