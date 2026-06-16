// 本测例验证 mount(2) 的常见错误路径会失败并返回预期 errno，包括错误文件系统类型、非块设备、忙挂载点、无效指针、超长路径、不存在目录和目标不是目录等情况；运行需要 Linux root 权限。

#include "mount_simple_common.h"

// 原测例使用的挂载点名称。
#define MNTPOINT "mntpoint"

// 用于触发 remount busy 的测试文件路径。
#define TEST_FILE MNTPOINT "/file"

#ifdef __linux__
// 用于创建临时工作目录的模板。
static char temp_dir[] = "/tmp/mount02_XXXXXX";

// 超长路径缓冲区，用于触发 ENAMETOOLONG。
static char long_path[PATH_MAX + 2];

// 单个 mount 错误场景的参数和期望 errno。
struct test_case {
    const char *device;
    const char *mntpoint;
    const char *fs_type;
    const char *desc;
    unsigned long flag;
    int exp_errno;
    int needs_pre_mount;
    int needs_open_file;
};

// 覆盖原 mount02 中列出的错误场景。
static struct test_case test_cases[] = {
    {SIMPLE_FS_SOURCE, MNTPOINT, "error", "wrong FS type", 0, ENODEV, 0, 0},
    {"char_device", MNTPOINT, "ext4", "char device", 0, ENOTBLK, 0, 0},
    {SIMPLE_FS_SOURCE, MNTPOINT, SIMPLE_FS_TYPE, "mounted folder", 0, EBUSY, 1, 0},
    {SIMPLE_FS_SOURCE, MNTPOINT, SIMPLE_FS_TYPE, "mounted folder containing file", MS_REMOUNT | MS_RDONLY, EBUSY, 1, 1},
    {NULL, MNTPOINT, SIMPLE_FS_TYPE, "invalid device", 0, EINVAL, 0, 0},
    {SIMPLE_FS_SOURCE, MNTPOINT, NULL, "invalid device type", 0, EINVAL, 0, 0},
    {SIMPLE_FS_SOURCE, MNTPOINT, SIMPLE_FS_TYPE, "mounted folder", MS_REMOUNT, EINVAL, 0, 0},
    {(const char *)-1, MNTPOINT, SIMPLE_FS_TYPE, "fault device", 0, EFAULT, 0, 0},
    {SIMPLE_FS_SOURCE, MNTPOINT, (const char *)-1, "fault device type", 0, EFAULT, 0, 0},
    {SIMPLE_FS_SOURCE, long_path, SIMPLE_FS_TYPE, "long name", 0, ENAMETOOLONG, 0, 0},
    {SIMPLE_FS_SOURCE, "nonexistent", SIMPLE_FS_TYPE, "non existant folder", 0, ENOENT, 0, 0},
    {SIMPLE_FS_SOURCE, "filename", SIMPLE_FS_TYPE, "file", 0, ENOTDIR, 0, 0},
};

// 准备错误场景需要的字符设备和普通文件。
static void setup_inputs(void)
{
    // 构造超过 PATH_MAX 的路径名。
    memset(long_path, 'a', sizeof(long_path) - 1);
    long_path[sizeof(long_path) - 1] = '\0';

    // 创建字符设备，供 ENOTBLK 场景使用；失败时继续，具体 errno 会由 mount 场景体现。
    if (mknod("char_device", S_IFCHR | 0777, 0) != 0 && errno != EEXIST)
        fprintf(stderr, "mknod char_device: %s\n", strerror(errno));

    // 创建普通文件，供 ENOTDIR 场景作为 mount target。
    int fd = open("filename", O_CREAT | O_RDWR, 0777);
    if (fd >= 0)
        close(fd);
}

// 执行单个 mount 错误场景。
static int run_case(struct test_case *tc)
{
    // 如果场景需要预先挂载，就先挂载 tmpfs。
    int fd = -1;
    if (tc->needs_pre_mount) {
        if (simple_mount_tmpfs(MNTPOINT, 0, NULL) != 0) {
            fprintf(stderr, "mounting %s setup failed: %s\n", tc->desc, strerror(errno));
            return EXIT_FAILURE;
        }
    }

    // 如果场景需要打开文件，就在挂载点中创建并保持写打开状态。
    if (tc->needs_open_file) {
        fd = open(TEST_FILE, O_CREAT | O_RDWR, 0700);
        if (fd < 0) {
            fprintf(stderr, "open %s: %s\n", TEST_FILE, strerror(errno));
            simple_umount_if_mounted(MNTPOINT);
            return EXIT_FAILURE;
        }
    }

    // 调用 mount，预期失败。
    errno = 0;
    int ret = mount(tc->device, tc->mntpoint, tc->fs_type, tc->flag, NULL);
    int saved_errno = errno;

    // 检查返回值和 errno，并保留原测例描述文本。
    int failed = 0;
    if (ret == -1 && saved_errno == tc->exp_errno) {
        printf("mounting %s failed as expected\n", tc->desc);
    } else {
        fprintf(stderr, "mounting %s failed unexpectedly; expected: %d - %s, got: %d - %s\n",
                tc->desc, tc->exp_errno, strerror(tc->exp_errno),
                saved_errno, strerror(saved_errno));
        failed = 1;
        if (ret == 0)
            simple_umount_if_mounted(tc->mntpoint);
    }

    // 清理当前场景的打开文件和预挂载。
    if (fd >= 0)
        close(fd);
    simple_umount_if_mounted(MNTPOINT);
    unlink(TEST_FILE);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
#endif

// 入口函数准备输入并遍历错误场景。
int main(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例需要的 Linux mount(2) 语义。
    fprintf(stderr, "SKIP: Linux mount(2) semantics are required\n");
    return EXIT_SKIP;
#else
    // mount 错误路径仍需要 root 权限才能越过权限检查。
    if (simple_require_root() != 0)
        return EXIT_SKIP;

    // 创建临时目录、挂载点和辅助输入。
    simple_enter_temp_dir(temp_dir);
    simple_mkdir_p(MNTPOINT, SIMPLE_DIR_MODE);
    setup_inputs();

    // 遍历所有错误场景。
    int failed = 0;
    for (size_t i = 0; i < sizeof(test_cases) / sizeof(test_cases[0]); i++) {
        if (run_case(&test_cases[i]) != EXIT_SUCCESS)
            failed = 1;
    }

    // 清理残留挂载和临时目录。
    simple_umount_if_mounted(MNTPOINT);
    simple_leave_temp_dir(temp_dir);
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#endif
}
