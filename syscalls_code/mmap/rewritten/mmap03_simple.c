// 本测例验证可执行权限文件可以用 PROT_EXEC 成功 mmap，并且映射内容与文件内容一致；在特定架构上读取该映射可能触发 SIGSEGV。

#include "mmap_simple_common.h"

// 原测例使用的临时文件名。
#define TEMPFILE "mmapfile"

#ifdef __linux__
// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap03_XXXXXX";

// SIGSEGV 处理后跳回主检查逻辑的位置。
static sigjmp_buf env;

// 记录是否收到了 SIGSEGV。
static volatile sig_atomic_t pass;

// 处理访问 PROT_EXEC 映射时可能出现的 SIGSEGV。
static void sig_handler(int sig)
{
    // SIGSEGV 是原测例在 ia64、hppa、mips 上允许的结果。
    if (sig == SIGSEGV) {
        pass = 1;
        siglongjmp(env, 1);
    }

    // 其他信号保持原测例的 unexpected signal 语义。
    fprintf(stderr, "received an unexpected signal\n");
    _exit(EXIT_FAILURE);
}

// 清理映射、文件描述符、缓冲区和临时目录。
static void cleanup(char *addr, size_t page_sz, int fd, char *dummy)
{
    // 解除有效映射。
    if (addr && addr != MAP_FAILED)
        munmap(addr, page_sz);

    // 关闭文件描述符。
    if (fd >= 0)
        close(fd);

    // 释放读取缓冲区并删除临时文件。
    free(dummy);
    unlink(TEMPFILE);
    simple_leave_temp_dir(temp_dir);
}
#endif

// 入口函数准备可执行文件映射并检查内容。
int main(void)
{
#ifndef __linux__
    // PROT_EXEC-only 文件映射在非 Linux 主机上的权限和信号语义不同，无法稳定复现原 LTP 测例。
    fprintf(stderr, "SKIP: PROT_EXEC mmap semantics require Linux\n");
    return EXIT_SKIP;
#else
    // 安装 SIGSEGV 处理器，并准备一页测试数据。
    signal(SIGSEGV, sig_handler);
    simple_enter_temp_dir(temp_dir);
    size_t page_sz = (size_t)simple_page_size();
    char *test_buff = calloc(page_sz, 1);
    if (!test_buff)
        simple_die("calloc failed (tst_buff)");
    memset(test_buff, 'A', page_sz);

    // 创建临时文件并写入测试数据。
    int fd = open(TEMPFILE, O_WRONLY | O_CREAT | O_TRUNC, 0666);
    if (fd < 0) {
        fprintf(stderr, "opening %s failed: %s\n", TEMPFILE, strerror(errno));
        free(test_buff);
        cleanup(NULL, 0, -1, NULL);
        return EXIT_FAILURE;
    }
    if (simple_write_full(fd, test_buff, page_sz) != 0) {
        fprintf(stderr, "writing to %s failed: %s\n", TEMPFILE, strerror(errno));
        free(test_buff);
        cleanup(NULL, 0, fd, NULL);
        return EXIT_FAILURE;
    }
    free(test_buff);

    // 设置文件可执行权限，再以只读方式重新打开。
    if (fchmod(fd, 0555) < 0) {
        fprintf(stderr, "fchmod of %s failed\n", TEMPFILE);
        cleanup(NULL, 0, fd, NULL);
        return EXIT_FAILURE;
    }
    if (close(fd) < 0) {
        fprintf(stderr, "closing %s failed: %s\n", TEMPFILE, strerror(errno));
        cleanup(NULL, 0, -1, NULL);
        return EXIT_FAILURE;
    }
    fd = open(TEMPFILE, O_RDONLY);
    if (fd < 0) {
        fprintf(stderr, "opening %s read-only failed: %s\n", TEMPFILE, strerror(errno));
        cleanup(NULL, 0, -1, NULL);
        return EXIT_FAILURE;
    }

    // 分配缓冲区并读取文件内容，用来和映射内容比较。
    char *dummy = calloc(page_sz, 1);
    if (!dummy) {
        fprintf(stderr, "calloc failed (dummy)\n");
        cleanup(NULL, 0, fd, NULL);
        return EXIT_FAILURE;
    }
    if (read(fd, dummy, page_sz) < 0) {
        fprintf(stderr, "reading %s failed: %s\n", TEMPFILE, strerror(errno));
        cleanup(NULL, 0, fd, dummy);
        return EXIT_FAILURE;
    }

    // 执行 PROT_EXEC 映射。
    char *addr = mmap(0, page_sz, PROT_EXEC, MAP_FILE | MAP_SHARED, fd, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mmap() failed on %s: %s\n", TEMPFILE, strerror(errno));
        cleanup(NULL, 0, fd, dummy);
        return EXIT_FAILURE;
    }

    // 访问映射并比较数据；特定架构上可能跳回这里。
    int failed = 0;
    if (sigsetjmp(env, 1) == 0) {
        if (memcmp(dummy, addr, page_sz)) {
            fprintf(stderr, "mapped memory region contains invalid data\n");
            failed = 1;
        } else {
            printf("mmap() functionality is correct\n");
        }
    }

    // 按原测例只在这些架构上把 SIGSEGV 作为预期结果报告。
#if defined(__ia64__) || defined(__hppa__) || defined(__mips__)
    if (pass)
        printf("Got SIGSEGV as expected\n");
    else {
        fprintf(stderr, "Mapped memory region with NO access is accessible\n");
        failed = 1;
    }
#else
    if (pass)
        failed = 1;
#endif

    // 解除映射失败时保留原消息。
    if (munmap(addr, page_sz) != 0) {
        fprintf(stderr, "failed to unmap the mmapped pages: %s\n", strerror(errno));
        failed = 1;
        addr = NULL;
    } else {
        addr = NULL;
    }

    // 清理资源并返回结果。
    cleanup(addr, page_sz, fd, dummy);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
