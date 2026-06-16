// 本测例验证 PROT_NONE 文件映射虽然可以创建，但访问映射内容会触发 SIGSEGV。

#include "mmap_simple_common.h"

// 原测例使用的临时文件名。
#define TEMPFILE "mmapfile"

#ifdef __linux__
// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap05_XXXXXX";

// SIGSEGV 处理后回到主逻辑的位置。
static sigjmp_buf env;

// 记录是否捕获到 SIGSEGV。
static volatile sig_atomic_t sig_flag;

// 捕获访问 PROT_NONE 映射时产生的 SIGSEGV。
static void sig_handler(int sig)
{
    // 只有 SIGSEGV 表示本测例的预期行为。
    if (sig == SIGSEGV) {
        sig_flag = 1;
        siglongjmp(env, 1);
    }
}
#endif

// 入口函数创建 PROT_NONE 映射并尝试读取。
int main(void)
{
#ifndef __linux__
    // PROT_NONE 访问在非 Linux 主机上可能产生不同信号，无法稳定复现原测例的 SIGSEGV 期望。
    fprintf(stderr, "SKIP: PROT_NONE SIGSEGV semantics require Linux\n");
    return EXIT_SKIP;
#else
    // 安装信号处理器，并准备一页文件内容。
    signal(SIGSEGV, sig_handler);
    simple_enter_temp_dir(temp_dir);
    size_t page_sz = (size_t)simple_page_size();
    char *buf = malloc(page_sz);
    if (!buf)
        simple_die("malloc failed");
    memset(buf, 'A', page_sz);

    // 创建临时文件并写入一页数据。
    int fd = open(TEMPFILE, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (fd < 0 || simple_write_full(fd, buf, page_sz) != 0) {
        fprintf(stderr, "writing to %s failed: %s\n", TEMPFILE, strerror(errno));
        free(buf);
        if (fd >= 0)
            close(fd);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }
    free(buf);

    // 使用 PROT_NONE 创建映射。
    volatile char *addr = mmap(NULL, page_sz, PROT_NONE,
                               MAP_FILE | MAP_SHARED, fd, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mmap() of %s failed: %s\n", TEMPFILE, strerror(errno));
        close(fd);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 尝试访问映射；如果没有 SIGSEGV，会输出原测例的信息行。
    if (sigsetjmp(env, 1) == 0)
        printf("Trying to access mapped region: %c\n", addr[0]);

    // 根据是否捕获 SIGSEGV 输出原测例消息。
    int failed = 0;
    if (sig_flag)
        printf("Received SIGSEGV signal as expected\n");
    else {
        fprintf(stderr, "SIGSEGV signal not received\n");
        failed = 1;
    }

    // 清理映射、文件和临时目录。
    munmap((void *)addr, page_sz);
    close(fd);
    unlink(TEMPFILE);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
