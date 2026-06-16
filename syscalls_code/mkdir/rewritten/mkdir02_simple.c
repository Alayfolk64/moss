// 本测例验证 mkdir 在父目录设置 S_ISGID 位时，新建子目录会继承父目录的组 ID 和 S_ISGID 位；运行需要 root，以便设置目录属组并临时切换到 nobody 用户。

// 启用 POSIX/GNU 接口声明，保证 mkdtemp、setreuid、setregid 等函数可见。
#define _GNU_SOURCE

#include <errno.h>
#include <grp.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

// 父目录名称，测试前会设置为带 S_ISGID 的目录。
#define TESTDIR1 "testdir1"

// 子目录名称，mkdir 会在这个路径上触发继承规则。
#define TESTDIR2 "testdir1/testdir2"

// 用于创建临时工作目录的模板，mkdtemp 会替换末尾的 X。
static char temp_dir[] = "/tmp/mkdir02_XXXXXX";

// 记录父目录使用的空闲组 ID，子进程会用它校验新目录属组。
static gid_t free_gid;

// 在常见普通用户组范围内寻找一个当前系统未使用的组 ID，用来模拟原测例的空闲组选择逻辑。
static gid_t find_unused_gid(gid_t avoid_gid)
{
    // 从较高的组号向下查找，降低碰到系统已有组的概率。
    for (gid_t gid = 60000; gid > 1000; gid--) {
        // 跳过 nobody 自己的组，确保继承结果和进程当前组不同。
        if (gid == avoid_gid)
            continue;

        // getgrgid 返回 NULL 表示该组号没有在组数据库中注册。
        errno = 0;
        if (getgrgid(gid) == NULL && errno == 0)
            return gid;
    }

    // 如果没有找到理想空闲组号，就返回一个不同于 nobody 组的保底值。
    return avoid_gid == 65534 ? 65533 : 65534;
}

// 清理测试过程中创建的目录，父进程保持 root 权限，因此可以删除子进程留下的内容。
static void cleanup(void)
{
    // 删除可能已经创建的子目录，失败时忽略，便于处理部分初始化失败的路径。
    rmdir(TESTDIR2);

    // 删除父目录，失败时忽略，避免清理逻辑掩盖真正的测试结果。
    rmdir(TESTDIR1);

    // 离开临时目录后再删除它本身。
    if (temp_dir[0] != '\0') {
        chdir("/");
        rmdir(temp_dir);
    }
}

// 子进程执行真正的 mkdir 检查，确认新目录继承了父目录组 ID 和 S_ISGID 位。
static int verify_mkdir_as_nobody(void)
{
    // 创建子目录，预期内核会根据父目录的 S_ISGID 规则设置属组和模式位。
    if (mkdir(TESTDIR2, 0777) != 0) {
        fprintf(stderr, "FAIL: mkdir(%s) 失败: %s\n", TESTDIR2, strerror(errno));
        return 1;
    }

    // 读取新目录属性，为后续的属组和 S_ISGID 检查做准备。
    struct stat st;
    if (stat(TESTDIR2, &st) != 0) {
        fprintf(stderr, "FAIL: stat(%s) 失败: %s\n", TESTDIR2, strerror(errno));
        return 1;
    }

    // 检查新目录的组 ID 是否继承自父目录。
    int failed = 0;
    if (st.st_gid != free_gid) {
        fprintf(stderr, "FAIL: 新目录 gid=%ld，期望 gid=%ld\n",
                (long)st.st_gid, (long)free_gid);
        failed = 1;
    }

    // 检查新目录是否继承了 S_ISGID 位。
    if ((st.st_mode & S_ISGID) == 0) {
        fprintf(stderr, "FAIL: 新目录没有继承 S_ISGID 位\n");
        failed = 1;
    }

    // 尽量删除子目录，让测试目录保持干净。
    if (rmdir(TESTDIR2) != 0)
        fprintf(stderr, "WARN: rmdir(%s) 失败: %s\n", TESTDIR2, strerror(errno));

    // 根据两项检查的结果返回最终状态。
    if (failed)
        return 1;

    printf("PASS: mkdir 创建的新目录继承了父目录 gid 和 S_ISGID 位\n");
    return 0;
}

// fork 子进程并把它切换到 nobody 身份，模拟原 LTP 测例中的非特权创建者。
static int run_child(uid_t nobody_uid, gid_t nobody_gid)
{
    // 创建子进程，父进程保留 root 权限负责清理。
    pid_t pid = fork();
    if (pid < 0) {
        fprintf(stderr, "FAIL: fork 失败: %s\n", strerror(errno));
        return 1;
    }

    // 子进程先切换组，再切换用户，随后执行 mkdir 行为检查。
    if (pid == 0) {
        if (setregid(nobody_gid, nobody_gid) != 0) {
            fprintf(stderr, "FAIL: setregid(nobody) 失败: %s\n", strerror(errno));
            fflush(NULL);
            _exit(1);
        }

        if (setreuid(nobody_uid, nobody_uid) != 0) {
            fprintf(stderr, "FAIL: setreuid(nobody) 失败: %s\n", strerror(errno));
            fflush(NULL);
            _exit(1);
        }

        int rc = verify_mkdir_as_nobody();
        fflush(NULL);
        _exit(rc);
    }

    // 父进程等待子进程结束，并把子进程退出码作为测例结果。
    int status;
    if (waitpid(pid, &status, 0) < 0) {
        fprintf(stderr, "FAIL: waitpid 失败: %s\n", strerror(errno));
        return 1;
    }

    // 将异常退出也视作测试失败。
    if (!WIFEXITED(status))
        return 1;

    return WEXITSTATUS(status);
}

// 入口函数负责 root 检查、目录准备、执行子进程测试和清理。
int main(void)
{
    // 原始 LTP 测例需要 root；这里显式检查，避免权限不足时得到误导性失败。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: 需要 root 才能 chown 目录并切换到 nobody 用户\n");
        return 2;
    }

    // 获取 nobody 用户信息，用于模拟非特权用户创建目录。
    struct passwd *pw = getpwnam("nobody");
    if (pw == NULL) {
        fprintf(stderr, "SKIP: 系统没有 nobody 用户，无法复现原测例\n");
        return 2;
    }

    // 选择一个未使用的组 ID，让继承结果更容易和进程当前组区分。
    free_gid = find_unused_gid(pw->pw_gid);

    // 创建独立临时目录，避免测试文件污染当前工作目录。
    if (mkdtemp(temp_dir) == NULL) {
        fprintf(stderr, "FAIL: mkdtemp 失败: %s\n", strerror(errno));
        return 1;
    }

    // 进入临时目录后，后续相对路径都落在这个隔离区域中。
    if (chdir(temp_dir) != 0) {
        fprintf(stderr, "FAIL: chdir(%s) 失败: %s\n", temp_dir, strerror(errno));
        cleanup();
        return 1;
    }

    // 清空 umask，避免权限位被进程默认 umask 意外遮掉。
    umask(0);

    // 创建父目录并设置属组，然后补上 0777 和 S_ISGID 位。
    if (mkdir(TESTDIR1, 0777) != 0 ||
        chown(TESTDIR1, getuid(), free_gid) != 0 ||
        chmod(TESTDIR1, 0777 | S_ISGID) != 0) {
        fprintf(stderr, "FAIL: 准备父目录失败: %s\n", strerror(errno));
        cleanup();
        return 1;
    }

    // 执行子进程测试，随后由父进程统一清理目录。
    int rc = run_child(pw->pw_uid, pw->pw_gid);
    cleanup();
    return rc;
}
