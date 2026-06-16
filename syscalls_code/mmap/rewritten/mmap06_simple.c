// 本测例验证 mmap 在只写 fd、长度为 0、缺少 MAP_PRIVATE/MAP_SHARED 等非法参数下失败，并返回原测例期望的 errno。

#include "mmap_simple_common.h"

// 单次 mmap 失败用例的参数和期望 errno。
struct tcase {
    size_t length;
    int prot;
    int flags;
    int exp_errno;
};

// 原测例中的映射长度。
#define MMAPSIZE 1024

// 原测例使用的临时文件名。
#define TEMPFILE "mmapfile"

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap06_XXXXXX";

// 覆盖原测例列出的非法 mmap 参数组合。
static struct tcase tcases[] = {
    {MMAPSIZE, PROT_WRITE, MAP_FILE | MAP_PRIVATE, EACCES},
    {MMAPSIZE, PROT_WRITE, MAP_FILE | MAP_SHARED, EACCES},
    {MMAPSIZE, PROT_READ, MAP_FILE | MAP_PRIVATE, EACCES},
    {MMAPSIZE, PROT_READ, MAP_FILE | MAP_SHARED, EACCES},
    {MMAPSIZE, PROT_READ | PROT_WRITE, MAP_FILE | MAP_PRIVATE, EACCES},
    {MMAPSIZE, PROT_READ | PROT_WRITE, MAP_FILE | MAP_SHARED, EACCES},
    {0, PROT_READ | PROT_WRITE, MAP_FILE | MAP_SHARED, EINVAL},
    {MMAPSIZE, PROT_READ | PROT_WRITE, MAP_FILE, EINVAL},
};

// 执行一个非法 mmap 参数组合，并检查 errno。
static int run_case(int fd, const struct tcase *tc)
{
    // 清空 errno 并调用 mmap，预期返回 MAP_FAILED。
    errno = 0;
    void *addr = mmap(NULL, tc->length, tc->prot, tc->flags, fd, 0);
    int saved_errno = errno;

    // 如果 mmap 意外成功，解除映射并报告失败。
    if (addr != MAP_FAILED) {
        fprintf(stderr, "mmap() succeeded unexpectedly\n");
        munmap(addr, tc->length ? tc->length : (size_t)simple_page_size());
        return EXIT_FAILURE;
    }

    // 检查 errno 是否符合当前用例。
    if (saved_errno == tc->exp_errno) {
        printf("mmap() failed as expected\n");
        return EXIT_SUCCESS;
    }

    // errno 不匹配时保留“unexpected errno”的输出语义。
    fprintf(stderr, "mmap() failed unexpectedly; expected: %d - %s: %s\n",
            tc->exp_errno, strerror(tc->exp_errno), strerror(saved_errno));
    return EXIT_FAILURE;
}

// 入口函数准备只写 fd 并遍历非法 mmap 用例。
int main(void)
{
    // 创建临时目录和只写测试文件。
    simple_enter_temp_dir(temp_dir);
    size_t page_sz = (size_t)simple_page_size();
    char *buf = malloc(page_sz);
    if (!buf)
        simple_die("malloc failed");
    memset(buf, 'A', page_sz);

    // 用只写方式打开文件，触发文件映射读权限相关的 EACCES。
    int fd = open(TEMPFILE, O_WRONLY | O_CREAT | O_TRUNC, 0666);
    if (fd < 0 || simple_write_full(fd, buf, page_sz) != 0) {
        fprintf(stderr, "writing to %s failed: %s\n", TEMPFILE, strerror(errno));
        free(buf);
        if (fd >= 0)
            close(fd);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }
    free(buf);

    // 遍历所有错误条件。
    int failed = 0;
    for (size_t i = 0; i < SIMPLE_ARRAY_SIZE(tcases); i++) {
        if (run_case(fd, &tcases[i]) != EXIT_SUCCESS)
            failed = 1;
    }

    // 清理文件和临时目录。
    close(fd);
    unlink(TEMPFILE);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
