// 本测例验证 ext4 在小块大小且文件系统耗尽空间时，mmap 文件写回不会静默丢数据；若内核仍有旧 bug，子进程会正常退出并报告 "bug is reproduced"，否则通常被 SIGBUS 杀死并报告 "bug is not reproduced"。

#include "mmap_simple_common.h"

// 原测例挂载点名称。
#define MNTPOINT "mntpoint"

// 父进程不断写满文件系统的文件路径。
#define FILE_PARENT "mntpoint/testfilep"

// 子进程 mmap 并扩展的文件路径。
#define FILE_CHILD "mntpoint/testfilec"

// 原测例使用的 ext4 块大小。
#define FS_BLOCKSIZE 1024

// 原测例重复运行次数。
#define LOOPS 10

#ifdef __linux__
// 用于生成临时工作目录的模板。
static char temp_dir[] = "/tmp/mmap16_XXXXXX";

// 记录 ext4 镜像是否已经挂载。
static int mounted;

// 清理挂载点、镜像文件和临时目录。
static void cleanup(void)
{
    // 已挂载时先卸载挂载点。
    if (mounted) {
        system("umount " MNTPOINT " >/dev/null 2>&1");
        mounted = 0;
    }

    // 删除测试文件、挂载点和镜像文件。
    unlink(FILE_PARENT);
    unlink(FILE_CHILD);
    rmdir(MNTPOINT);
    unlink("mmap16.img");
    simple_leave_temp_dir(temp_dir);
}

// 创建一个 10MB 的 ext4 镜像并挂载到 MNTPOINT。
static int setup_mount(void)
{
    // 创建隔离临时目录和挂载点。
    simple_enter_temp_dir(temp_dir);
    if (mkdir(MNTPOINT, 0755) != 0) {
        fprintf(stderr, "mkdir mntpoint: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 创建 10MB 镜像文件，对应原测例 dev_extra_opts 的 10240 个 1KB 块。
    int img = open("mmap16.img", O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (img < 0) {
        fprintf(stderr, "open mmap16.img: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    if (ftruncate(img, 10 * 1024 * 1024) != 0) {
        fprintf(stderr, "ftruncate mmap16.img: %s\n", strerror(errno));
        close(img);
        return EXIT_FAILURE;
    }
    close(img);

    // 使用 mkfs.ext4 创建 1KB block size 的 ext4 文件系统。
    if (system("mkfs.ext4 -q -b 1024 mmap16.img 10240 >/dev/null 2>&1") != 0) {
        fprintf(stderr, "SKIP: mkfs.ext4 is required\n");
        return EXIT_SKIP;
    }

    // 使用系统 mount 命令通过 loop 设备挂载镜像。
    if (system("mount -o loop mmap16.img " MNTPOINT " >/dev/null 2>&1") != 0) {
        fprintf(stderr, "SKIP: loop mount is required\n");
        return EXIT_SKIP;
    }
    mounted = 1;
    return EXIT_SUCCESS;
}

// 子进程执行 mmap、mremap，并在父进程写满文件系统后继续写映射页。
static void do_child(int ready_fd, int continue_fd)
{
    // 准备子文件，先写入一个 1KB 块。
    int childfd = open(FILE_CHILD, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (childfd < 0) {
        fprintf(stderr, "open child: %s\n", strerror(errno));
        _exit(2);
    }
    char buf[FS_BLOCKSIZE];
    memset(buf, 'a', FS_BLOCKSIZE);
    if (simple_write_full(childfd, buf, FS_BLOCKSIZE) != 0) {
        fprintf(stderr, "write child: %s\n", strerror(errno));
        _exit(2);
    }

    // 先保留一段地址空间，模拟原测例为 mremap 预留可扩展地址的做法。
    int page_size = (int)simple_page_size();
    char *addr = mmap(NULL, (size_t)page_size * 2, PROT_WRITE | PROT_READ,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mmap reserve: %s\n", strerror(errno));
        _exit(2);
    }
    munmap(addr, (size_t)page_size * 2);

    // 在预留地址处映射子文件的第一个块。
    addr = mmap(addr, FS_BLOCKSIZE, PROT_WRITE | PROT_READ, MAP_SHARED, childfd, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mmap child: %s\n", strerror(errno));
        _exit(2);
    }
    addr[0] = 'a';

    // 扩大文件到两个页大小，并把映射扩展过去。
    if (ftruncate(childfd, (off_t)page_size * 2) != 0) {
        fprintf(stderr, "ftruncate child: %s\n", strerror(errno));
        _exit(2);
    }
    addr = mremap(addr, FS_BLOCKSIZE, (size_t)2 * page_size, 0);
    if (addr == MAP_FAILED) {
        fprintf(stderr, "mremap failed unexpectedly: %s\n", strerror(errno));
        _exit(2);
    }

    // 通知父进程开始填满文件系统，并等待父进程完成。
    if (write(ready_fd, "1", 1) != 1)
        _exit(2);
    char token;
    if (read(continue_fd, &token, 1) != 1)
        _exit(2);

    // 文件系统满后写入后续 1KB 块；旧 bug 会静默丢数据，修复后通常 SIGBUS。
    for (int offset = FS_BLOCKSIZE; offset < page_size; offset += FS_BLOCKSIZE)
        addr[offset] = 'a';

    // 如果没有 SIGBUS，则按原测例语义退出 1，表示 bug 被复现。
    munmap(addr, (size_t)2 * page_size);
    close(childfd);
    _exit(1);
}

// 执行一次父子进程协作复现场景。
static int run_single(void)
{
    // 建立两个管道，替代 LTP checkpoint：child_ready 和 parent_done。
    int child_ready[2];
    int parent_done[2];
    if (pipe(child_ready) != 0 || pipe(parent_done) != 0) {
        fprintf(stderr, "pipe: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 创建子进程执行 mmap 写入路径。
    pid_t child = fork();
    if (child < 0) {
        fprintf(stderr, "fork: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    if (child == 0) {
        close(child_ready[0]);
        close(parent_done[1]);
        do_child(child_ready[1], parent_done[0]);
    }

    // 父进程等待子进程 mremap 完成。
    close(child_ready[1]);
    close(parent_done[0]);
    char token;
    if (read(child_ready[0], &token, 1) != 1) {
        fprintf(stderr, "child process terminate unexpectedly with status pipe\n");
        close(child_ready[0]);
        close(parent_done[1]);
        return EXIT_FAILURE;
    }
    close(child_ready[0]);

    // 父进程不断写父文件，直到文件系统返回 ENOSPC。
    int parentfd = open(FILE_PARENT, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (parentfd < 0) {
        fprintf(stderr, "open parent: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }
    char buf[FS_BLOCKSIZE];
    memset(buf, 'a', FS_BLOCKSIZE);
    while (1) {
        ssize_t ret = write(parentfd, buf, FS_BLOCKSIZE);
        if (ret < 0) {
            if (errno == ENOSPC)
                break;
            fprintf(stderr, "write failed unexpectedly: %s\n", strerror(errno));
            close(parentfd);
            return EXIT_FAILURE;
        }
    }
    close(parentfd);

    // 通知子进程继续写 mmap 后续块。
    if (write(parent_done[1], "1", 1) != 1) {
        fprintf(stderr, "checkpoint wake failed: %s\n", strerror(errno));
        close(parent_done[1]);
        return EXIT_FAILURE;
    }
    close(parent_done[1]);

    // 等待子进程，并根据退出状态判断 bug 是否复现。
    int status;
    if (waitpid(child, &status, 0) < 0) {
        fprintf(stderr, "waitpid: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    int bug_reproduced = 0;
    if (WIFEXITED(status) && WEXITSTATUS(status) == 1) {
        bug_reproduced = 1;
    } else if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGBUS) {
        char status_buf[128];
        fprintf(stderr, "child process terminate unexpectedly with status %s\n",
                simple_strstatus(status, status_buf, sizeof(status_buf)));
        return EXIT_FAILURE;
    }

    // 删除两个测试文件，为下一轮循环恢复空间。
    unlink(FILE_PARENT);
    unlink(FILE_CHILD);

    // 保留原测例最终输出。
    if (bug_reproduced) {
        fprintf(stderr, "bug is reproduced\n");
        return EXIT_FAILURE;
    }
    printf("bug is not reproduced\n");
    return EXIT_SUCCESS;
}
#endif

// 入口函数检查环境，挂载小 ext4 文件系统并执行 LOOPS 次复现尝试。
int main(void)
{
#ifndef __linux__
    // 该回归测试依赖 Linux ext4、mremap 和 SIGBUS 语义。
    fprintf(stderr, "SKIP: mmap16 requires Linux ext4 mount support\n");
    return EXIT_SKIP;
#else
    // 原测例需要 root 才能挂载 ext4 镜像。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: root privileges are required\n");
        return EXIT_SKIP;
    }

    // 准备 ext4 挂载环境。
    int rc = setup_mount();
    if (rc != EXIT_SUCCESS) {
        cleanup();
        return rc;
    }

    // 连续运行多轮以提高复现概率。
    int failed = 0;
    for (int i = 0; i < LOOPS; i++) {
        if (run_single() != EXIT_SUCCESS)
            failed = 1;
    }

    // 清理挂载环境。
    cleanup();
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
