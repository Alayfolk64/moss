// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open08 的简化转写版。
 *
 * 测试目标：
 *   覆盖 open() 常见失败场景：EEXIST、EISDIR、ENOTDIR、
 *   ENAMETOOLONG、EACCES、EFAULT。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define EXIT_SKIP 77

#ifndef O_DIRECTORY
#define O_DIRECTORY 0
#endif

static int failed;

static void expect_errno_open(const char *name, const char *path, int flags,
			      int expected)
{
	int fd;

	errno = 0;
	fd = open(path, flags, 0644);
	if (fd != -1) {
		close(fd);
		printf("FAIL: %s unexpectedly succeeded\n", name);
		failed = 1;
		return;
	}

	if (errno == expected)
		printf("PASS: %s failed with %s\n", name, strerror(expected));
	else {
		printf("FAIL: %s expected %s but got %s\n",
		       name, strerror(expected), strerror(errno));
		failed = 1;
	}
}

static int run_eacces_as_nobody(void)
{
	struct passwd *pw = getpwnam("nobody");
	int fd;

	if (!pw) {
		printf("SKIP: nobody user not found for EACCES case\n");
		return 0;
	}

	if (seteuid(pw->pw_uid) == -1) {
		perror("seteuid nobody");
		return 1;
	}

	errno = 0;
	fd = open("root_only_0600", O_WRONLY);
	if (fd != -1) {
		close(fd);
		printf("FAIL: unprivileged open of root-only file succeeded\n");
		failed = 1;
	} else if (errno == EACCES) {
		printf("PASS: unprivileged open failed with EACCES\n");
	} else {
		printf("FAIL: EACCES case expected EACCES but got %s\n",
		       strerror(errno));
		failed = 1;
	}

	if (seteuid(0) == -1) {
		perror("restore root euid");
		return 1;
	}

	return 0;
}

int main(void)
{
	int fd;
	char long_name[4096];

	unlink("existing_file");
	unlink("root_only_0600");

	fd = open("existing_file", O_CREAT | O_WRONLY, 0600);
	if (fd == -1) {
		perror("create existing_file");
		return EXIT_FAILURE;
	}
	close(fd);

	expect_errno_open("O_CREAT | O_EXCL on existing file",
			  "existing_file", O_CREAT | O_EXCL | O_RDWR, EEXIST);
	expect_errno_open("O_RDWR on directory",
			  ".", O_RDWR, EISDIR);
	expect_errno_open("O_DIRECTORY on regular file",
			  "existing_file", O_RDONLY | O_DIRECTORY, ENOTDIR);

	memset(long_name, 'a', sizeof(long_name));
	long_name[sizeof(long_name) - 1] = '\0';
	expect_errno_open("too-long pathname", long_name, O_RDWR, ENAMETOOLONG);

	fd = open("root_only_0600", O_CREAT | O_WRONLY | O_TRUNC, 0600);
	if (fd == -1) {
		perror("create root_only_0600");
		return EXIT_FAILURE;
	}
	close(fd);

	if (geteuid() == 0)
		failed += run_eacces_as_nobody();
	else
		printf("SKIP: EACCES case needs root to switch user\n");

	errno = 0;
	fd = open((const char *)-1, O_CREAT, 0644);
	if (fd != -1) {
		close(fd);
		printf("FAIL: bad pathname pointer unexpectedly succeeded\n");
		failed = 1;
	} else if (errno == EFAULT) {
		printf("PASS: bad pathname pointer failed with EFAULT\n");
	} else {
		printf("FAIL: bad pathname pointer expected EFAULT but got %s\n",
		       strerror(errno));
		failed = 1;
	}

	unlink("existing_file");
	unlink("root_only_0600");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
