// 本测例验证 mount(2) 的常见 flag 行为，包括 MS_RDONLY、MS_NODEV、MS_NOEXEC、MS_REMOUNT、MS_NOSUID、MS_NOATIME、MS_NODIRATIME 和 MS_STRICTATIME；独立版使用 tmpfs，运行需要 Linux root 权限。

#include "mount_simple_common.h"

// 原测例使用的挂载点名称。
#define MNTPOINT "mntpoint"

// 用于验证 MS_NOSUID 的辅助程序名。
#define TESTBIN "mount03_suid_child_simple"

// 辅助程序复制到挂载点内后的路径。
#define BIN_PATH MNTPOINT "/" TESTBIN

// atime 测试写入文件的固定内容。
#define TEST_STR "abcdefghijklmnopqrstuvwxyz"

// 普通文件权限。
#define FILE_MODE 0644

// setuid-root 辅助程序权限。
#define SUID_MODE (0511 | S_ISUID)

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/mount03_XXXXXX";

// 当前测试打开的文件描述符，cleanup 中统一关闭。
static int otfd = -1;

// nobody 用户 UID，用于 MS_NOSUID 场景。
static uid_t nobody_uid;

// nobody 用户 GID，用于 MS_NOSUID 场景。
static gid_t nobody_gid;

// 复制一个文件到目标路径，用于把 suid child 放入挂载点。
static void copy_file(const char *src, const char *dst)
{
    // 打开源文件和目标文件。
    int in = open(src, O_RDONLY);
    if (in < 0)
        simple_die_errno("open source child failed");
    int out = open(dst, O_CREAT | O_WRONLY | O_TRUNC, 0755);
    if (out < 0)
        simple_die_errno("open target child failed");

    // 分块复制内容。
    char buf[4096];
    ssize_t n;
    while ((n = read(in, buf, sizeof(buf))) > 0) {
        if (simple_write_full(out, buf, (size_t)n) != 0)
            simple_die_errno("write target child failed");
    }
    if (n < 0)
        simple_die_errno("read source child failed");

    // 关闭两个文件描述符。
    close(in);
    close(out);
}

// 清理当前 flag 场景遗留的打开文件和挂载。
static void cleanup_mount(void)
{
    // 关闭测试文件描述符。
    if (otfd >= 0) {
        close(otfd);
        otfd = -1;
    }

    // 如果挂载点仍挂载，则卸载它。
    simple_umount_if_mounted(MNTPOINT);
}

// 验证 MS_RDONLY：只读挂载上创建文件应返回 EROFS。
static int test_rdonly(void)
{
    // 在只读挂载点中尝试创建文件。
    errno = 0;
    otfd = open(MNTPOINT "/rdonly", O_CREAT | O_RDWR, 0700);
    int saved_errno = errno;

    // 检查 open 是否因只读文件系统失败。
    if (otfd == -1 && saved_errno == EROFS) {
        printf("open() failed as expected\n");
        return 0;
    }

    // 成功或 errno 不匹配都表示 MS_RDONLY 行为异常。
    fprintf(stderr, "open() failed unexpectedly; expected: %d - %s, got: %d - %s\n",
            EROFS, strerror(EROFS), saved_errno, strerror(saved_errno));
    if (otfd >= 0)
        close(otfd);
    otfd = -1;
    return 1;
}

// 验证 MS_NODEV：设备节点存在时，打开应返回 EACCES。
static int test_nodev(void)
{
    // 在挂载点中创建一个块设备节点。
    const char *path = MNTPOINT "/nodev";
    if (mknod(path, S_IFBLK | 0777, 0) != 0 && errno != EEXIST)
        simple_die_errno("mknod nodev failed");

    // 尝试打开设备节点，MS_NODEV 应阻止访问。
    errno = 0;
    otfd = open(path, O_RDWR, 0700);
    int saved_errno = errno;

    // 检查 EACCES。
    int failed = 0;
    if (otfd == -1 && saved_errno == EACCES) {
        printf("open() failed as expected\n");
    } else {
        fprintf(stderr, "open() failed unexpectedly; expected: %d - %s, got: %d - %s\n",
                EACCES, strerror(EACCES), saved_errno, strerror(saved_errno));
        failed = 1;
    }

    // 删除设备节点。
    if (otfd >= 0)
        close(otfd);
    otfd = -1;
    unlink(path);
    return failed;
}

// 验证 MS_NOEXEC：挂载点内文件不能执行。
static int test_noexec(void)
{
    // 创建一个空的可执行文件。
    const char *path = MNTPOINT "/noexec";
    otfd = open(path, O_CREAT | O_RDWR, 0700);
    if (otfd < 0)
        simple_die_errno("open noexec failed");
    close(otfd);
    otfd = -1;

    // 子进程尝试执行该文件，避免替换主测试进程。
    pid_t pid = fork();
    if (pid < 0)
        simple_die_errno("fork failed");
    if (pid == 0) {
        execl(path, path, NULL);
        _exit(errno == EACCES ? 0 : 1);
    }

    // 父进程检查子进程是否按 EACCES 退出。
    int status;
    waitpid(pid, &status, 0);
    if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
        printf("execlp() failed as expected\n");
        return 0;
    }

    fprintf(stderr, "execlp() failed unexpectedly; expected: %d - %s\n",
            EACCES, strerror(EACCES));
    return 1;
}

// 验证 MS_REMOUNT：先只读挂载，再 remount 为可写后应能创建文件。
static int test_remount(void)
{
    // 用 MS_REMOUNT 改变挂载选项。
    if (mount(SIMPLE_FS_SOURCE, MNTPOINT, SIMPLE_FS_TYPE, MS_REMOUNT, NULL) != 0)
        simple_die_errno("mount remount failed");

    // remount 为可写后，创建文件应成功。
    otfd = open(MNTPOINT "/remount", O_CREAT | O_RDWR, 0700);
    if (otfd >= 0) {
        printf("open() returned fd\n");
        return 0;
    }

    fprintf(stderr, "open() failed: %s\n", strerror(errno));
    return 1;
}

// 验证 MS_NOSUID：setuid-root 程序在 nosuid 挂载点中不能获得 root 权限。
static int test_nosuid(void)
{
    // 复制辅助程序到挂载点内，并设置 setuid-root 权限。
    copy_file("./" TESTBIN, BIN_PATH);
    if (chown(BIN_PATH, 0, 0) != 0)
        simple_die_errno("chown child failed");
    if (chmod(BIN_PATH, SUID_MODE) != 0)
        simple_die_errno("chmod child failed");

    // 子进程切换到 nobody 后执行辅助程序。
    pid_t pid = fork();
    if (pid < 0)
        simple_die_errno("fork failed");
    if (pid == 0) {
        if (setregid(nobody_gid, nobody_gid) != 0)
            _exit(1);
        if (setreuid(nobody_uid, nobody_uid) != 0)
            _exit(1);
        execl(BIN_PATH, BIN_PATH, NULL);
        _exit(1);
    }

    // 父进程等待辅助程序返回。
    int status;
    waitpid(pid, &status, 0);
    if (WIFEXITED(status) && WEXITSTATUS(status) == 0)
        return 0;

    fprintf(stderr, "Failed to execute %s: %s\n", BIN_PATH, strerror(errno));
    return 1;
}

// 验证文件和目录 atime 是否按预期更新。
static int test_file_dir_noatime(int update_fatime, int update_datime)
{
    // 创建文件和目录，并记录初始 atime。
    const char *file = MNTPOINT "/noatime";
    const char *dir = MNTPOINT "/nodiratime";
    otfd = open(file, O_CREAT | O_RDWR | O_TRUNC, 0700);
    if (otfd < 0)
        simple_die_errno("open noatime failed");
    if (mkdir(dir, 0700) != 0 && errno != EEXIST)
        simple_die_errno("mkdir nodiratime failed");

    // 写入测试内容并读取初始文件 atime。
    if (simple_write_full(otfd, TEST_STR, strlen(TEST_STR)) != 0)
        simple_die_errno("write noatime failed");
    struct stat st;
    if (fstat(otfd, &st) != 0)
        simple_die_errno("fstat noatime failed");
    time_t atime = st.st_atime;

    // 打开并读取目录一次，记录初始目录 atime。
    DIR *test_dir = opendir(dir);
    if (!test_dir)
        simple_die_errno("opendir nodiratime failed");
    readdir(test_dir);
    closedir(test_dir);
    struct stat dir_st;
    if (stat(dir, &dir_st) != 0)
        simple_die_errno("stat nodiratime failed");
    time_t dir_atime = dir_st.st_atime;

    // 等待超过一秒，避免 atime 秒级时间戳无法区分。
    usleep(1001000);

    // 读文件内容，触发可能的文件 atime 更新。
    char readbuf[20];
    lseek(otfd, 0, SEEK_SET);
    read(otfd, readbuf, sizeof(readbuf));
    if (fstat(otfd, &st) != 0)
        simple_die_errno("fstat noatime failed");

    // 再次读取目录，触发可能的目录 atime 更新。
    test_dir = opendir(dir);
    if (!test_dir)
        simple_die_errno("opendir nodiratime failed");
    readdir(test_dir);
    closedir(test_dir);
    if (stat(dir, &dir_st) != 0)
        simple_die_errno("stat nodiratime failed");

    // 检查文件 atime 是否符合预期。
    int failed = 0;
    if (update_fatime) {
        if (st.st_atime > atime)
            printf("st.st_atime(%ld) > atime(%ld)\n", (long)st.st_atime, (long)atime);
        else {
            fprintf(stderr, "st.st_atime(%ld) < atime(%ld)\n", (long)st.st_atime, (long)atime);
            failed = 1;
        }
    } else if (st.st_atime == atime) {
        printf("st.st_atime == atime\n");
    } else {
        fprintf(stderr, "st.st_atime != atime\n");
        failed = 1;
    }

    // 检查目录 atime 是否符合预期。
    if (update_datime) {
        if (dir_st.st_atime > dir_atime)
            printf("dir_st.st_atime(%ld) > dir_atime(%ld)\n", (long)dir_st.st_atime, (long)dir_atime);
        else {
            fprintf(stderr, "dir_st.st_atime(%ld) < dir_atime(%ld)\n", (long)dir_st.st_atime, (long)dir_atime);
            failed = 1;
        }
    } else if (dir_st.st_atime == dir_atime) {
        printf("dir_st.st_atime == dir_atime\n");
    } else {
        fprintf(stderr, "dir_st.st_atime != dir_atime\n");
        failed = 1;
    }

    // 关闭当前测试文件。
    close(otfd);
    otfd = -1;
    return failed;
}

// 单个 flag 测试用例。
struct tcase {
    unsigned long flag;
    unsigned long flag2;
    const char *desc;
    int (*test)(void);
};

// MS_NOATIME 的包装函数。
static int test_noatime(void)
{
    // 文件和目录 atime 都不应更新。
    return test_file_dir_noatime(0, 0);
}

// MS_NODIRATIME 的包装函数。
static int test_nodiratime(void)
{
    // 文件 atime 应更新，目录 atime 不应更新。
    return test_file_dir_noatime(1, 0);
}

// MS_STRICTATIME 的包装函数。
static int test_strictatime(void)
{
    // 文件和目录 atime 都应更新。
    return test_file_dir_noatime(1, 1);
}

// 原测例覆盖的 flag 列表。
static struct tcase tcases[] = {
    {MS_RDONLY, MS_RDONLY, "MS_RDONLY", test_rdonly},
    {MS_NODEV, MS_NODEV, "MS_NODEV", test_nodev},
    {MS_NOEXEC, MS_NOEXEC, "MS_NOEXEC", test_noexec},
    {MS_RDONLY, MS_REMOUNT, "MS_REMOUNT", test_remount},
    {MS_NOSUID, MS_NOSUID, "MS_NOSUID", test_nosuid},
    {MS_NOATIME, MS_NOATIME, "MS_NOATIME", test_noatime},
    {MS_NODIRATIME, MS_NODIRATIME, "MS_NODIRATIME", test_nodiratime},
    {MS_STRICTATIME, MS_STRICTATIME, "MS_STRICTATIME", test_strictatime},
};

// 执行单个 flag 场景。
static int run_case(struct tcase *tc)
{
    // 输出原测例的信息文本。
    printf("Testing flag %s\n", tc->desc);

    // 按当前 flag 挂载 tmpfs。
    if (simple_mount_tmpfs(MNTPOINT, tc->flag, NULL) != 0) {
        fprintf(stderr, "mount %s failed: %s\n", tc->desc, strerror(errno));
        return 1;
    }

    // 执行 flag 专属行为检查。
    int failed = 0;
    if (tc->test && tc->test() != 0)
        failed = 1;

    // 读取 statfs flags，验证内核记录的挂载 flag。
    struct statfs stfs;
    if (statfs(MNTPOINT, &stfs) != 0) {
        fprintf(stderr, "statfs failed: %s\n", strerror(errno));
        failed = 1;
    } else if (tc->flag == MS_STRICTATIME) {
#ifdef MS_RELATIME
        if (stfs.f_flags & (MS_NOATIME | MS_RELATIME)) {
#else
        if (stfs.f_flags & MS_NOATIME) {
#endif
            fprintf(stderr, "statfs() gets the incorrect mount flag\n");
            failed = 1;
        } else {
            printf("statfs() gets the correct mount flag\n");
        }
    } else if (stfs.f_flags & tc->flag2) {
        printf("statfs() gets the correct mount flag\n");
    } else {
        fprintf(stderr, "statfs() gets the incorrect mount flag\n");
        failed = 1;
    }

    // 清理当前挂载。
    cleanup_mount();
    return failed;
}
#endif

// 入口函数准备 nobody 用户和挂载点，并遍历所有 flag。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux mount flag 语义。
    fprintf(stderr, "SKIP: Linux mount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // mount flag 测试需要 root 权限。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 获取 nobody 用户信息，用于 MS_NOSUID 测试。
    struct passwd *ltpuser = getpwnam("nobody");
    if (!ltpuser) {
        fprintf(stderr, "SKIP: nobody user is required\n");
        return EXIT_SKIP;
    }
    nobody_uid = ltpuser->pw_uid;
    nobody_gid = ltpuser->pw_gid;

    // 创建临时目录和挂载点。
    simple_enter_temp_dir(temp_dir);
    simple_mkdir_p(MNTPOINT, SIMPLE_DIR_MODE);

    // 遍历所有 flag 测试。
    int failed = 0;
    for (size_t i = 0; i < sizeof(tcases) / sizeof(tcases[0]); i++) {
        if (run_case(&tcases[i]) != 0)
            failed = 1;
    }

    // 清理挂载点和临时目录。
    cleanup_mount();
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
