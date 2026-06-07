// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open_tree02 的简化转写版。
 *
 * 测试目标：
 *   open_tree() 对 invalid fd、invalid path、invalid flags 返回预期错误。
 */

#ifndef __linux__
#include <stdio.h>
#include <stdlib.h>
#define EXIT_SKIP 77
int main(void)
{
	printf("SKIP: open_tree is Linux-specific\n");
	return EXIT_SKIP;
}
#else
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/mount.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

#define EXIT_SKIP 77

struct testcase {
	const char *name;
	int dirfd;
	const char *path;
	unsigned int flags;
	int expected_errno;
};

static int open_tree_call(int dfd, const char *path, unsigned int flags)
{
#ifdef SYS_open_tree
	return syscall(SYS_open_tree, dfd, path, flags);
#else
	errno = ENOSYS;
	return -1;
#endif
}

int main(void)
{
	struct testcase tests[] = {
		{"invalid fd", -1, ".", OPEN_TREE_CLONE, EBADF},
		{"invalid path", AT_FDCWD, "does-not-exist", OPEN_TREE_CLONE, ENOENT},
		{"invalid flags", AT_FDCWD, ".", 0xffffffffU, EINVAL},
	};
	size_t i;
	int failed = 0;

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++) {
		int fd;

		errno = 0;
		fd = open_tree_call(tests[i].dirfd, tests[i].path, tests[i].flags);
		if (fd == -1 && errno == ENOSYS) {
			printf("SKIP: open_tree syscall is not available\n");
			return EXIT_SKIP;
		}
		if (fd >= 0) {
			close(fd);
			printf("FAIL: %s unexpectedly succeeded\n", tests[i].name);
			failed = 1;
		} else if (errno == tests[i].expected_errno) {
			printf("PASS: %s failed with %s\n",
			       tests[i].name, strerror(errno));
		} else {
			printf("FAIL: %s expected %s but got %s\n",
			       tests[i].name, strerror(tests[i].expected_errno),
			       strerror(errno));
			failed = 1;
		}
	}

	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
#endif
