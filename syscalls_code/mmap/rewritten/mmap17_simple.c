// 本测例验证 MAP_FIXED_NOREPLACE 不会替换已有映射：在已映射地址再次 mmap 应失败并把 errno 设为 EEXIST。

#include "mmap_simple_common.h"

// 第一个被映射的文件名。
#define FNAME1 "file1_to_mmap"

// 第二个尝试映射到相同地址的文件名。
#define FNAME2 "file2_to_mmap"

#ifdef __linux__
// 写入两个文件的固定字符串。
static const char str[] = "Writing to mapped file";

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap17_XXXXXX";
#endif

// 入口函数执行 MAP_FIXED_NOREPLACE 冲突映射检查。
int main(void)
{
#ifndef __linux__
    // MAP_FIXED_NOREPLACE 是 Linux 语义，非 Linux 主机跳过。
    fprintf(stderr, "SKIP: MAP_FIXED_NOREPLACE requires Linux\n");
    return EXIT_SKIP;
#else
    // 准备临时目录和两个测试文件。
    simple_enter_temp_dir(temp_dir);
    size_t str_len = strlen(str);
    int fd_file1 = open(FNAME1, O_CREAT | O_RDWR | O_TRUNC, 0600);
    int fd_file2 = open(FNAME2, O_CREAT | O_RDWR | O_TRUNC, 0600);
    if (fd_file1 < 0 || fd_file2 < 0)
        simple_die_errno("open");

    // 写入第一个文件并先建立一段私有映射。
    if (simple_write_full(fd_file1, str, str_len) != 0)
        simple_die_errno("write file1_to_mmap");
    void *mapped_address = mmap(NULL, str_len, PROT_WRITE,
                                MAP_PRIVATE, fd_file1, 0);
    if (mapped_address == MAP_FAILED)
        simple_die_errno("mmap file1_to_mmap");

    // 写入第二个文件，随后尝试映射到同一个地址。
    if (simple_write_full(fd_file2, str, str_len) != 0)
        simple_die_errno("write file2_to_mmap");
    errno = 0;
    void *address = mmap(mapped_address, str_len, PROT_WRITE,
                         MAP_PRIVATE | MAP_FIXED_NOREPLACE, fd_file2, 0);
    int saved_errno = errno;

    // 检查 MAP_FIXED_NOREPLACE 是否按预期返回 EEXIST。
    int failed = 0;
    if (address == MAP_FAILED && saved_errno == EEXIST) {
        printf("mmap set errno to EEXIST as expected\n");
    } else {
        fprintf(stderr, "mmap failed, with unexpected error code, expected EEXIST: %s\n",
                strerror(saved_errno));
        if (address != MAP_FAILED)
            munmap(address, str_len);
        failed = 1;
    }

    // 清理映射、文件和临时目录。
    munmap(mapped_address, str_len);
    close(fd_file2);
    close(fd_file1);
    unlink(FNAME1);
    unlink(FNAME2);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
