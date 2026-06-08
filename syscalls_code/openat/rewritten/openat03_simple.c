// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * openat03 的简化转写版。
 *
 * 测试目标：
 *   openat(AT_FDCWD, ".", O_TMPFILE | O_RDWR, mode) 能创建匿名文件，
 *   写入后可通过 linkat(/proc/self/fd/<fd>) 链接到目录中。
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

	fd = openat(AT_FDCWD, ".", O_TMPFILE | O_RDWR, 0600);
	if (fd == -1) {
		if (errno == EOPNOTSUPP || errno == ENOTSUP || errno == EISDIR) {
			printf("SKIP: O_TMPFILE is not supported here\n");
			return EXIT_SKIP;
		}
		perror("openat O_TMPFILE");
		return EXIT_FAILURE;
	}

	write(fd, data, sizeof(data));
	fstat(fd, &st);
	if (st.st_size != (off_t)sizeof(data)) {
		printf("FAIL: tmpfile size is %ld, expected %zu\n",
		       (long)st.st_size, sizeof(data));
		close(fd);
		return EXIT_FAILURE;
	}
	printf("PASS: openat O_TMPFILE stores written data\n");

	snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
	unlink("openat03_linked");
	if (linkat(AT_FDCWD, path, AT_FDCWD, "openat03_linked",
		   AT_SYMLINK_FOLLOW) == -1) {
		perror("linkat tmpfile");
		close(fd);
		return EXIT_FAILURE;
	}
	printf("PASS: openat O_TMPFILE can be linked into the directory\n");

	unlink("openat03_linked");
	close(fd);
	return EXIT_SUCCESS;
#else
	printf("SKIP: O_TMPFILE test is Linux-specific\n");
	return EXIT_SKIP;
#endif
}
