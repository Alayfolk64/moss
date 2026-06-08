// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open01 的简化转写版。
 *
 * 测试目标：
 *   1. open(O_CREAT) 创建文件后，fstat() 能看到创建时给的 sticky bit。
 *   2. open(O_DIRECTORY) 打开目录后，fstat() 能看到这是目录。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef O_DIRECTORY
#define O_DIRECTORY 0
#endif

static void fail_now(const char *msg)
{
	perror(msg);
	exit(EXIT_FAILURE);
}

int main(void)
{
	struct stat st;
	int fd;
	int failed = 0;

	unlink("open01_file");
	rmdir("open01_dir");

	fd = open("open01_file", O_RDWR | O_CREAT | O_TRUNC, 01444);
	if (fd == -1)
		fail_now("open regular file");
	if (fstat(fd, &st) == -1)
		fail_now("fstat regular file");
	close(fd);

	if (st.st_mode & S_ISVTX)
		printf("PASS: created file keeps sticky bit\n");
	else {
#ifdef __linux__
		printf("FAIL: created file does not have sticky bit\n");
		failed = 1;
#else
		printf("SKIP: regular-file sticky bit is not preserved on this OS\n");
#endif
	}

	if (mkdir("open01_dir", 0755) == -1)
		fail_now("mkdir");

	fd = open("open01_dir", O_RDONLY | O_DIRECTORY);
	if (fd == -1)
		fail_now("open directory");
	if (fstat(fd, &st) == -1)
		fail_now("fstat directory");
	close(fd);

	if (S_ISDIR(st.st_mode))
		printf("PASS: opened path is reported as a directory\n");
	else {
		printf("FAIL: opened path is not reported as a directory\n");
		failed = 1;
	}

	unlink("open01_file");
	rmdir("open01_dir");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
