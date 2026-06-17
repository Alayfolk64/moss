// 本测例验证使用已经关闭的无效文件描述符进行文件映射时，mmap 会失败并返回 EBADF。

#include "mmap_simple_common.h"

// 原测例使用的临时文件名。
#define TEMPFILE "mmapfile"

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap08_XXXXXX";

// 入口函数准备一个失效 fd 并检查 mmap 的 EBADF。
int main(void)
{
    // 创建临时目录，并打开后立即关闭一个文件描述符。
    simple_enter_temp_dir(temp_dir);
    size_t page_sz = (size_t)simple_page_size();
    int fd = open(TEMPFILE, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (fd < 0)
        simple_die_errno("opening mmapfile failed");
    if (close(fd) != 0)
        simple_die_errno("closing mmapfile failed");

    // 使用已经关闭的 fd 调用 mmap，预期失败。
    errno = 0;
    void *addr = mmap(NULL, page_sz, PROT_WRITE, MAP_FILE | MAP_SHARED, fd, 0);
    int saved_errno = errno;

    // mmap 意外成功时解除映射并报告失败。
    int failed = 0;
    if (addr != MAP_FAILED) {
        fprintf(stderr, "mmap() succeeded unexpectedly\n");
        munmap(addr, page_sz);
        failed = 1;
    } else if (saved_errno == EBADF) {
        printf("mmap() failed as expected\n");
    } else {
        fprintf(stderr, "mmap() failed unexpectedly; expected: %d - %s: %s\n",
                EBADF, strerror(EBADF), strerror(saved_errno));
        failed = 1;
    }

    // 清理临时文件和目录。
    unlink(TEMPFILE);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
