// 本测例验证 mkdir 在多种错误路径下会失败并设置预期 errno，包括 EFAULT、ENAMETOOLONG、EEXIST、ENOENT、ENOTDIR、ELOOP 和 EROFS；EROFS 分支需要 root 和 mount 权限。

// 启用 POSIX/GNU 接口声明，保证 mkdtemp、symlink、mount 等函数可见。
#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#ifdef __linux__
#include <sys/mount.h>
#endif

// mkdir 测试统一使用的目录权限。
#define MODE 0777

// 已存在的普通文件路径，用于触发 EEXIST。
#define PATH_EXIST_FILE "exist_file"

// 已存在的 FIFO 路径，用于触发 EEXIST。
#define PATH_EXIST_FIFO "exist_fifo"

// 已存在的目录路径，用于触发 EEXIST。
#define PATH_EXIST_DIR "exist_dir"

// 已存在的符号链接路径，用于触发 EEXIST。
#define PATH_EXIST_SYMLINK "exist_symlink"

// 已存在的设备文件路径，用于触发 EEXIST。
#define PATH_NULLDEV "/dev/null"

// 父目录不存在的路径，用于触发 ENOENT。
#define PATH_MISSING_PARENT "missing_parent/child"

// 已存在的普通文件，会被当作路径中的父组件。
#define PATH_NOTDIR_FILE "notdir_file"

// 父组件不是目录的路径，用于触发 ENOTDIR。
#define PATH_NOTDIR_CHILD "notdir_file/child"

// 只读挂载点名称，用于触发 EROFS。
#define MNT_POINT "mntpoint"

// 只读挂载点内部的待创建目录路径。
#define PATH_ROFS_CHILD "mntpoint/rofs_child"

// 构造一个明显非法的用户态地址，用于触发 EFAULT。
#define BAD_PATH ((const char *)-1)

// 计算静态数组元素个数，便于遍历测试用例表。
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))

// 用于创建临时工作目录的模板，mkdtemp 会替换末尾的 X。
static char temp_dir[] = "/tmp/mkdir03_XXXXXX";

// 超长路径缓冲区，长度超过 PATH_MAX，用于触发 ENAMETOOLONG。
static char long_dir[PATH_MAX + 2];

// 符号链接循环路径缓冲区，用于触发 ELOOP。
static char loop_dir[PATH_MAX];

// 记录只读文件系统是否挂载成功，清理时据此决定是否 umount。
static int rofs_mounted;

// 单个 mkdir 错误分支的路径、预期 errno 和说明。
struct test_case {
    const char *desc;
    const char *pathname;
    int expected_errno;
    int needs_rofs;
};

// 覆盖原 LTP 测例中列出的 mkdir 错误场景。
static struct test_case cases[] = {
    {"非法用户地址", BAD_PATH, EFAULT, 0},
    {"路径名过长", long_dir, ENAMETOOLONG, 0},
    {"普通文件已存在", PATH_EXIST_FILE, EEXIST, 0},
    {"目录已存在", PATH_EXIST_DIR, EEXIST, 0},
    {"FIFO 已存在", PATH_EXIST_FIFO, EEXIST, 0},
    {"符号链接已存在", PATH_EXIST_SYMLINK, EEXIST, 0},
    {"设备文件已存在", PATH_NULLDEV, EEXIST, 0},
    {"父目录不存在", PATH_MISSING_PARENT, ENOENT, 0},
    {"父组件不是目录", PATH_NOTDIR_CHILD, ENOTDIR, 0},
    {"符号链接展开形成循环", loop_dir, ELOOP, 0},
    {"只读文件系统", PATH_ROFS_CHILD, EROFS, 1},
};

// 创建一个普通文件，用于准备 EEXIST 或 ENOTDIR 场景。
static int touch_file(const char *path)
{
    // 使用 open 创建文件，并立即关闭描述符。
    int fd = open(path, O_CREAT | O_WRONLY, 0666);
    if (fd < 0) {
        fprintf(stderr, "FAIL: 创建文件 %s 失败: %s\n", path, strerror(errno));
        return -1;
    }

    // 关闭文件描述符，避免泄漏测试资源。
    if (close(fd) != 0) {
        fprintf(stderr, "FAIL: close(%s) 失败: %s\n", path, strerror(errno));
        return -1;
    }

    return 0;
}

// 构造一个会反复经过符号链接的长路径，用于触发 ELOOP。
static int build_loop_path(void)
{
    // 从当前目录开始，然后不断追加会进入符号链接环的路径组件。
    strcpy(loop_dir, ".");
    for (int i = 0; i < 43; i++) {
        // 手动追加路径组件，避免依赖非 Linux 标准库里的 strlcat。
        size_t used = strlen(loop_dir);
        const char segment[] = "/test_eloop";
        if (used + sizeof(segment) > sizeof(loop_dir)) {
            fprintf(stderr, "FAIL: ELOOP 路径超过缓冲区\n");
            return -1;
        }
        memcpy(loop_dir + used, segment, sizeof(segment));
    }

    return 0;
}

// 准备只读 tmpfs 挂载点，用于触发 mkdir 的 EROFS。
static int setup_readonly_mount(void)
{
#ifndef __linux__
    // 非 Linux 主机没有本测例使用的 mount(2) tmpfs 只读挂载接口。
    fprintf(stderr, "SKIP: EROFS 分支需要 Linux mount(2) 支持\n");
    return 2;
#else
    // 非 root 或缺少挂载能力时，无法可靠复现 EROFS。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: EROFS 分支需要 root 和 mount 权限\n");
        return 2;
    }

    // 创建挂载点目录，后续把 tmpfs 以只读方式挂到这里。
    if (mkdir(MNT_POINT, 0755) != 0) {
        fprintf(stderr, "FAIL: 创建挂载点失败: %s\n", strerror(errno));
        return -1;
    }

    // 挂载只读 tmpfs，使 mkdir 在其中创建目录时返回 EROFS。
    if (mount("tmpfs", MNT_POINT, "tmpfs", MS_RDONLY, "size=4m") != 0) {
        fprintf(stderr, "SKIP: mount 只读 tmpfs 失败: %s\n", strerror(errno));
        return 2;
    }

    rofs_mounted = 1;
    return 0;
#endif
}

// 清理所有可能创建的文件、目录、符号链接和挂载点。
static void cleanup(void)
{
    // 如果只读文件系统已经挂载，必须先卸载再删除挂载点目录。
    if (rofs_mounted) {
#ifdef __linux__
        if (umount(MNT_POINT) != 0)
            fprintf(stderr, "WARN: umount(%s) 失败: %s\n", MNT_POINT, strerror(errno));
#endif
        rofs_mounted = 0;
    }

    // 删除普通文件、FIFO 和符号链接。
    unlink(PATH_EXIST_FILE);
    unlink(PATH_EXIST_FIFO);
    unlink(PATH_EXIST_SYMLINK);
    unlink(PATH_NOTDIR_FILE);

    // 删除符号链接循环相关条目。
    unlink("test_eloop/test_eloop");
    rmdir("test_eloop");

    // 删除测试目录和挂载点目录。
    rmdir(PATH_EXIST_DIR);
    rmdir(MNT_POINT);

    // 离开临时目录后删除临时目录本身。
    if (temp_dir[0] != '\0') {
        chdir("/");
        rmdir(temp_dir);
    }
}

// 准备 mkdir03 所需的所有错误场景输入。
static int setup(void)
{
    // 创建并进入独立临时目录，所有相对路径都在这里生成。
    if (mkdtemp(temp_dir) == NULL) {
        fprintf(stderr, "FAIL: mkdtemp 失败: %s\n", strerror(errno));
        return -1;
    }
    if (chdir(temp_dir) != 0) {
        fprintf(stderr, "FAIL: chdir(%s) 失败: %s\n", temp_dir, strerror(errno));
        return -1;
    }

    // 构造超过 PATH_MAX 的字符串，最后保留 NUL 终止符。
    memset(long_dir, 'a', sizeof(long_dir) - 1);
    long_dir[sizeof(long_dir) - 1] = '\0';

    // 准备会导致 EEXIST 的普通文件、目录、FIFO 和符号链接。
    if (touch_file(PATH_EXIST_FILE) != 0 ||
        mkdir(PATH_EXIST_DIR, 0777) != 0 ||
        mkfifo(PATH_EXIST_FIFO, 0777) != 0 ||
        symlink(temp_dir, PATH_EXIST_SYMLINK) != 0) {
        fprintf(stderr, "FAIL: 准备 EEXIST 场景失败: %s\n", strerror(errno));
        return -1;
    }

    // 准备 ENOTDIR 场景：路径中的父组件实际是普通文件。
    if (touch_file(PATH_NOTDIR_FILE) != 0)
        return -1;

    // 准备 ELOOP 场景：目录内的同名符号链接指回上一层目录。
    if (mkdir("test_eloop", 0755) != 0 ||
        symlink("../test_eloop", "test_eloop/test_eloop") != 0 ||
        build_loop_path() != 0) {
        fprintf(stderr, "FAIL: 准备 ELOOP 场景失败: %s\n", strerror(errno));
        return -1;
    }

    // 准备 EROFS 场景；失败时允许后续用例继续执行，并把 EROFS 记为跳过。
    int rofs_rc = setup_readonly_mount();
    if (rofs_rc < 0)
        return -1;

    return rofs_rc;
}

// 执行单个 mkdir 错误场景，并检查返回值和 errno。
static int run_case(const struct test_case *tc)
{
    // 如果只读挂载不可用，明确跳过 EROFS 分支。
    if (tc->needs_rofs && !rofs_mounted) {
        printf("SKIP: %s 需要可用的只读挂载点\n", tc->desc);
        return 2;
    }

    // 清空 errno，随后调用 mkdir 触发当前错误条件。
    errno = 0;
    int ret = mkdir(tc->pathname, MODE);
    int saved_errno = errno;

    // mkdir 在这些场景下都应该失败。
    if (ret != -1) {
        fprintf(stderr, "FAIL: %s: mkdir 意外成功\n", tc->desc);
        rmdir(tc->pathname);
        return 1;
    }

    // 检查 errno 是否和原 LTP 测例期望一致。
    if (saved_errno != tc->expected_errno) {
        fprintf(stderr, "FAIL: %s: errno=%d (%s)，期望 errno=%d (%s)\n",
                tc->desc, saved_errno, strerror(saved_errno),
                tc->expected_errno, strerror(tc->expected_errno));
        return 1;
    }

    // 当前错误分支符合预期。
    printf("PASS: %s -> errno=%d (%s)\n",
           tc->desc, saved_errno, strerror(saved_errno));
    return 0;
}

// 入口函数负责准备场景、逐项执行 mkdir 错误检查并清理资源。
int main(void)
{
    // 初始化所有测试输入；返回 2 表示只有 EROFS 分支不可用。
    int setup_rc = setup();
    if (setup_rc < 0) {
        cleanup();
        return 1;
    }

    // 逐项运行错误场景，并累计失败和跳过数量。
    int failed = 0;
    int skipped = setup_rc == 2 ? 1 : 0;
    for (size_t i = 0; i < ARRAY_SIZE(cases); i++) {
        int rc = run_case(&cases[i]);
        if (rc == 1)
            failed++;
        else if (rc == 2)
            skipped++;
    }

    // 清理临时目录、挂载点和测试文件。
    cleanup();

    // 失败优先返回 1；仅有不支持场景时返回 2，表示测试环境不完整。
    if (failed)
        return 1;
    if (skipped)
        return 2;
    return 0;
}
