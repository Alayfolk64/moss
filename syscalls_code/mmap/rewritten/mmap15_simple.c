// 本测例验证普通页不能用 MAP_FIXED 映射到极高地址，mmap 应失败并返回 ENOMEM 或 EINVAL。

#include "mmap_simple_common.h"

// 原测例使用的临时文件名。
#define TEMPFILE "mmapfile"

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap15_XXXXXX";

// 入口函数尝试把文件页映射到高地址。
int main(void)
{
    // 准备临时目录、页大小和测试文件。
    simple_enter_temp_dir(temp_dir);
    long page_size = simple_page_size();
#ifdef __ia64__
    void *high_addr = (void *)(0xa000000000000000UL);
#else
    void *high_addr = (void *)(-(intptr_t)page_size);
#endif

    // 创建一个用于文件映射的临时文件。
    int fd = open(TEMPFILE, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (fd < 0)
        simple_die_errno("opening mmapfile failed");
    if (ftruncate(fd, page_size) != 0)
        simple_die_errno("ftruncate");

    // 使用 MAP_FIXED 请求极高地址映射，预期失败。
    errno = 0;
    void *ret = mmap(high_addr, (size_t)page_size, PROT_READ,
                     MAP_SHARED | MAP_FIXED, fd, 0);
    int saved_errno = errno;

    // mmap 成功说明内核接受了不该接受的高地址映射。
    int failed = 0;
    if (ret != MAP_FAILED) {
        fprintf(stderr, "mmap() into high mem region succeeded unexpectedly\n");
        munmap(ret, (size_t)page_size);
        failed = 1;
    } else if (saved_errno == ENOMEM || saved_errno == EINVAL) {
        printf("mmap() failed with expected errno\n");
    } else {
        fprintf(stderr, "mmap() failed with unexpected errno: %s\n", strerror(saved_errno));
        failed = 1;
    }

    // 清理文件和临时目录。
    close(fd);
    unlink(TEMPFILE);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
