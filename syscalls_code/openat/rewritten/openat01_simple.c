// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * openat01 的简化转写版。
 *
 * 测试目标：
 *   1. 相对路径会按 dirfd 指向的目录解释。
 *   2. 绝对路径会忽略 dirfd。
 *   3. dirfd 是普通文件时返回 ENOTDIR。
 *   4. dirfd 无效时返回 EBADF。
 *   5. dirfd 是 AT_FDCWD 时，相对路径按当前目录解释。
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

static int failed;

static void expect_success(const char *name, int dirfd, const char *path)
{
	int fd = openat(dirfd, path, O_RDWR);

	if (fd >= 0) {
		printf("PASS: %s succeeded\n", name);
		close(fd);
	} else {
		printf("FAIL: %s failed with %s\n", name, strerror(errno));
		failed = 1;
	}
}

static void expect_errno_case(const char *name, int dirfd, const char *path,
			      int expected)
{
	int fd;

	errno = 0;
	fd = openat(dirfd, path, O_RDWR);
	if (fd >= 0) {
		close(fd);
		printf("FAIL: %s unexpectedly succeeded\n", name);
		failed = 1;
	} else if (errno == expected) {
		printf("PASS: %s failed with %s\n", name, strerror(expected));
	} else {
		printf("FAIL: %s expected %s but got %s\n",
		       name, strerror(expected), strerror(errno));
		failed = 1;
	}
}

int main(void)
{
	char cwd[PATH_MAX];
	char absolute_path[PATH_MAX];
	int dirfd;
	int filefd;

	unlink("test_dir/test_file");
	rmdir("test_dir");

	if (mkdir("test_dir", 0700) == -1) {
		perror("mkdir");
		return EXIT_FAILURE;
	}

	filefd = open("test_dir/test_file", O_CREAT | O_RDWR, 0600);
	if (filefd == -1) {
		perror("create test file");
		return EXIT_FAILURE;
	}

	dirfd = open("test_dir", O_RDONLY);
	if (dirfd == -1) {
		perror("open test_dir");
		return EXIT_FAILURE;
	}

	if (!getcwd(cwd, sizeof(cwd))) {
		perror("getcwd");
		return EXIT_FAILURE;
	}
	snprintf(absolute_path, sizeof(absolute_path), "%s/test_dir/test_file", cwd);

	expect_success("relative path under dirfd", dirfd, "test_file");
	expect_success("absolute path ignores dirfd", dirfd, absolute_path);
	expect_errno_case("file fd used as dirfd", filefd, "test_file", ENOTDIR);
	expect_errno_case("invalid dirfd", 100000, "test_file", EBADF);
	expect_success("AT_FDCWD relative path", AT_FDCWD, "test_dir/test_file");

	close(filefd);
	close(dirfd);
	unlink("test_dir/test_file");
	rmdir("test_dir");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
