// 本测例验证 mount(2) 可以把一个文件系统挂载到目录上，并能通过挂载状态检查确认目录已经挂载；独立版默认使用 tmpfs，运行需要 Linux root 权限。

#include "mount_simple_common.h"

// 原测例使用的挂载点名称。
#define MNTPOINT "mntpoint"

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/mount01_XXXXXX";
#endif

// 入口函数执行基础 mount 和 umount 检查。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux mount(2) 语义。
    fprintf(stderr, "SKIP: Linux mount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // mount 需要 root 权限或 CAP_SYS_ADMIN。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 创建并进入临时目录，然后准备挂载点。
    simple_enter_temp_dir(temp_dir);
    simple_mkdir_p(MNTPOINT, SIMPLE_DIR_MODE);

    // 调用 mount，把 tmpfs 挂载到测试目录。
    int failed = 0;
    if (simple_mount_tmpfs(MNTPOINT, 0, NULL) != 0) {
        fprintf(stderr, "mount failed: %s\n", strerror(errno));
        failed = 1;
    } else if (simple_is_mounted(MNTPOINT)) {
        printf("folder has been mounted\n");
    } else {
        fprintf(stderr, "folder has not been mounted\n");
        failed = 1;
    }

    // 清理挂载点和临时目录。
    simple_umount_if_mounted(MNTPOINT);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
