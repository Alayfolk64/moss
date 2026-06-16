// 本测例验证 umount(2) 的常见错误路径会失败并返回预期 errno，包括忙挂载点、非法地址、不存在目录、不是挂载点以及超长路径；运行需要 Linux root 权限。

#include "umount_simple_common.h"

// 原测例使用的挂载点名称。
#define MNTPOINT "mntpoint"

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/umount02_XXXXXX";

// 超长路径缓冲区，用于触发 ENAMETOOLONG。
static char long_path[PATH_MAX + 2];

// 单个 umount 错误场景。
struct tcase {
    const char *err_desc;
    const char *mntpoint;
    int exp_errno;
};

// 原测例列出的错误场景。
static struct tcase tcases[] = {
    {"Already mounted/busy", MNTPOINT, EBUSY},
    {"Invalid address", (const char *)-1, EFAULT},
    {"Directory not found", "nonexistent", ENOENT},
    {"Invalid  device", "./", EINVAL},
    {"Pathname too long", long_path, ENAMETOOLONG},
};

// 执行单个 umount 错误场景。
static int run_case(const struct tcase *tc)
{
    // 调用 umount，预期失败。
    errno = 0;
    int ret = umount(tc->mntpoint);
    int saved_errno = errno;

    // 检查 errno 是否符合预期。
    if (ret == -1 && saved_errno == tc->exp_errno) {
        printf("umount() failed as expected\n");
        return 0;
    }

    // 报告非预期结果。
    fprintf(stderr, "umount() failed unexpectedly; expected: %d - %s, got: %d - %s\n",
            tc->exp_errno, strerror(tc->exp_errno),
            saved_errno, strerror(saved_errno));
    return 1;
}
#endif

// 入口函数准备 busy 挂载点并遍历错误场景。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux umount(2) 语义。
    fprintf(stderr, "SKIP: Linux umount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // umount 错误路径测试需要 root 权限越过 EPERM。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 创建临时目录和挂载点。
    simple_enter_temp_dir(temp_dir);
    simple_mkdir_p(MNTPOINT, 0775);

    // 挂载 tmpfs，并打开挂载点内文件制造 busy 场景。
    if (simple_mount_tmpfs(MNTPOINT) != 0) {
        fprintf(stderr, "mount failed: %s\n", strerror(errno));
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }
    int fd = open(MNTPOINT "/file", O_CREAT | O_RDWR, 0777);
    if (fd < 0) {
        fprintf(stderr, "creat file failed: %s\n", strerror(errno));
        simple_umount_best_effort(MNTPOINT);
        simple_leave_temp_dir(temp_dir);
        return EXIT_FAILURE;
    }

    // 构造超长路径。
    memset(long_path, 'a', sizeof(long_path) - 1);
    long_path[sizeof(long_path) - 1] = '\0';

    // 遍历错误场景。
    int failed = 0;
    for (size_t i = 0; i < sizeof(tcases) / sizeof(tcases[0]); i++) {
        if (run_case(&tcases[i]) != 0)
            failed = 1;
    }

    // 清理打开文件和挂载点。
    close(fd);
    simple_umount_best_effort(MNTPOINT);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
