// 本测例验证文件大小不是页大小整数倍时，mmap 后 EOF 到页尾区域可访问且为 0，并且写入这段区域后不会把 "XYZ" 写回文件。

#include "mmap_simple_common.h"

// 原测例使用的临时文件名。
#define TEMPFILE "mmapfile"

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap01_XXXXXX";

// 检查文件内容中是否存在 XYZ 模式，替代原测例的 grep 命令。
static int file_contains_xyz(int fd)
{
    // 回到文件开头准备读取。
    if (lseek(fd, 0, SEEK_SET) == -1)
        simple_die_errno("Reposition offset failed");

    // 逐块读取文件并查找连续的 XYZ。
    char buf[256];
    int state = 0;
    ssize_t n;
    while ((n = read(fd, buf, sizeof(buf))) > 0) {
        for (ssize_t i = 0; i < n; i++) {
            char c = buf[i];
            if ((state == 0 && c == 'X') ||
                (state == 1 && c == 'Y') ||
                (state == 2 && c == 'Z')) {
                state++;
                if (state == 3)
                    return 1;
            } else {
                state = (c == 'X') ? 1 : 0;
            }
        }
    }

    // read 出错时沿用文件读写失败路径。
    if (n < 0)
        simple_die_errno("reading mmapfile failed");

    return 0;
}

// 清理映射、文件描述符、临时文件和临时目录。
static void cleanup(char *addr, size_t page_sz, int fd)
{
    // 如果映射有效，解除映射。
    if (addr && addr != MAP_FAILED) {
        if (munmap(addr, page_sz) != 0)
            fprintf(stderr, "munmap failed: %s\n", strerror(errno));
    }

    // 关闭文件并删除临时文件。
    if (fd >= 0)
        close(fd);
    unlink(TEMPFILE);

    // 删除临时目录。
    simple_leave_temp_dir(temp_dir);
}

// 入口函数执行原 mmap01 的 EOF 页尾零填充和写回检查。
int main(void)
{
    // 创建临时目录并准备基本数据。
    simple_enter_temp_dir(temp_dir);
    long page_sz = simple_page_size();
    const char write_buf[] = "hello world\n";
    char *dummy = calloc((size_t)page_sz, 1);
    if (!dummy) {
        fprintf(stderr, "calloc failed (dummy)\n");
        cleanup(NULL, 0, -1);
        return EXIT_FAILURE;
    }

    // 创建并写入一个长度小于页大小的文件。
    int fd = open(TEMPFILE, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (fd < 0) {
        fprintf(stderr, "opening %s failed\n", TEMPFILE);
        free(dummy);
        cleanup(NULL, 0, fd);
        return EXIT_FAILURE;
    }
    if (simple_write_full(fd, write_buf, strlen(write_buf)) != 0) {
        fprintf(stderr, "writing to %s\n", TEMPFILE);
        free(dummy);
        cleanup(NULL, 0, fd);
        return EXIT_FAILURE;
    }

    // 读取文件大小，后续只检查 EOF 到页尾的区域。
    struct stat st;
    if (stat(TEMPFILE, &st) < 0) {
        fprintf(stderr, "stat of %s failed: %s\n", TEMPFILE, strerror(errno));
        free(dummy);
        cleanup(NULL, 0, fd);
        return EXIT_FAILURE;
    }
    size_t file_sz = (size_t)st.st_size;

    // 映射一页文件内容，预期 EOF 后到页尾全部为 0。
    char *addr = mmap(NULL, (size_t)page_sz, PROT_READ | PROT_WRITE,
                      MAP_FILE | MAP_SHARED, fd, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mmap of %s failed: %s\n", TEMPFILE, strerror(errno));
        free(dummy);
        cleanup(NULL, 0, fd);
        return EXIT_FAILURE;
    }

    // 检查 EOF 到页尾区域是否全 0。
    if (memcmp(&addr[file_sz], dummy, (size_t)page_sz - file_sz)) {
        fprintf(stderr, "mapped memory area contains invalid data\n");
        free(dummy);
        cleanup(addr, (size_t)page_sz, fd);
        return EXIT_FAILURE;
    }

    // 向 EOF 后的页尾区域写入 XYZ。
    addr[file_sz] = 'X';
    addr[file_sz + 1] = 'Y';
    addr[file_sz + 2] = 'Z';

    // 同步映射，预期不会把 EOF 后的修改写入文件。
    if (msync(addr, (size_t)page_sz, MS_SYNC) != 0) {
        fprintf(stderr, "failed to synchronize mapped file: %s\n", strerror(errno));
        free(dummy);
        cleanup(addr, (size_t)page_sz, fd);
        return EXIT_FAILURE;
    }

    // 检查文件中是否出现 XYZ，并保留原测例的成功/失败消息。
    int failed = 0;
    if (!file_contains_xyz(fd))
        printf("Functionality of mmap() successful\n");
    else {
        fprintf(stderr, "Specified pattern found in file\n");
        failed = 1;
    }

    // 清理资源并返回结果。
    free(dummy);
    cleanup(addr, (size_t)page_sz, fd);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
