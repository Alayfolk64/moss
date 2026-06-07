// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * openat203 的简化转写版。
 *
 * 测试目标：
 *   openat2() 对无效 dirfd、pathname、flags、mode、resolve、
 *   以及 open_how 结构体 size 参数返回正确错误。
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

struct open_how_pad {
	struct open_how how;
	uint64_t pad;
};

struct testcase {
	const char *name;
	int dfd;
	const char *path;
	uint64_t flags;
	uint64_t mode;
	uint64_t resolve;
	size_t size;
	int use_pad;
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
		{"invalid dirfd", -1, "test_file", O_RDWR | O_CREAT, 0700, 0, sizeof(struct open_how), 0, EBADF},
		{"invalid pathname pointer", AT_FDCWD, NULL, O_RDONLY | O_CREAT, 0400, 0, sizeof(struct open_how), 0, EFAULT},
		{"mode without O_CREAT", AT_FDCWD, "test_file", O_RDONLY, 0200, 0, sizeof(struct open_how), 0, EINVAL},
		{"invalid mode bits", AT_FDCWD, "test_file", O_RDWR | O_CREAT, UINT64_MAX, 0, sizeof(struct open_how), 0, EINVAL},
		{"invalid resolve bits", AT_FDCWD, "test_file", O_RDWR | O_CREAT, 0700, UINT64_MAX, sizeof(struct open_how), 0, EINVAL},
		{"size is zero", AT_FDCWD, "test_file", O_RDWR | O_CREAT, 0700, 0, 0, 0, EINVAL},
		{"size too small", AT_FDCWD, "test_file", O_RDWR | O_CREAT, 0700, 0, sizeof(struct open_how) - 1, 0, EINVAL},
		{"size too big with nonzero padding", AT_FDCWD, "test_file", O_RDWR | O_CREAT, 0700, 0, sizeof(struct open_how_pad), 1, E2BIG},
	};
	struct open_how how;
	struct open_how_pad padded;
	size_t i;
	int failed = 0;

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++) {
		struct open_how *howp = &how;
		int fd;

		memset(&how, 0, sizeof(how));
		memset(&padded, 0, sizeof(padded));
		if (tests[i].use_pad) {
			padded.pad = 0xDEAD;
			howp = &padded.how;
		}

		howp->flags = tests[i].flags;
		howp->mode = tests[i].mode;
		howp->resolve = tests[i].resolve;

		errno = 0;
		fd = openat2_call(tests[i].dfd, tests[i].path, howp,
				  tests[i].size);
		if (fd == -1 && errno == ENOSYS) {
			printf("SKIP: openat2 syscall is not available\n");
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

	unlink("test_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
#endif
