// 本测例验证 MS_MOVE 可以把一个已有挂载点移动到新位置：移动后源路径中的文件不可见，目标路径中的文件可见；运行需要 Linux root 权限。

#include "mount_simple_common.h"

// 源挂载点名称。
#define MNTPOINT_SRC "mntpoint1"

// 目标挂载点名称。
#define MNTPOINT_DST "mntpoint2"

// MS_MOVE 旧头文件兜底值。
#ifndef MS_MOVE
#define MS_MOVE 8192
#endif

// MS_PRIVATE 旧头文件兜底值。
#ifndef MS_PRIVATE
#define MS_PRIVATE (1 << 18)
#endif

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/mount06_XXXXXX";
#endif

// 入口函数执行 move mount 检查。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux MS_MOVE 语义。
    fprintf(stderr, "SKIP: Linux mount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // move mount 需要 root 权限或 CAP_SYS_ADMIN。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 创建临时目录并把它转为 private mount，满足 move mount 的父挂载前提。
    simple_enter_temp_dir(temp_dir);
    char cwd[PATH_MAX];
    if (!getcwd(cwd, sizeof(cwd)))
        simple_die_errno("getcwd failed");
    if (mount(cwd, cwd, "none", MS_BIND, NULL) != 0)
        simple_die_errno("bind current dir failed");
    if (mount("none", cwd, "none", MS_PRIVATE, NULL) != 0)
        simple_die_errno("private current dir failed");

    // 准备源和目标挂载点。
    simple_mkdir_p(MNTPOINT_SRC, SIMPLE_DIR_MODE);
    simple_mkdir_p(MNTPOINT_DST, SIMPLE_DIR_MODE);

    // 挂载 tmpfs 到源挂载点并创建测试文件。
    if (simple_mount_tmpfs(MNTPOINT_SRC, 0, NULL) != 0)
        simple_die_errno("mount source failed");
    simple_write_text_file(MNTPOINT_SRC "/testfile", "LTP TEST FILE");

    // 使用 MS_MOVE 把源挂载移动到目标挂载点。
    if (mount(MNTPOINT_SRC, MNTPOINT_DST, SIMPLE_FS_TYPE, MS_MOVE, NULL) != 0)
        simple_die_errno("move mount failed");

    // 检查源路径中的文件不再存在。
    int failed = 0;
    if (access(MNTPOINT_SRC "/testfile", F_OK) == -1 && errno == ENOENT)
        printf("File %s doesn't exist\n", MNTPOINT_SRC "/testfile");
    else {
        fprintf(stderr, "File %s doesn't exist failed unexpectedly\n", MNTPOINT_SRC "/testfile");
        failed = 1;
    }

    // 检查目标路径中的文件存在。
    if (access(MNTPOINT_DST "/testfile", F_OK) == 0)
        printf("File %s exists :\n", MNTPOINT_DST "/testfile");
    else {
        fprintf(stderr, "File %s exists :: %s\n", MNTPOINT_DST "/testfile", strerror(errno));
        failed = 1;
    }

    // 清理目标挂载、当前目录 bind 挂载和临时目录。
    simple_umount_if_mounted(MNTPOINT_SRC);
    simple_umount_if_mounted(MNTPOINT_DST);
    simple_umount_if_mounted(cwd);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
