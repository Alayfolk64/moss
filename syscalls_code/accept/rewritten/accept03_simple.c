// 本测例验证 accept() 作用于非 socket 文件描述符时返回 ENOTSOCK，作用于没有关联 struct file 的无效/O_PATH fd 时返回 EBADF。

#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

// 临时普通文件模板。
static char temp_file[] = "/tmp/accept03_file_XXXXXX";

// 临时目录路径模板。
static char temp_dir[] = "/tmp/accept03_dir_XXXXXX";

// 对一个 fd 调用 accept() 并检查 errno。
static int check_accept(int fd, int exp_errno, const char *desc)
{
    // 准备普通 IPv4 sockaddr 输出缓冲区。
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = 0;
    addr.sin_addr.s_addr = htonl(INADDR_ANY);
    socklen_t size = sizeof(addr);

    // 清空 errno 后调用 accept()。
    errno = 0;
    int ret = accept(fd, (struct sockaddr *)&addr, &size);
    int saved_errno = errno;

    // 检查是否按预期失败。
    if (ret == -1 && saved_errno == exp_errno) {
        printf("accept() on %s\n", desc);
        return 0;
    }

    // accept() 意外成功时关闭新 fd。
    if (ret >= 0)
        close(ret);

    // 报告非预期结果。
    fprintf(stderr, "accept() on %s: returned %d, errno %d (expected %d)\n",
            desc, ret, saved_errno, exp_errno);
    return 1;
}

// 创建临时普通文件并返回 fd。
static int create_temp_file(void)
{
    // mkstemp 返回已打开的普通文件 fd。
    int fd = mkstemp(temp_file);
    if (fd < 0) {
        fprintf(stderr, "mkstemp failed: %s\n", strerror(errno));
        return -1;
    }
    return fd;
}

// 入口函数构造多类非 socket fd 并逐项调用 accept()。
int main(void)
{
    // 创建普通文件 fd，预期 ENOTSOCK。
    int failed = 0;
    int file_fd = create_temp_file();
    if (file_fd >= 0) {
        if (check_accept(file_fd, ENOTSOCK, "regular file") != 0)
            failed = 1;
        close(file_fd);
    } else {
        failed = 1;
    }

    // 创建目录 fd，预期 ENOTSOCK。
    if (mkdtemp(temp_dir) != NULL) {
        int dir_fd = open(temp_dir, O_RDONLY);
        if (dir_fd >= 0) {
            if (check_accept(dir_fd, ENOTSOCK, "directory") != 0)
                failed = 1;
            close(dir_fd);
        } else {
            fprintf(stderr, "open(directory) failed: %s\n", strerror(errno));
            failed = 1;
        }
    } else {
        fprintf(stderr, "mkdtemp failed: %s\n", strerror(errno));
        failed = 1;
    }

    // 创建 pipe read end，预期 ENOTSOCK。
    int pipe_fds[2];
    if (pipe(pipe_fds) == 0) {
        if (check_accept(pipe_fds[0], ENOTSOCK, "pipe read end") != 0)
            failed = 1;
        close(pipe_fds[0]);
        close(pipe_fds[1]);
    } else {
        fprintf(stderr, "pipe() failed: %s\n", strerror(errno));
        failed = 1;
    }

    // 使用已经关闭的 fd，预期 EBADF。
    int closed_fd = open(temp_file, O_RDONLY);
    if (closed_fd >= 0) {
        close(closed_fd);
        if (check_accept(closed_fd, EBADF, "closed file descriptor") != 0)
            failed = 1;
    } else {
        failed = 1;
    }

    // Linux O_PATH fd 没有关联可用于 accept() 的普通文件对象，预期 EBADF。
#ifdef O_PATH
    int path_fd = open(temp_file, O_PATH);
    if (path_fd >= 0) {
        if (check_accept(path_fd, EBADF, "O_PATH file") != 0)
            failed = 1;
        close(path_fd);
    }
#endif

    // 删除临时文件和目录。
    unlink(temp_file);
    rmdir(temp_dir);
    return failed ? 1 : 0;
}
