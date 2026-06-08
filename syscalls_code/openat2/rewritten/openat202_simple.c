// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * openat202 的简化转写版。
 *
 * 测试目标：
 *   openat2() 的 resolve 选项能限制跨挂载点、magic link、
 *   普通符号链接和逃出目录根的路径解析。
 */

#ifndef __linux__
#include <stdio.h>
#include <stdlib.h>
#define EXIT_SKIP 77
int main(void)
{
	printf("SKIP: openat2 is Linux-specific\n");
	return EXIT_SKIP;
}
#else
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/openat2.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

#define EXIT_SKIP 77

struct testcase {
	const char *name;
	const char *path;
	uint64_t resolve;
	int expected_errno;
};

static int openat2_call(int dfd, const char *path, struct open_how *how,
			size_t size)
{
#ifdef SYS_openat2
	return syscall(SYS_openat2, dfd, path, how, size);
#else
	errno = ENOSYS;
	return -1;
#endif
}

int main(void)
{
	struct testcase tests[] = {
		{"open /proc/version", "/proc/version", 0, 0},
		{"open /proc/self/exe magiclink", "/proc/self/exe", 0, 0},
		{"open ordinary symlink", "foo_symlink", 0, 0},
		{"RESOLVE_NO_XDEV blocks /proc", "/proc/version", RESOLVE_NO_XDEV, EXDEV},
		{"RESOLVE_NO_MAGICLINKS blocks /proc/self/exe", "/proc/self/exe", RESOLVE_NO_MAGICLINKS, ELOOP},
		{"RESOLVE_NO_SYMLINKS blocks symlink", "foo_symlink", RESOLVE_NO_SYMLINKS, ELOOP},
		{"RESOLVE_BENEATH blocks absolute /proc", "/proc/version", RESOLVE_BENEATH, EXDEV},
		{"RESOLVE_BENEATH blocks ../foo", "../foo", RESOLVE_BENEATH, EXDEV},
		{"RESOLVE_IN_ROOT treats /proc as inside cwd", "/proc/version", RESOLVE_IN_ROOT, ENOENT},
	};
	struct open_how how;
	size_t i;
	int failed = 0;

	unlink("foo_symlink");
	symlink("/proc/version", "foo_symlink");

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++) {
		int fd;

		memset(&how, 0, sizeof(how));
		how.flags = O_RDONLY | O_CREAT;
		how.mode = 0400;
		how.resolve = tests[i].resolve;

		errno = 0;
		fd = openat2_call(AT_FDCWD, tests[i].path, &how, sizeof(how));
		if (fd == -1 && errno == ENOSYS) {
			printf("SKIP: openat2 syscall is not available\n");
			return EXIT_SKIP;
		}

		if (!tests[i].expected_errno) {
			if (fd >= 0) {
				printf("PASS: %s succeeded\n", tests[i].name);
				close(fd);
			} else {
				printf("FAIL: %s failed with %s\n",
				       tests[i].name, strerror(errno));
				failed = 1;
			}
			continue;
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

	unlink("foo_symlink");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
#endif
