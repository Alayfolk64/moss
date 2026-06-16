// 本测例验证两个文件 mmap 后再用对方原虚拟地址重新 mmap，内容仍然正确，覆盖内核 TLB 刷新相关回归场景。

#include "mmap_simple_common.h"

// 每个文件映射的长度。
#define LEN 64

// 第一个测试文件名。
static const char tmp1[] = "testfile1";

// 第二个测试文件名。
static const char tmp2[] = "testfile2";

// 第一个文件的测试字符串。
static const char str1[] = "testing 123";

// 第二个文件的测试字符串。
static const char str2[] = "my test mem";

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap19_XXXXXX";

// 比较映射内容和期望字符串。
static void compare_or_die(const char *expected, const char *mapped, const char *name)
{
    // strncmp 不匹配时保留原测例失败文本。
    if (strncmp(expected, mapped, strlen(expected))) {
        fprintf(stderr, "failed on compare %s\n", name);
        exit(EXIT_FAILURE);
    }
}

// 入口函数执行映射、交换地址重映射和内容校验。
int main(void)
{
    // 创建临时目录和两个测试文件。
    simple_enter_temp_dir(temp_dir);
    int f1 = open(tmp1, O_RDWR | O_CREAT | O_TRUNC, S_IRUSR | S_IWUSR);
    int f2 = open(tmp2, O_RDWR | O_CREAT | O_TRUNC, S_IRUSR | S_IWUSR);
    if (f1 < 0 || f2 < 0)
        simple_die_errno("open");
    if (simple_write_full(f1, str1, strlen(str1)) != 0 ||
        simple_write_full(f2, str2, strlen(str2)) != 0)
        simple_die_errno("write");

    // 第一次映射两个文件并保存地址。
    char *mm1 = mmap(0, LEN, PROT_READ, MAP_PRIVATE, f1, 0);
    char *mm2 = mmap(0, LEN, PROT_READ, MAP_PRIVATE, f2, 0);
    if (mm1 == MAP_FAILED || mm2 == MAP_FAILED)
        simple_die_errno("mmap");
    char *save_mm1 = mm1;
    char *save_mm2 = mm2;

    // 首次检查两个映射的内容。
    compare_or_die(str1, mm1, tmp1);
    compare_or_die(str2, mm2, tmp2);

    // 解除映射后，尝试把两个文件映射到对方原来的地址。
    munmap(mm1, LEN);
    munmap(mm2, LEN);
    mm1 = mmap(save_mm2, LEN, PROT_READ, MAP_PRIVATE, f1, 0);
    mm2 = mmap(save_mm1, LEN, PROT_READ, MAP_PRIVATE, f2, 0);
    if (mm1 == MAP_FAILED || mm2 == MAP_FAILED)
        simple_die_errno("mmap");

    // 如果内核没有使用请求地址，保留原测例的信息输出。
    if (mm1 != save_mm2 || mm2 != save_mm1)
        printf("mmap not using same address\n");

    // 再次检查内容，确认重映射后没有读到旧 TLB 数据。
    compare_or_die(str1, mm1, tmp1);
    compare_or_die(str2, mm2, tmp2);
    printf("memory test succeeded\n");

    // 清理映射、文件和临时目录。
    munmap(mm1, LEN);
    munmap(mm2, LEN);
    close(f1);
    close(f2);
    unlink(tmp1);
    unlink(tmp2);
    simple_leave_temp_dir(temp_dir);
    return EXIT_SUCCESS;
}
