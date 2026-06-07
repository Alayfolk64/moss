// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open02 的简化转写版。
 *
 * 测试目标：
 *   1. 不带 O_CREAT 打开不存在的文件应返回 ENOENT。
 *   2. 非特权用户用 O_NOATIME 打开不属于自己的文件应返回 EPERM。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define EXIT_SKIP 77

static int check_errno(const char *name, int expected)
{
	if (errno == expected) {
		printf("PASS: %s failed with %s\n", name, strerror(expected));
		return 0;
	}

	printf("FAIL: %s expected %s but got %s\n",
	       name, strerror(expected), strerror(errno));
	return 1;
}

int main(void)
{
	int fd;
	int failed = 0;

	unlink("open02_missing");
	errno = 0;
	fd = open("open02_missing", O_RDWR);
	if (fd != -1) {
		close(fd);
		printf("FAIL: open missing file without O_CREAT succeeded\n");
		failed = 1;
	} else {
		failed += check_errno("open missing file without O_CREAT", ENOENT);
	}

#ifdef O_NOATIME
	if (geteuid() != 0) {
		printf("SKIP: O_NOATIME permission case needs root to switch user\n");
		return failed ? EXIT_FAILURE : EXIT_SKIP;
	}

	fd = open("open02_root_file", O_CREAT | O_WRONLY | O_TRUNC, 0644);
	if (fd == -1) {
		perror("create root-owned file");
		return EXIT_FAILURE;
	}
	close(fd);

	{
		struct passwd *pw = getpwnam("nobody");

		if (!pw) {
			printf("SKIP: user nobody was not found\n");
			unlink("open02_root_file");
			return failed ? EXIT_FAILURE : EXIT_SKIP;
		}

		if (seteuid(pw->pw_uid) == -1) {
			perror("seteuid nobody");
			unlink("open02_root_file");
			return EXIT_FAILURE;
		}
	}

	errno = 0;
	fd = open("open02_root_file", O_RDONLY | O_NOATIME);
	if (fd != -1) {
		close(fd);
		printf("FAIL: unprivileged O_NOATIME open succeeded\n");
		failed = 1;
	} else {
		failed += check_errno("unprivileged O_NOATIME open", EPERM);
	}

	if (seteuid(0) == -1) {
		perror("restore root euid");
		return EXIT_FAILURE;
	}
	unlink("open02_root_file");
#else
	printf("SKIP: O_NOATIME is not available on this platform\n");
#endif

	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
