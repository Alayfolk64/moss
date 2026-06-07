// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * openat201 的简化转写版。
 *
 * 测试目标：
 *   openat2() 能在 dirfd 和 AT_FDCWD 两种情况下，配合常见 flags、
 *   mode、resolve 选项成功创建/打开文件。
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
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#define EXIT_SKIP 77

struct open_how_pad {
	struct open_how how;
	uint64_t pad;
};

struct testcase {
	const char *name;
	int use_dirfd;
	const char *path;
	uint64_t flags;
	uint64_t mode;
	uint64_t resolve;
	size_t size;
	int use_pad;
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
		{"dirfd O_RDWR", 1, "test_file", O_RDWR, 0700, 0, sizeof(struct open_how), 0},
		{"dirfd O_RDONLY", 1, "test_file", O_RDONLY, 0400, 0, sizeof(struct open_how), 0},
		{"dirfd O_WRONLY", 1, "test_file", O_WRONLY, 0200, 0, sizeof(struct open_how), 0},
		{"dirfd RESOLVE_NO_XDEV", 1, "test_file", O_RDWR, 0700, RESOLVE_NO_XDEV, sizeof(struct open_how), 0},
		{"dirfd RESOLVE_NO_MAGICLINKS", 1, "test_file", O_RDWR, 0700, RESOLVE_NO_MAGICLINKS, sizeof(struct open_how), 0},
		{"dirfd RESOLVE_NO_SYMLINKS", 1, "test_file", O_RDWR, 0700, RESOLVE_NO_SYMLINKS, sizeof(struct open_how), 0},
		{"dirfd RESOLVE_BENEATH", 1, "test_file", O_RDWR, 0700, RESOLVE_BENEATH, sizeof(struct open_how), 0},
		{"dirfd RESOLVE_IN_ROOT", 1, "test_file", O_RDWR, 0700, RESOLVE_IN_ROOT, sizeof(struct open_how), 0},
		{"AT_FDCWD O_RDWR", 0, "test_file", O_RDWR, 0700, 0, sizeof(struct open_how), 0},
		{"AT_FDCWD O_RDONLY", 0, "test_file", O_RDONLY, 0400, 0, sizeof(struct open_how), 0},
		{"AT_FDCWD O_WRONLY", 0, "test_file", O_WRONLY, 0200, 0, sizeof(struct open_how), 0},
		{"AT_FDCWD RESOLVE_NO_XDEV", 0, "test_file", O_RDWR, 0700, RESOLVE_NO_XDEV, sizeof(struct open_how), 0},
		{"AT_FDCWD RESOLVE_NO_MAGICLINKS", 0, "test_file", O_RDWR, 0700, RESOLVE_NO_MAGICLINKS, sizeof(struct open_how), 0},
		{"AT_FDCWD RESOLVE_NO_SYMLINKS", 0, "test_file", O_RDWR, 0700, RESOLVE_NO_SYMLINKS, sizeof(struct open_how), 0},
		{"AT_FDCWD RESOLVE_BENEATH", 0, "test_file", O_RDWR, 0700, RESOLVE_BENEATH, sizeof(struct open_how), 0},
		{"AT_FDCWD padded struct", 0, "test_file", O_RDWR, 0700, RESOLVE_IN_ROOT, sizeof(struct open_how_pad), 1},
	};
	struct open_how_pad padded;
	size_t i;
	int dirfd;
	int failed = 0;

	mkdir("test_dir", 0700);
	dirfd = open("test_dir", O_RDONLY | O_DIRECTORY);
	if (dirfd == -1) {
		perror("open test_dir");
		return EXIT_FAILURE;
	}

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++) {
		struct open_how how;
		struct open_how *howp = &how;
		int dfd = tests[i].use_dirfd ? dirfd : AT_FDCWD;
		int fd;
		struct stat st;

		memset(&how, 0, sizeof(how));
		memset(&padded, 0, sizeof(padded));

		if (tests[i].use_pad)
			howp = &padded.how;

		howp->flags = tests[i].flags | O_CREAT;
		howp->mode = tests[i].mode;
		howp->resolve = tests[i].resolve;

		errno = 0;
		fd = openat2_call(dfd, tests[i].path, howp, tests[i].size);
		if (fd == -1 && errno == ENOSYS) {
			printf("SKIP: openat2 syscall is not available\n");
			close(dirfd);
			return EXIT_SKIP;
		}
		if (fd == -1) {
			printf("FAIL: %s failed with %s\n",
			       tests[i].name, strerror(errno));
			failed = 1;
			continue;
		}

		if (fstat(fd, &st) == -1 || st.st_size != 0) {
			printf("FAIL: %s opened fd but fstat was unexpected\n",
			       tests[i].name);
			failed = 1;
		} else {
			printf("PASS: %s succeeded\n", tests[i].name);
		}
		close(fd);
	}

	close(dirfd);
	unlink("test_file");
	unlink("test_dir/test_file");
	rmdir("test_dir");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
#endif
