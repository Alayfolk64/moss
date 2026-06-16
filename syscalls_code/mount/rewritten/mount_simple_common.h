// 本公共头为 mount 简化测例提供 Linux mount 测试常用的临时目录、root 检查、挂载状态检查和递归清理工具。

#ifndef MOUNT_SIMPLE_COMMON_H
#define MOUNT_SIMPLE_COMMON_H

// 启用 GNU/POSIX 扩展声明，保证 mount、umount、mkdtemp 等接口可见。
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pwd.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#ifdef __linux__
#include <sys/mount.h>
#include <sys/statfs.h>
#endif

// 用 2 表示当前环境不满足测例前提。
#define EXIT_SKIP 2

// 独立版默认使用 tmpfs 替代 LTP 自动准备的块设备文件系统。
#define SIMPLE_FS_TYPE "tmpfs"

// 独立版默认传给 tmpfs 的 source 名称。
#define SIMPLE_FS_SOURCE "tmpfs"

// 统一创建测试目录时使用的权限。
#define SIMPLE_DIR_MODE 0755

// 在非 Linux 主机上跳过 Linux mount 语义测试。
static inline int simple_skip_nonlinux(void)
{
#ifndef __linux__
    // mount(2) 的五参数 Linux ABI 和这些 flag 语义在非 Linux 上不可复现。
    fprintf(stderr, "SKIP: Linux mount(2) semantics are required\n");
    return 1;
#else
    // Linux 主机可以继续执行测试。
    return 0;
#endif
}

// 检查是否具备 root 权限；mount/umount 大多数路径需要 CAP_SYS_ADMIN。
static inline int simple_require_root(void)
{
    // 非 root 情况下直接报告 skip，避免把权限前提误报为测试失败。
    if (geteuid() != 0) {
        fprintf(stderr, "SKIP: root privileges are required\n");
        return -1;
    }

    return 0;
}

// 打印 errno 失败消息并退出，调用点传入的字符串尽量保持原测例语义。
static inline void simple_die_errno(const char *msg)
{
    // 追加 errno 文本来替代原测试框架的 TERRNO/TTERRNO 输出。
    fprintf(stderr, "%s: %s\n", msg, strerror(errno));
    exit(EXIT_FAILURE);
}

// 持续写入直到缓冲区全部写完。
static inline int simple_write_full(int fd, const void *buf, size_t len)
{
    // 逐段推进写指针，处理短写。
    const char *p = (const char *)buf;
    while (len > 0) {
        ssize_t ret = write(fd, p, len);

        // write 失败时把 errno 留给调用者。
        if (ret < 0)
            return -1;

        // 返回 0 表示没有前进，按 I/O 错误处理。
        if (ret == 0) {
            errno = EIO;
            return -1;
        }

        // 更新剩余数据。
        p += ret;
        len -= (size_t)ret;
    }

    return 0;
}

// 创建并进入临时目录。
static inline void simple_enter_temp_dir(char *template)
{
    // mkdtemp 会原地生成唯一目录。
    if (mkdtemp(template) == NULL)
        simple_die_errno("mkdtemp failed");

    // 后续相对路径都落在该临时目录中。
    if (chdir(template) != 0)
        simple_die_errno("chdir failed");
}

// 删除普通文件或空目录，用于递归清理。
static inline void simple_remove_entry(const char *path)
{
    // 先尝试 unlink，适用于普通文件、符号链接和设备节点。
    if (unlink(path) == 0)
        return;

    // unlink 不适用于目录，再尝试 rmdir。
    if (errno == EISDIR || errno == EPERM)
        rmdir(path);
}

// 递归删除目录内容，不跨越挂载点的清理由调用者先 umount。
static inline void simple_remove_tree(const char *path)
{
    // 打开目录失败时尝试按普通条目删除。
    DIR *dir = opendir(path);
    if (!dir) {
        simple_remove_entry(path);
        return;
    }

    // 遍历目录下所有非 "." 和 ".." 条目。
    struct dirent *de;
    while ((de = readdir(dir)) != NULL) {
        if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, ".."))
            continue;

        // 拼接子路径并递归删除。
        char child[PATH_MAX];
        snprintf(child, sizeof(child), "%s/%s", path, de->d_name);
        simple_remove_tree(child);
    }

    // 关闭目录后删除目录本身。
    closedir(dir);
    rmdir(path);
}

// 离开并删除临时目录。
static inline void simple_leave_temp_dir(const char *path)
{
    // 先离开临时目录，避免当前工作目录阻止删除。
    chdir("/");

    // 尝试递归清理临时目录。
    if (path)
        simple_remove_tree(path);
}

#ifdef __linux__
// 判断目标路径当前是否是挂载点。
static inline int simple_is_mounted(const char *target)
{
    // 通过 /proc/self/mountinfo 判断，避免依赖外部 mount 命令。
    FILE *fp = fopen("/proc/self/mountinfo", "r");
    if (!fp)
        return 0;

    // 将相对路径转为绝对路径，便于匹配 mountinfo 中的挂载点字段。
    char abs_target[PATH_MAX];
    if (!realpath(target, abs_target)) {
        fclose(fp);
        return 0;
    }

    // mountinfo 第 5 个字段是挂载点路径。
    char line[4096];
    int mounted = 0;
    while (fgets(line, sizeof(line), fp)) {
        char *save = NULL;
        char *field = strtok_r(line, " ", &save);
        int index = 1;
        while (field) {
            if (index == 5) {
                if (!strcmp(field, abs_target))
                    mounted = 1;
                break;
            }
            field = strtok_r(NULL, " ", &save);
            index++;
        }
        if (mounted)
            break;
    }

    // 关闭 mountinfo 并返回检查结果。
    fclose(fp);
    return mounted;
}

// 如果目标已挂载，则尝试卸载它。
static inline void simple_umount_if_mounted(const char *target)
{
    // 只有 mountinfo 确认挂载时才调用 umount。
    if (simple_is_mounted(target) && umount(target) != 0)
        fprintf(stderr, "umount %s: %s\n", target, strerror(errno));
}

// 创建目录，如果目录已存在则视为成功。
static inline void simple_mkdir_p(const char *path, mode_t mode)
{
    // mkdir 成功或 EEXIST 都允许继续。
    if (mkdir(path, mode) != 0 && errno != EEXIST)
        simple_die_errno("mkdir failed");
}

// 挂载 tmpfs 到目标目录。
static inline int simple_mount_tmpfs(const char *target, unsigned long flags, const char *data)
{
    // 使用 tmpfs 复现无需块设备的 mount 行为。
    return mount(SIMPLE_FS_SOURCE, target, SIMPLE_FS_TYPE, flags, data);
}

// 创建一个文本文件，内容用于 bind/move mount 后验证可见性。
static inline void simple_write_text_file(const char *path, const char *text)
{
    // 以截断方式创建文件。
    int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0)
        simple_die_errno("open file failed");

    // 写入完整文本。
    if (simple_write_full(fd, text, strlen(text)) != 0) {
        close(fd);
        simple_die_errno("write file failed");
    }

    // 关闭文件描述符。
    if (close(fd) != 0)
        simple_die_errno("close file failed");
}
#endif

#endif
