// 本公共头为 mmap 简化测例提供少量通用工具，避免每个独立 C 文件重复编写页大小、写满文件、临时目录和错误格式化逻辑。

#ifndef MMAP_SIMPLE_COMMON_H
#define MMAP_SIMPLE_COMMON_H

// 启用 GNU/POSIX 扩展声明，保证 Linux 上的 mmap 相关接口和常量尽量可见。
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

// macOS 等系统没有 MAP_FILE；Linux 中它等价于 0。
#ifndef MAP_FILE
#define MAP_FILE 0
#endif

// macOS 使用 MAP_ANON 名称，这里补齐 MAP_ANONYMOUS 以便源码统一。
#ifndef MAP_ANONYMOUS
#ifdef MAP_ANON
#define MAP_ANONYMOUS MAP_ANON
#endif
#endif

// 旧系统头可能没有 MAP_FIXED_NOREPLACE，Linux 中该值用于不替换已有映射。
#ifndef MAP_FIXED_NOREPLACE
#define MAP_FIXED_NOREPLACE 0x100000
#endif

// 旧系统头可能没有 MAP_SHARED_VALIDATE，Linux 中该值要求严格校验未知 flag。
#ifndef MAP_SHARED_VALIDATE
#define MAP_SHARED_VALIDATE 0x03
#endif

// 非 Linux 系统通常没有 MAP_POPULATE；补值只为编译，运行时会跳过 Linux 专属检查。
#ifndef MAP_POPULATE
#define MAP_POPULATE 0x8000
#endif

// 非 Linux 系统通常没有 MAP_LOCKED；补值只为编译，运行时会跳过 Linux 专属检查。
#ifndef MAP_LOCKED
#define MAP_LOCKED 0x2000
#endif

// 非 Linux 系统通常没有 MAP_GROWSDOWN；补值只为编译，运行时会跳过 Linux 专属检查。
#ifndef MAP_GROWSDOWN
#define MAP_GROWSDOWN 0x0100
#endif

// 用 2 表示环境不满足或能力缺失，和本目录其他简化测例的 skip 风格保持一致。
#define EXIT_SKIP 2

// 获取系统页大小；如果系统调用失败，直接用保留原意的错误文本退出。
static inline long simple_page_size(void)
{
    // sysconf 返回页大小，mmap 系列测例大多按页构造长度和偏移。
    long page_size = sysconf(_SC_PAGESIZE);

    // 页大小不可用时无法继续构造 mmap 参数。
    if (page_size <= 0) {
        fprintf(stderr, "sysconf(_SC_PAGESIZE): %s\n", strerror(errno));
        exit(EXIT_FAILURE);
    }

    return page_size;
}

// 打印带 errno 的失败信息，并终止当前测例。
static inline void simple_die_errno(const char *message)
{
    // message 保持调用点传入的原始 LTP 文本，errno 作为框架替代信息追加。
    fprintf(stderr, "%s: %s\n", message, strerror(errno));
    exit(EXIT_FAILURE);
}

// 打印不带 errno 的失败信息，并终止当前测例。
static inline void simple_die(const char *message)
{
    // 用原始消息文本直接报告失败。
    fprintf(stderr, "%s\n", message);
    exit(EXIT_FAILURE);
}

// 持续写入直到缓冲区全部写完，用来替代原框架中的“写满缓冲区”封装。
static inline int simple_write_full(int fd, const void *buf, size_t len)
{
    // 逐段推进写指针，处理短写。
    const char *p = (const char *)buf;
    while (len > 0) {
        ssize_t ret = write(fd, p, len);

        // 写入出错时把 errno 留给调用者。
        if (ret < 0)
            return -1;

        // write 返回 0 表示无法前进，按 I/O 错误处理。
        if (ret == 0) {
            errno = EIO;
            return -1;
        }

        // 更新剩余待写数据。
        p += ret;
        len -= (size_t)ret;
    }

    return 0;
}

// 把一个文件截断到指定大小，并用固定字节填充内容。
static inline int simple_fill_fd(int fd, int byte, size_t size)
{
    // 从文件开头写入，保证测试读取到可预测内容。
    if (lseek(fd, 0, SEEK_SET) == -1)
        return -1;

    // 使用小块缓冲反复写入，避免为大文件分配过多内存。
    char buf[4096];
    memset(buf, byte, sizeof(buf));
    while (size > 0) {
        size_t chunk = size < sizeof(buf) ? size : sizeof(buf);
        if (simple_write_full(fd, buf, chunk) != 0)
            return -1;
        size -= chunk;
    }

    // 截断到当前偏移，保证文件大小正好等于请求大小。
    return ftruncate(fd, lseek(fd, 0, SEEK_CUR));
}

// 创建并进入临时目录，所有相对路径测试文件都落在隔离目录里。
static inline void simple_enter_temp_dir(char *template)
{
    // mkdtemp 会原地替换模板末尾的 X，生成唯一目录。
    if (mkdtemp(template) == NULL)
        simple_die_errno("mkdtemp failed");

    // 进入临时目录，使后续文件名和原 LTP 的 needs_tmpdir 行为一致。
    if (chdir(template) != 0)
        simple_die_errno("chdir failed");
}

// 离开并删除临时目录。
static inline void simple_leave_temp_dir(const char *path)
{
    // 先切回根目录，避免当前工作目录阻止 rmdir。
    chdir("/");

    // 删除临时目录失败只打印警告，不覆盖主要测试结果。
    if (path && rmdir(path) != 0)
        fprintf(stderr, "rmdir %s: %s\n", path, strerror(errno));
}

// 根据 wait status 生成短文本，用于保留原测例里打印子进程状态的位置。
static inline const char *simple_strstatus(int status, char *buf, size_t buflen)
{
    // 正常退出时打印退出码。
    if (WIFEXITED(status)) {
        snprintf(buf, buflen, "exited, status=%d", WEXITSTATUS(status));
        return buf;
    }

    // 被信号终止时打印信号号和系统信号名。
    if (WIFSIGNALED(status)) {
        snprintf(buf, buflen, "killed by signal %d (%s)",
                 WTERMSIG(status), strsignal(WTERMSIG(status)));
        return buf;
    }

    // 其他 wait 状态用十六进制形式保留调试信息。
    snprintf(buf, buflen, "status=0x%x", status);
    return buf;
}

// 返回信号名，用于保留原测例里打印信号名称的位置。
static inline const char *simple_strsig(int sig)
{
    // strsignal 返回当前系统的信号描述字符串。
    const char *name = strsignal(sig);

    // 极端情况下没有信号名时，使用 UNKNOWN 作为兜底。
    return name ? name : "UNKNOWN";
}

// 统计数组元素个数。
#define SIMPLE_ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))

#endif
