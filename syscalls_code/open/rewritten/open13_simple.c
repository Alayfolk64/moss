// SPDX-License-Identifier: GPL-2.0-only
/*
 * open13 的简化转写版。
 *
 * 测试目标：
 *   O_PATH 打开的 fd 只能作为路径句柄使用；read/write/fchmod/fchown
 *   这类真正访问文件内容或属性的操作应失败并返回 EBADF。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define EXIT_SKIP 77

#ifdef O_PATH
static int check_ebadf(const char *name, int ret)
{
	if (ret == -1 && errno == EBADF) {
		printf("PASS: %s failed with EBADF\n", name);
		return 0;
	}

	printf("FAIL: %s expected EBADF but got %s\n",
	       name, ret == -1 ? strerror(errno) : "success");
	return 1;
}
#endif

int main(void)
{
#ifdef O_PATH
	char buf[8];
	int fd;
	int failed = 0;

	fd = open("open13_file", O_CREAT | O_RDWR | O_TRUNC, 0777);
	if (fd == -1) {
		perror("create open13_file");
		return EXIT_FAILURE;
	}
	close(fd);

	fd = open("open13_file", O_PATH);
	if (fd == -1) {
		perror("open O_PATH");
		unlink("open13_file");
		return EXIT_FAILURE;
	}

	errno = 0;
	failed += check_ebadf("read on O_PATH fd", read(fd, buf, sizeof(buf)));
	errno = 0;
	failed += check_ebadf("write on O_PATH fd", write(fd, "x", 1));
	errno = 0;
	failed += check_ebadf("fchmod on O_PATH fd", fchmod(fd, 0666));
	errno = 0;
	failed += check_ebadf("fchown on O_PATH fd", fchown(fd, 1000, 1000));

	close(fd);
	unlink("open13_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#else
	printf("SKIP: O_PATH is not available on this platform\n");
	return EXIT_SKIP;
#endif
}
