// 本测例验证非超级用户调用 umount(2) 会失败并返回 EPERM；独立版由 root 先挂载 tmpfs，再切换到 nobody 执行 umount。

#include "umount_simple_common.h"

// 原测例使用的挂载点名称。
#define MNTPOINT "mntpoint"

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/umount03_XXXXXX";
#endif

// 入口函数切换普通用户并检查 umount 的 EPERM。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux umount(2) 语义。
    fprintf(stderr, "SKIP: Linux umount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // 需要 root 才能挂载并在测试后恢复权限。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 获取 nobody 用户。
    struct passwd *pw = getpwnam("nobody");
    if (!pw) {
        fprintf(stderr, "SKIP: nobody user is required\n");
        return EXIT_SKIP;
    }

    // 创建临时目录、挂载点并挂载 tmpfs。
    simple_enter_temp_dir(temp_dir);
    simple_mkdir_p(MNTPOINT, 0775);
    if (simple_mount_tmpfs(MNTPOINT) != 0) {
        fprintf(stderr, "mount failed: %s\n", strerror(errno));
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 切换到 nobody 后调用 umount，预期 EPERM。
    if (seteuid(pw->pw_uid) != 0) {
        fprintf(stderr, "seteuid(nobody): %s\n", strerror(errno));
        simple_umount_best_effort(MNTPOINT);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }
    errno = 0;
    int ret = umount(MNTPOINT);
    int saved_errno = errno;

    // 恢复 root 权限以便清理。
    if (seteuid(0)) {
        fprintf(stderr, "seteuid(0) Failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 检查非 root umount 是否返回 EPERM。
    int failed = 0;
    if (ret == -1 && saved_errno == EPERM) {
        printf("umount() failed as expected\n");
    } else {
        fprintf(stderr, "umount() failed unexpectedly; expected: %d - %s, got: %d - %s\n",
                EPERM, strerror(EPERM), saved_errno, strerror(saved_errno));
        failed = 1;
    }

    // 清理挂载点和临时目录。
    simple_umount_best_effort(MNTPOINT);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
