// 本测例验证 umount(2) 的基础功能：先挂载 tmpfs 到目录，再调用 umount，预期成功；运行需要 Linux root 权限。

#include "umount_simple_common.h"

// 原测例使用的挂载点名称。
#define MNTPOINT "mntpoint"

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/umount01_XXXXXX";
#endif

// 入口函数执行 mount 后 umount 的成功路径。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux umount(2) 语义。
    fprintf(stderr, "SKIP: Linux umount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // mount/umount 需要 root 权限或 CAP_SYS_ADMIN。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 创建临时目录和挂载点。
    simple_enter_temp_dir(temp_dir);
    simple_mkdir_p(MNTPOINT, 0775);

    // 先挂载 tmpfs，为 umount 准备目标。
    if (simple_mount_tmpfs(MNTPOINT) != 0) {
        fprintf(stderr, "mount failed: %s\n", strerror(errno));
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 调用 umount，预期成功。
    int failed = 0;
    if (umount(MNTPOINT) == 0) {
        printf("umount() passed\n");
    } else {
        fprintf(stderr, "umount() failed: %s\n", strerror(errno));
        if (errno == EBUSY) {
            printf("umount() Failed with EBUSY possibly some daemon (gvfsd-trash) is probing newly mounted dirs\n");
        }
        failed = 1;
    }

    // 清理临时目录。
    simple_umount_best_effort(MNTPOINT);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
