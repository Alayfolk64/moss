// 本测例验证只读打开的文件描述符可以用 PROT_READ + MAP_SHARED 成功 mmap，且映射内容与文件内容一致。

#include "mmap_simple_common.h"

// 原测例使用的临时文件名。
#define TEMPFILE "mmapfile"

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap02_XXXXXX";

// 入口函数准备只读文件映射并比较内容。
int main(void)
{
    // 创建隔离临时目录并分配一页测试数据。
    simple_enter_temp_dir(temp_dir);
    size_t page_sz = (size_t)simple_page_size();
    char *buf = malloc(page_sz);
    if (!buf) {
        fprintf(stderr, "malloc failed\n");
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }
    memset(buf, 'A', page_sz);

    // 创建文件并写入一整页 A。
    int fd = open(TEMPFILE, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (fd < 0 || simple_write_full(fd, buf, page_sz) != 0) {
        fprintf(stderr, "writing to %s failed: %s\n", TEMPFILE, strerror(errno));
        free(buf);
        if (fd >= 0)
            close(fd);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 改成只读权限并以只读方式重新打开。
    if (fchmod(fd, 0444) != 0 || close(fd) != 0) {
        fprintf(stderr, "closing %s failed: %s\n", TEMPFILE, strerror(errno));
        free(buf);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }
    fd = open(TEMPFILE, O_RDONLY);
    if (fd < 0) {
        fprintf(stderr, "opening %s read-only failed: %s\n", TEMPFILE, strerror(errno));
        free(buf);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 执行只读共享映射。
    char *addr = mmap(NULL, page_sz, PROT_READ, MAP_FILE | MAP_SHARED, fd, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mmap() failed on %s: %s\n", TEMPFILE, strerror(errno));
        free(buf);
        close(fd);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 比较映射内容与原缓冲区，并保留原输出文本。
    int failed = 0;
    if (memcmp(buf, addr, page_sz) == 0)
        printf("mmap() functionality successful\n");
    else {
        fprintf(stderr, "mapped memory area contains invalid data\n");
        failed = 1;
    }

    // 解除映射并清理文件。
    munmap(addr, page_sz);
    close(fd);
    unlink(TEMPFILE);
    free(buf);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
