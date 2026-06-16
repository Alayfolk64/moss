// 本测例验证普通用户调用 mount(2) 会失败并返回 EPERM；独立版先由 root 准备挂载点，然后临时切换到 nobody 执行 mount。

#include "mount_simple_common.h"

// 原测例使用的挂载点名称。
#define MNTPOINT "mntpoint"

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/mount04_XXXXXX";
#endif

// 入口函数切换到 nobody 并检查 mount 的 EPERM。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux mount(2) 语义。
    fprintf(stderr, "SKIP: Linux mount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // 需要 root 才能切换有效 UID 后再恢复清理。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 查找 nobody 用户。
    struct passwd *pw = getpwnam("nobody");
    if (!pw) {
        fprintf(stderr, "SKIP: nobody user is required\n");
        return EXIT_SKIP;
    }

    // 创建临时目录和挂载点。
    simple_enter_temp_dir(temp_dir);
    simple_mkdir_p(MNTPOINT, SIMPLE_DIR_MODE);

    // 切换到 nobody，再调用 mount，预期 EPERM。
    if (seteuid(pw->pw_uid) != 0) {
        fprintf(stderr, "seteuid(nobody): %s\n", strerror(errno));
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }
    errno = 0;
    int ret = simple_mount_tmpfs(MNTPOINT, 0, NULL);
    int saved_errno = errno;

    // 恢复 root 权限用于清理。
    if (seteuid(0) != 0) {
        fprintf(stderr, "seteuid(0) Failed: %s\n", strerror(errno));
        return EXIT_FAILURE;
    }

    // 检查普通用户 mount 是否按预期失败。
    int failed = 0;
    if (ret == -1 && saved_errno == EPERM) {
        printf("mount() failed expectedly for normal user\n");
    } else {
        fprintf(stderr, "mount() failed expectedly for normal user failed unexpectedly; expected: %d - %s, got: %d - %s\n",
                EPERM, strerror(EPERM), saved_errno, strerror(saved_errno));
        failed = 1;
        if (ret == 0)
            simple_umount_if_mounted(MNTPOINT);
    }

    // 清理临时目录。
    simple_umount_if_mounted(MNTPOINT);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
