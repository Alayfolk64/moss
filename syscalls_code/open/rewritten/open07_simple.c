// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open07 的简化转写版。
 *
 * 测试目标：
 *   O_NOFOLLOW 不能打开“最后一个路径组件”为符号链接的路径；
 *   但路径中间的目录符号链接仍可被跟随。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef O_NOFOLLOW
#define O_NOFOLLOW 0
#endif

static int failed;

static void expect_eloop(const char *path)
{
	int fd;

	errno = 0;
	fd = open(path, O_RDONLY | O_NOFOLLOW);
	if (fd != -1) {
		close(fd);
		printf("FAIL: open(O_NOFOLLOW) unexpectedly opened %s\n", path);
		failed = 1;
	} else if (errno == ELOOP) {
		printf("PASS: open(O_NOFOLLOW) rejected final symlink %s\n", path);
	} else {
		printf("FAIL: %s expected ELOOP but got %s\n",
		       path, strerror(errno));
		failed = 1;
	}
}

int main(void)
{
	int fd;

	unlink("testfile");
	unlink("symfile1");
	unlink("symfile2");
	unlink("symdir1/testfile");
	unlink("symdir1");
	unlink("symdir2");
	rmdir("testdir");

	fd = open("testfile", O_CREAT | O_WRONLY, 0644);
	if (fd == -1) {
		perror("create testfile");
		return EXIT_FAILURE;
	}
	close(fd);
	if (mkdir("testdir", 0755) == -1) {
		perror("mkdir testdir");
		return EXIT_FAILURE;
	}

	symlink("testfile", "symfile1");
	symlink("symfile1", "symfile2");
	symlink("testdir", "symdir1");
	symlink("symdir1", "symdir2");

	fd = open("symdir1/testfile", O_CREAT | O_WRONLY, 0644);
	if (fd == -1) {
		perror("create file through symlinked directory");
		return EXIT_FAILURE;
	}
	close(fd);

	expect_eloop("symfile1");
	expect_eloop("symfile2");
	expect_eloop("symdir1");
	expect_eloop("symdir2");

	fd = open("symdir1/testfile", O_RDONLY | O_NOFOLLOW);
	if (fd == -1) {
		printf("FAIL: open(O_NOFOLLOW) failed for file inside symlinked dir: %s\n",
		       strerror(errno));
		failed = 1;
	} else {
		printf("PASS: open(O_NOFOLLOW) opened regular file inside symlinked dir\n");
		close(fd);
	}

	unlink("testfile");
	unlink("symfile1");
	unlink("symfile2");
	unlink("symdir1/testfile");
	unlink("symdir1");
	unlink("symdir2");
	rmdir("testdir");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
