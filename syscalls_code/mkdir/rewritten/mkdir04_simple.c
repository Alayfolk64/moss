// 本测例验证非目录所有者不能在 0700 权限目录内 mkdir，预期 mkdir 失败并设置 errno 为 EACCES；运行需要 root，以便准备两个不同用户并切换身份。

// 启用 POSIX/GNU 接口声明，保证 mkdtemp、setreuid 等函数可见。
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

// 受限父目录名称，权限会设置为 0700。
#define TESTDIR "testdir"

// 非所有者尝试创建的子目录路径。
#define TESTSUBDIR "testdir/testdir"

// 用于创建临时工作目录的模板，mkdtemp 会替换末尾的 X。
static char temp_dir[] = "/tmp/mkdir04_XXXXXX";

// 清理测试目录，父进程保持 root 权限，因此可以删除受限目录。
static void cleanup(void)
{
    // 删除可能意外创建成功的子目录。
    rmdir(TESTSUBDIR);

    // 删除受限父目录。
    rmdir(TESTDIR);

    // 离开临时目录后删除临时目录本身。
    if (temp_dir[0] != '\0') {
        chdir("/");
        rmdir(temp_dir);
    }
}

// 从 passwd 数据库中找两个不同的非 root 用户，模拟原测例的用户选择逻辑。
static int find_two_users(uid_t users[2])
{
    // 遍历系统用户表，跳过 root 和重复 UID。
    int count = 0;
    setpwent();
    for (struct passwd *pw = getpwent(); pw != NULL; pw = getpwent()) {
        if (pw->pw_uid == 0)
            continue;
        if (count == 1 && pw->pw_uid == users[0])
            continue;
        users[count++] = pw->pw_uid;
        if (count == 2)
            break;
    }
    endpwent();

    // 至少需要两个不同非 root 用户，才能让目录所有者和访问者不同。
    return count == 2 ? 0 : -1;
}

// 子进程以第二个非 root 用户身份执行 mkdir，并检查 EACCES。
static int verify_mkdir_denied(uid_t user)
{
    // 切换到无权限用户，确保后续 mkdir 不具备目录所有者权限。
    if (setreuid(user, user) != 0) {
        fprintf(stderr, "FAIL: setreuid(%ld) 失败: %s\n", (long)user, strerror(errno));
        return 1;
    }

    // 调用 mkdir，预期因为父目录 0700 且所有者不同而失败。
    errno = 0;
    if (mkdir(TESTSUBDIR, 0777) != -1) {
        fprintf(stderr, "FAIL: mkdir(%s) 意外成功\n", TESTSUBDIR);
        return 1;
    }

    // 检查 errno 是否是权限拒绝 EACCES。
    if (errno != EACCES) {
        fprintf(stderr, "FAIL: errno=%d (%s)，期望 EACCES\n", errno, strerror(errno));
        return 1;
    }

    // mkdir 按预期被权限检查拒绝。
    printf("PASS: 非所有者在 0700 目录内 mkdir 返回 EACCES\n");
    return 0;
}

// fork 子进程执行权限检查，父进程等待并保留 root 权限用于清理。
static int run_child(uid_t user)
{
    // 创建子进程，避免主进程永久丢失 root 权限。
    pid_t pid = fork();
    if (pid < 0) {
        fprintf(stderr, "FAIL: fork 失败: %s\n", strerror(errno));
        return 1;
    }

    // 子进程切换身份并执行 mkdir 权限测例。
    if (pid == 0) {
        int rc = verify_mkdir_denied(user);
        fflush(NULL);
        _exit(rc);
    }

    // 父进程等待子进程返回结果。
    int status;
    if (waitpid(pid, &status, 0) < 0) {
        fprintf(stderr, "FAIL: waitpid 失败: %s\n", strerror(errno));
        return 1;
    }

    // 异常退出说明测例没有正常完成。
    if (!WIFEXITED(status))
        return 1;

    return WEXITSTATUS(status);
}

// 入口函数负责准备受限目录、启动非所有者子进程并清理资源。
int main(void)
{
    // 原始测例需要 root；这里显式检查，避免无法 chown 或切换用户。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: 需要 root 才能 chown 目录并切换用户\n");
        return 2;
    }

    // 查找两个不同的非 root 用户，分别作为目录所有者和访问者。
    uid_t users[2];
    if (find_two_users(users) != 0) {
        fprintf(stderr, "SKIP: 需要两个不同的非 root 用户才能复现 EACCES 场景\n");
        return 2;
    }

    // 创建并进入临时目录，避免测试文件污染当前目录。
    if (mkdtemp(temp_dir) == NULL) {
        fprintf(stderr, "FAIL: mkdtemp 失败: %s\n", strerror(errno));
        return 1;
    }
    if (chmod(temp_dir, 0755) != 0 || chdir(temp_dir) != 0) {
        fprintf(stderr, "FAIL: 准备临时目录失败: %s\n", strerror(errno));
        cleanup();
        return 1;
    }

    // 创建 0700 父目录，并把它的所有者设置为第一个非 root 用户。
    if (mkdir(TESTDIR, 0700) != 0 ||
        chown(TESTDIR, users[0], getgid()) != 0) {
        fprintf(stderr, "FAIL: 准备受限目录失败: %s\n", strerror(errno));
        cleanup();
        return 1;
    }

    // 第二个非 root 用户尝试在受限目录内创建子目录。
    int rc = run_child(users[1]);
    cleanup();
    return rc;
}
