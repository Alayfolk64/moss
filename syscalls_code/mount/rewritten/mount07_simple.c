// 本测例验证 MS_NOSYMFOLLOW 挂载选项：未设置时符号链接可被 open 跟随，设置后 open 跟随符号链接应返回 ELOOP，同时 readlink、realpath、symlink/unlink 和 statfs 标志仍符合预期；运行需要 Linux root 权限和支持该 flag 的内核。

#include "mount_simple_common.h"

// 原测例使用的挂载点名称。
#define MNTPOINT "mntpoint"

// statfs 中 MS_NOSYMFOLLOW 对应的标志位。
#ifndef ST_NOSYMFOLLOW
#define ST_NOSYMFOLLOW 0x2000
#endif

// mount 中 MS_NOSYMFOLLOW 对应的标志位。
#ifndef MS_NOSYMFOLLOW
#define MS_NOSYMFOLLOW 0x100
#endif

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/mount07_XXXXXX";

// 测试文件绝对路径。
static char test_file[PATH_MAX];

// 指向测试文件的符号链接绝对路径。
static char link_file[PATH_MAX];

// 临时符号链接路径，用于验证 symlink/unlink。
static char temp_link_file[PATH_MAX];

// 创建测试文件和符号链接。
static void setup_symlink(void)
{
    // 创建目标文件。
    int fd = open(test_file, O_CREAT | O_RDWR, 0644);
    if (fd < 0)
        simple_die_errno("creat test_file failed");

    // 创建符号链接指向目标文件。
    if (symlink(test_file, link_file) != 0 && errno != EEXIST)
        simple_die_errno("symlink link_file failed");

    // 关闭目标文件。
    close(fd);
}

// 验证 open 是否按 nosymfollow 预期跟随或拒绝符号链接。
static int test_link_traversal(bool nosymfollow)
{
    // 根据 nosymfollow 状态执行 open。
    errno = 0;
    int fd = open(link_file, O_RDWR);
    int saved_errno = errno;

    // MS_NOSYMFOLLOW 生效时应返回 ELOOP。
    if (nosymfollow) {
        if (fd == -1 && saved_errno == ELOOP) {
            printf("open(%s, 0, O_RDWR)\n", link_file);
            return 0;
        }
        fprintf(stderr, "open(%s, 0, O_RDWR) failed unexpectedly: %s\n",
                link_file, strerror(saved_errno));
        if (fd >= 0)
            close(fd);
        return 1;
    }

    // 未设置 MS_NOSYMFOLLOW 时，open 应成功。
    if (fd >= 0) {
        printf("open(%s, 0, O_RDWR)\n", link_file);
        close(fd);
        return 0;
    }

    fprintf(stderr, "open(%s, 0, O_RDWR): %s\n", link_file, strerror(saved_errno));
    return 1;
}

// 验证 readlink 仍能读取符号链接目标。
static int test_readlink(void)
{
    // 读取符号链接内容。
    char buf[4096];
    memset(buf, 0, sizeof(buf));
    ssize_t ret = readlink(link_file, buf, sizeof(buf) - 1);
    if (ret < 0) {
        fprintf(stderr, "readlink(%s, buf, %zu): %s\n", link_file, sizeof(buf), strerror(errno));
        return 1;
    }
    printf("readlink(%s, buf, %zu)\n", link_file, sizeof(buf));

    // 比较 readlink 内容和原目标路径。
    if (strcmp(buf, test_file) != 0) {
        fprintf(stderr, "readlink strcmp failed, %s, %s\n", buf, test_file);
        return 1;
    }
    printf("readlink strcmp succeeded\n");
    return 0;
}

// 验证 realpath 仍能解析符号链接目标。
static int test_realpath(void)
{
    // 解析符号链接的真实路径。
    char *resolved = realpath(link_file, NULL);
    if (!resolved) {
        fprintf(stderr, "realpath failed: %s\n", strerror(errno));
        return 1;
    }

    // 比较 realpath 结果和测试文件路径。
    int failed = 0;
    if (strcmp(resolved, test_file) != 0) {
        fprintf(stderr, "realpath strcmp failed, %s, %s\n", resolved, test_file);
        failed = 1;
    } else {
        printf("realpath strcmp succeeded\n");
    }

    // 释放 realpath 返回的缓冲区。
    free(resolved);
    return failed;
}

// 验证在 MS_NOSYMFOLLOW 下仍能创建和删除符号链接。
static int test_cycle_link(void)
{
    // 创建临时符号链接。
    if (symlink(test_file, temp_link_file) == 0)
        printf("symlink(%s, %s)\n", test_file, temp_link_file);
    else {
        fprintf(stderr, "symlink(%s, %s): %s\n", test_file, temp_link_file, strerror(errno));
        return 1;
    }

    // 删除临时符号链接。
    if (unlink(temp_link_file) == 0)
        printf("unlink(%s)\n", temp_link_file);
    else {
        fprintf(stderr, "unlink(%s): %s\n", temp_link_file, strerror(errno));
        return 1;
    }

    return 0;
}

// 验证 statfs 是否正确报告 ST_NOSYMFOLLOW。
static int test_statfs(bool nosymfollow)
{
    // 获取挂载点 statfs 信息。
    struct statfs buf;
    if (statfs(MNTPOINT, &buf) != 0)
        simple_die_errno("statfs failed");

    // 根据是否设置 nosymfollow 检查标志。
    int has_flag = !!(buf.f_flags & ST_NOSYMFOLLOW);
    if (has_flag) {
        if (nosymfollow) {
            printf("ST_NOSYMFOLLOW set on %s\n", MNTPOINT);
            return 0;
        }
        fprintf(stderr, "ST_NOSYMFOLLOW set on %s\n", MNTPOINT);
        return 1;
    }

    if (nosymfollow) {
        fprintf(stderr, "ST_NOSYMFOLLOW not set on %s\n", MNTPOINT);
        return 1;
    }
    printf("ST_NOSYMFOLLOW not set on %s\n", MNTPOINT);
    return 0;
}

// 执行一轮 nosymfollow 前后的行为检查。
static int run_tests(bool nosymfollow)
{
    // 依次执行符号链接遍历、readlink、realpath、创建删除链接和 statfs 检查。
    int failed = 0;
    failed |= test_link_traversal(nosymfollow);
    failed |= test_readlink();
    failed |= test_realpath();
    failed |= test_cycle_link();
    failed |= test_statfs(nosymfollow);
    return failed;
}
#endif

// 入口函数挂载 tmpfs，先检查默认行为，再 remount 为 MS_NOSYMFOLLOW 并复查。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux MS_NOSYMFOLLOW 语义。
    fprintf(stderr, "SKIP: Linux mount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // MS_NOSYMFOLLOW 测试需要 root 权限。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 创建临时目录和挂载点，并计算绝对路径。
    simple_enter_temp_dir(temp_dir);
    simple_mkdir_p(MNTPOINT, SIMPLE_DIR_MODE);
    char cwd[PATH_MAX];
    if (!getcwd(cwd, sizeof(cwd)))
        simple_die_errno("getcwd failed");
    snprintf(test_file, sizeof(test_file), "%s/%s/test_file", cwd, MNTPOINT);
    snprintf(link_file, sizeof(link_file), "%s/%s/link_file", cwd, MNTPOINT);
    snprintf(temp_link_file, sizeof(temp_link_file), "%s/%s/temp_link_file", cwd, MNTPOINT);

    // 先不设置 MS_NOSYMFOLLOW 挂载。
    printf("Testing behaviour when not setting MS_NOSYMFOLLOW\n");
    if (simple_mount_tmpfs(MNTPOINT, 0, NULL) != 0) {
        fprintf(stderr, "mount without MS_NOSYMFOLLOW failed: %s\n", strerror(errno));
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }
    setup_symlink();
    int failed = run_tests(false);

    // remount 并设置 MS_NOSYMFOLLOW。
    printf("Testing behaviour when setting MS_NOSYMFOLLOW\n");
    if (mount(SIMPLE_FS_SOURCE, MNTPOINT, SIMPLE_FS_TYPE,
              MS_REMOUNT | MS_NOSYMFOLLOW, NULL) != 0) {
        fprintf(stderr, "mount MS_NOSYMFOLLOW failed: %s\n", strerror(errno));
        simple_umount_if_mounted(MNTPOINT);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }
    failed |= run_tests(true);

    // 清理挂载点和临时目录。
    simple_umount_if_mounted(MNTPOINT);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
