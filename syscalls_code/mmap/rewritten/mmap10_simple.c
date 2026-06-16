// 本测例验证 /dev/zero 或匿名页映射可以读写，并且 fork 后子进程对中间区域 munmap 不会破坏父进程完整映射；支持原测例的 -a 和 -s 选项。

#include "mmap_simple_common.h"

// 原测例映射的基础大小。
#define SIZE (5 * 1024 * 1024)

// KSM 配置目录，用于 -s 选项的环境检查。
#define PATH_KSM "/sys/kernel/mm/ksm/"

// 记录是否使用匿名页。
static int opt_anon;

// 记录是否尝试加入 KSM。
static int opt_ksm;

// 打印原测例的帮助信息。
static void help(void)
{
    // 保留原始帮助文本。
    printf("  -a      Test anonymous pages\n");
    printf("  -s      Add to KSM regions\n");
}

// 解析 -a、-s 和帮助选项。
static void parse_opts(int argc, char **argv)
{
    // 逐项扫描命令行参数。
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-a")) {
            opt_anon = 1;
        } else if (!strcmp(argv[i], "-s")) {
            opt_ksm = 1;
        } else if (!strcmp(argv[i], "-h") || !strcmp(argv[i], "--help")) {
            help();
            exit(EXIT_SUCCESS);
        }
    }
}

// 在一个子进程中解除部分映射，并用指定失败消息上报错误。
static pid_t fork_munmap_child(char *base, long ps, const char *message)
{
    // 创建子进程执行局部 munmap。
    pid_t pid = fork();
    if (pid < 0) {
        fprintf(stderr, "fork: %s\n", strerror(errno));
        exit(EXIT_FAILURE);
    }

    // 子进程只解除中间部分映射，然后退出。
    if (pid == 0) {
        if (munmap(base + SIZE + ps, SIZE - ps - ps) == -1) {
            fprintf(stderr, "%s: %s\n", message, strerror(errno));
            _exit(1);
        }
        _exit(0);
    }

    return pid;
}

// 执行一次 /dev/zero 或匿名页映射测试。
static int mmapzero(void)
{
    // 根据页大小计算原测例的映射长度。
    long ps = simple_page_size();
    size_t len = SIZE + SIZE - (size_t)ps;
    int fd = -1;
    char *x;

    // 根据 -a 选项选择匿名映射或 /dev/zero 映射。
    if (opt_anon) {
        x = mmap(NULL, len, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    } else {
        fd = open("/dev/zero", O_RDWR, 0666);
        if (fd < 0) {
            fprintf(stderr, "open: %s\n", strerror(errno));
            return EXIT_FAILURE;
        }
        x = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    }

    // mmap 必须成功。
    if (x == MAP_FAILED) {
        fprintf(stderr, "mmap: %s\n", strerror(errno));
        if (fd >= 0)
            close(fd);
        return EXIT_FAILURE;
    }

    // 如果请求 KSM，Linux 上尝试 MADV_MERGEABLE，其他系统跳过能力。
#if defined(MADV_MERGEABLE)
    if (opt_ksm && madvise(x, len, MADV_MERGEABLE) == -1) {
        fprintf(stderr, "madvise: %s\n", strerror(errno));
        munmap(x, len);
        if (fd >= 0)
            close(fd);
        return EXIT_FAILURE;
    }
#endif

    // 写入映射中的一个位置，确保映射可写。
    x[SIZE] = 0;

    // 按原测例创建三个子进程，分别测试局部 munmap。
    pid_t children[3];
    children[0] = fork_munmap_child(x, ps, "munmap");
    children[1] = fork_munmap_child(x, ps, "subsequent munmap #1");
    children[2] = fork_munmap_child(x, ps, "subsequent munmap #2");

    // 父进程解除整段映射，并保留原消息。
    int failed = 0;
    if (munmap(x, len) == -1) {
        fprintf(stderr, "munmap all: %s\n", strerror(errno));
        failed = 1;
    }

    // 等待三个子进程，并保留原测例的 child exit status 消息。
    for (size_t i = 0; i < SIMPLE_ARRAY_SIZE(children); i++) {
        int status;
        if (waitpid(children[i], &status, 0) < 0) {
            fprintf(stderr, "fork: %s\n", strerror(errno));
            failed = 1;
        } else if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            fprintf(stderr, "child exit status is %d\n",
                    WIFEXITED(status) ? WEXITSTATUS(status) : status);
            failed = 1;
        }
    }

    // 关闭 /dev/zero 文件描述符。
    if (fd >= 0)
        close(fd);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}

// 入口函数处理选项和环境检查，然后运行测试。
int main(int argc, char **argv)
{
    // 解析原测例支持的选项。
    parse_opts(argc, argv);

    // KSM 选项要求 Linux KSM 配置目录存在。
    if (opt_ksm) {
        if (access(PATH_KSM, F_OK) == -1) {
            fprintf(stderr, "KSM configuration is not enabled\n");
            return EXIT_SKIP;
        }
#if defined(MADV_MERGEABLE)
        printf("add to KSM regions.\n");
#else
        fprintf(stderr, "MADV_MERGEABLE missing in sys/mman.h\n");
        return EXIT_SKIP;
#endif
    }

    // 保留原测例的映射来源提示。
    if (opt_anon)
        printf("use anonymous pages.\n");
    else
        printf("use /dev/zero.\n");

    // 原测例需要 root；非 root 时给出环境说明。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: root privileges are required\n");
        return EXIT_SKIP;
    }

    // 保留原测例的测试开始提示并执行主体。
    printf("start tests.\n");
    return mmapzero();
}
