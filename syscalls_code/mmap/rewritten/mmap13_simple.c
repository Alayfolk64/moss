// 本测例验证映射长度大于文件大小时，mmap 本身成功，但访问不对应文件内容的页面会触发 SIGBUS。

#include "mmap_simple_common.h"

// 原测例使用的临时文件名。
#define TEMPFILE "mmapfile"

// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap13_XXXXXX";

// SIGBUS 处理后回到主逻辑的位置。
static sigjmp_buf env;

// 记录是否捕获到 SIGBUS。
static volatile sig_atomic_t pass;

// 捕获访问文件外映射页面时产生的 SIGBUS。
static void sig_handler(int sig)
{
    // SIGBUS 是本测例的预期信号。
    if (sig == SIGBUS) {
        pass = 1;
        siglongjmp(env, 1);
    }
}

// 入口函数创建短文件的大范围映射并访问文件外页面。
int main(void)
{
    // 安装信号处理器并创建临时目录。
    signal(SIGBUS, sig_handler);
    simple_enter_temp_dir(temp_dir);
    size_t page_sz = (size_t)simple_page_size();

    // 创建文件并截断为半页大小。
    int fd = open(TEMPFILE, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (fd < 0)
        simple_die_errno("opening mmapfile failed");
    if (ftruncate(fd, (off_t)(page_sz / 2)) != 0)
        simple_die_errno("ftruncate");

    // 映射两页，超出文件实际大小的页面稍后会触发 SIGBUS。
    char *addr = mmap(0, page_sz * 2, PROT_READ | PROT_WRITE,
                      MAP_FILE | MAP_SHARED, fd, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mmap() of %s failed: %s\n", TEMPFILE, strerror(errno));
        close(fd);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 尝试写入第二页，预期触发 SIGBUS 并跳回。
    if (sigsetjmp(env, 1) == 0) {
        char *ch = addr + page_sz + 1;
        *ch = 0;
    }

    // 根据是否收到 SIGBUS 输出原测例消息。
    int failed = 0;
    if (pass == 1)
        printf("Received SIGBUS signal as expected\n");
    else {
        fprintf(stderr, "SIGBUS signal not received\n");
        failed = 1;
    }

    // 清理映射和临时文件。
    munmap(addr, page_sz * 2);
    close(fd);
    unlink(TEMPFILE);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
