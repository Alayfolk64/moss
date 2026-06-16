// 本公共头为 umount 简化测例提供 Linux tmpfs 挂载、root 检查、临时目录和递归清理工具。

#ifndef UMOUNT_SIMPLE_COMMON_H
#define UMOUNT_SIMPLE_COMMON_H

// 启用 GNU/POSIX 扩展声明，保证 mkdtemp、mount、umount 等接口可见。
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#ifdef __linux__
#include <sys/mount.h>
#endif

// 用 2 表示当前环境不满足测例前提。
#define EXIT_SKIP 2

// 独立版默认用 tmpfs 替代 LTP 自动准备的测试设备。
#define SIMPLE_FS_TYPE "tmpfs"

// tmpfs 挂载时使用的 source 名称。
#define SIMPLE_FS_SOURCE "tmpfs"

// 递归删除普通文件、符号链接或空目录。
static inline void simple_remove_entry(const char *path)
{
    // 普通文件和符号链接优先使用 unlink。
    if (unlink(path) == 0)
        return;

    // 目录使用 rmdir。
    if (errno == EISDIR || errno == EPERM)
        rmdir(path);
}

// 递归删除目录树。
static inline void simple_remove_tree(const char *path)
{
    // 打不开目录时按普通条目删除。
    DIR *dir = opendir(path);
    if (!dir) {
        simple_remove_entry(path);
        return;
    }

    // 删除所有子条目。
    struct dirent *de;
    while ((de = readdir(dir)) != NULL) {
        if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, ".."))
            continue;

        char child[PATH_MAX];
        snprintf(child, sizeof(child), "%s/%s", path, de->d_name);
        simple_remove_tree(child);
    }

    // 删除目录本身。
    closedir(dir);
    rmdir(path);
}

// 创建并进入临时目录。
static inline void simple_enter_temp_dir(char *template)
{
    // mkdtemp 会生成唯一目录。
    if (mkdtemp(template) == NULL) {
        fprintf(stderr, "mkdtemp failed: %s\n", strerror(errno));
        exit(EXIT_FAILURE);
    }

    // 进入临时目录，使相对路径隔离。
    if (chdir(template) != 0) {
        fprintf(stderr, "chdir failed: %s\n", strerror(errno));
        exit(EXIT_FAILURE);
    }
}

// 离开并删除临时目录。
static inline void simple_leave_temp_dir(const char *path)
{
    // 先离开临时目录。
    chdir("/");

    // 再递归删除目录树。
    if (path)
        simple_remove_tree(path);
}

// 检查 root 权限。
static inline int simple_require_root(void)
{
    // umount 测试通常需要 CAP_SYS_ADMIN。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: root privileges are required\n");
        return -1;
    }

    return 0;
}

#ifdef __linux__
// 创建目录，如果已存在则视为成功。
static inline void simple_mkdir_p(const char *path, mode_t mode)
{
    // mkdir 成功或目录已存在都可以继续。
    if (mkdir(path, mode) != 0 && errno != EEXIST) {
        fprintf(stderr, "mkdir failed: %s\n", strerror(errno));
        exit(EXIT_FAILURE);
    }
}

// 挂载 tmpfs 到指定目录。
static inline int simple_mount_tmpfs(const char *target)
{
    // 使用 tmpfs 复现无需块设备的 umount 行为。
    return mount(SIMPLE_FS_SOURCE, target, SIMPLE_FS_TYPE, 0, NULL);
}

// 如果可能，卸载指定目录。
static inline void simple_umount_best_effort(const char *target)
{
    // 清理路径中不关心 umount 失败，避免覆盖主测试结果。
    if (target)
        umount(target);
}
#endif

#endif
