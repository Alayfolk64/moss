// 本测例验证 MS_BIND 可以把一个已挂载目录树绑定到另一个挂载点，并且原目录中的文件和子目录能在新挂载点访问；运行需要 Linux root 权限。

#include "mount_simple_common.h"

// 原测例的第一个挂载点。
#define MNTPOINT1 "mntpoint1"

// 第一个挂载点中的测试文件。
#define TESTFILE1 MNTPOINT1 "/testfile"

// 第一个挂载点中的测试目录。
#define TESTDIR1 MNTPOINT1 "/testdir"

// 原测例的第二个挂载点。
#define MNTPOINT2 "mntpoint2"

// bind 挂载后应能看到的测试文件。
#define TESTFILE2 MNTPOINT2 "/testfile"

// bind 挂载后应能看到的测试目录。
#define TESTDIR2 MNTPOINT2 "/testdir"

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/mount05_XXXXXX";
#endif

// 入口函数执行 bind mount 可见性检查。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux MS_BIND 语义。
    fprintf(stderr, "SKIP: Linux mount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // bind mount 需要 root 权限或 CAP_SYS_ADMIN。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 创建临时目录和两个挂载点。
    simple_enter_temp_dir(temp_dir);
    simple_mkdir_p(MNTPOINT1, SIMPLE_DIR_MODE);
    simple_mkdir_p(MNTPOINT2, SIMPLE_DIR_MODE);

    // 先把 tmpfs 挂载到第一个挂载点。
    if (simple_mount_tmpfs(MNTPOINT1, 0, NULL) != 0) {
        fprintf(stderr, "mount mntpoint1 failed: %s\n", strerror(errno));
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 在第一个挂载点中创建文件和目录。
    printf("Creating file in '%s'\n", TESTFILE1);
    simple_write_text_file(TESTFILE1, "LTP TEST FILE");
    simple_mkdir_p(TESTDIR1, 0750);

    // 把第一个挂载点 bind 到第二个挂载点。
    if (mount(MNTPOINT1, MNTPOINT2, NULL, MS_BIND, NULL) != 0) {
        fprintf(stderr, "mount bind failed: %s\n", strerror(errno));
        simple_umount_if_mounted(MNTPOINT1);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 检查 bind 挂载后的文件和目录是否可见，并保留原测例输出文本。
    int failed = 0;
    if (access(TESTFILE2, F_OK) == 0)
        printf("Accessing to '%s'\n", TESTFILE2);
    else {
        fprintf(stderr, "Accessing to '%s': %s\n", TESTFILE2, strerror(errno));
        failed = 1;
    }
    if (access(TESTDIR2, F_OK) == 0)
        printf("Accessing to '%s'\n", TESTDIR2);
    else {
        fprintf(stderr, "Accessing to '%s': %s\n", TESTDIR2, strerror(errno));
        failed = 1;
    }

    // 按挂载层次清理。
    simple_umount_if_mounted(MNTPOINT2);
    simple_umount_if_mounted(MNTPOINT1);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
