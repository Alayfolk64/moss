// 本测例验证 mkdir 成功创建目录后，新目录的所有者 UID 和 GID 分别等于进程当时的有效 UID 和有效 GID；运行需要 root，以便切换到 nobody 用户。

// 启用 POSIX/GNU 接口声明，保证 mkdtemp、setuid 等函数可见。
#define _GNU_SOURCE

#include <errno.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

// mkdir 创建目录时使用的权限位。
#define PERMS 0777

// 待创建并检查属主属组的目录名称。
#define TESTDIR "testdir"

// 用于创建临时工作目录的模板，mkdtemp 会替换末尾的 X。
static char temp_dir[] = "/tmp/mkdir05_XXXXXX";

// 清理测试目录，父进程保持 root 权限，因此可以删除 nobody 创建的目录。
static void cleanup(void)
{
    // 删除测试目录，失败时忽略，便于处理创建失败路径。
    rmdir(TESTDIR);

    // 离开临时目录后删除临时目录本身。
    if (temp_dir[0] != '\0') {
        chdir("/");
        rmdir(temp_dir);
    }
}

// 子进程切换到 nobody 后创建目录，并检查目录 UID/GID 是否来自进程有效身份。
static int verify_owner_after_mkdir(uid_t nobody_uid)
{
    // 只切换 UID，保留当前有效 GID，用来复现原 LTP 测例的身份变化方式。
    if (setuid(nobody_uid) != 0) {
        fprintf(stderr, "FAIL: setuid(nobody) 失败: %s\n", strerror(errno));
        return 1;
    }

    // 调用 mkdir 创建新目录，预期操作成功。
    if (mkdir(TESTDIR, PERMS) != 0) {
        fprintf(stderr, "FAIL: mkdir(%s) 失败: %s\n", TESTDIR, strerror(errno));
        return 1;
    }

    // 读取目录属性，用来和当前进程有效 UID/GID 比较。
    struct stat st;
    if (stat(TESTDIR, &st) != 0) {
        fprintf(stderr, "FAIL: stat(%s) 失败: %s\n", TESTDIR, strerror(errno));
        return 1;
    }

    // 检查目录所有者 UID 是否等于进程有效 UID。
    int failed = 0;
    if (st.st_uid != geteuid()) {
        fprintf(stderr, "FAIL: st_uid=%ld，期望 geteuid=%ld\n",
                (long)st.st_uid, (long)geteuid());
        failed = 1;
    }

    // 检查目录所有者 GID 是否等于进程有效 GID。
    if (st.st_gid != getegid()) {
        fprintf(stderr, "FAIL: st_gid=%ld，期望 getegid=%ld\n",
                (long)st.st_gid, (long)getegid());
        failed = 1;
    }

    // 根据 UID/GID 检查结果报告测例状态。
    if (failed)
        return 1;

    printf("PASS: mkdir 创建目录的 UID/GID 等于进程有效 UID/GID\n");
    return 0;
}

// fork 子进程执行 nobody 身份下的 mkdir，父进程保留 root 权限负责清理。
static int run_child(uid_t nobody_uid)
{
    // 创建子进程，避免主进程永久切换到 nobody。
    pid_t pid = fork();
    if (pid < 0) {
        fprintf(stderr, "FAIL: fork 失败: %s\n", strerror(errno));
        return 1;
    }

    // 子进程切换身份并执行目录属主属组检查。
    if (pid == 0) {
        int rc = verify_owner_after_mkdir(nobody_uid);
        fflush(NULL);
        _exit(rc);
    }

    // 父进程等待子进程结束，并读取退出码。
    int status;
    if (waitpid(pid, &status, 0) < 0) {
        fprintf(stderr, "FAIL: waitpid 失败: %s\n", strerror(errno));
        return 1;
    }

    // 异常退出说明测试没有正常完成。
    if (!WIFEXITED(status))
        return 1;

    return WEXITSTATUS(status);
}

// 入口函数负责 root 检查、临时目录准备、执行子进程测试和清理。
int main(void)
{
    // 原始 LTP 测例需要 root；这里显式检查，避免无法切换到 nobody。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: 需要 root 才能切换到 nobody 用户\n");
        return 2;
    }

    // 获取 nobody 用户，用作创建目录时的有效 UID。
    struct passwd *pw = getpwnam("nobody");
    if (pw == NULL) {
        fprintf(stderr, "SKIP: 系统没有 nobody 用户，无法复现原测例\n");
        return 2;
    }

    // 创建并进入临时目录，给 nobody 用户开放写权限。
    if (mkdtemp(temp_dir) == NULL) {
        fprintf(stderr, "FAIL: mkdtemp 失败: %s\n", strerror(errno));
        return 1;
    }
    if (chmod(temp_dir, 0777) != 0 || chdir(temp_dir) != 0) {
        fprintf(stderr, "FAIL: 准备临时目录失败: %s\n", strerror(errno));
        cleanup();
        return 1;
    }

    // 执行子进程测试，随后由父进程统一清理。
    int rc = run_child(pw->pw_uid);
    cleanup();
    return rc;
}
