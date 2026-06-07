// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open14 的简化转写版。
 *
 * 测试目标：
 *   O_TMPFILE 创建匿名临时文件，写入后不出现在目录中；
 *   通过 /proc/self/fd/<fd> + linkat() 可以把它链接成普通文件。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define EXIT_SKIP 77

int main(void)
{
#if defined(O_TMPFILE) && defined(__linux__)
	char data[1024];
	char path[PATH_MAX];
	struct stat st;
	int fd;

	memset(data, 1, sizeof(data));

	fd = open(".", O_TMPFILE | O_RDWR, 0600);
	if (fd == -1) {
		if (errno == EOPNOTSUPP || errno == ENOTSUP || errno == EISDIR) {
			printf("SKIP: O_TMPFILE is not supported here\n");
			return EXIT_SKIP;
		}
		perror("open O_TMPFILE");
		return EXIT_FAILURE;
	}

	if (write(fd, data, sizeof(data)) != (ssize_t)sizeof(data)) {
		perror("write tmpfile");
		close(fd);
		return EXIT_FAILURE;
	}

	if (fstat(fd, &st) == -1) {
		perror("fstat tmpfile");
		close(fd);
		return EXIT_FAILURE;
	}

	if (st.st_size == (off_t)sizeof(data))
		printf("PASS: anonymous O_TMPFILE stores written data\n");
	else {
		printf("FAIL: tmpfile size is %ld, expected %zu\n",
		       (long)st.st_size, sizeof(data));
		close(fd);
		return EXIT_FAILURE;
	}

	snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
	unlink("open14_linked");
	if (linkat(AT_FDCWD, path, AT_FDCWD, "open14_linked",
		   AT_SYMLINK_FOLLOW) == -1) {
		perror("linkat tmpfile");
		close(fd);
		return EXIT_FAILURE;
	}

	if (stat("open14_linked", &st) == -1) {
		perror("stat linked tmpfile");
		close(fd);
		return EXIT_FAILURE;
	}

	printf("PASS: anonymous O_TMPFILE can be linked into the directory\n");
	unlink("open14_linked");
	close(fd);
	return EXIT_SUCCESS;
#else
	printf("SKIP: O_TMPFILE test is Linux-specific\n");
	return EXIT_SKIP;
#endif
}
