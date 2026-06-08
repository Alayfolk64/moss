// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open09 的简化转写版。
 *
 * 测试目标：
 *   O_RDONLY 打开的 fd 不能写，O_WRONLY 打开的 fd 不能读，错误应为 EBADF。
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(void)
{
	char byte = 'x';
	int fd;
	int failed = 0;

	fd = open("open09_file", O_CREAT | O_TRUNC | O_RDWR, 0600);
	if (fd == -1) {
		perror("create file");
		return EXIT_FAILURE;
	}
	close(fd);

	fd = open("open09_file", O_RDONLY);
	if (fd == -1) {
		perror("open O_RDONLY");
		return EXIT_FAILURE;
	}
	errno = 0;
	if (write(fd, &byte, 1) == -1 && errno == EBADF)
		printf("PASS: write() on O_RDONLY fd failed with EBADF\n");
	else {
		printf("FAIL: write() on O_RDONLY fd did not fail with EBADF\n");
		failed = 1;
	}
	close(fd);

	fd = open("open09_file", O_WRONLY);
	if (fd == -1) {
		perror("open O_WRONLY");
		return EXIT_FAILURE;
	}
	errno = 0;
	if (read(fd, &byte, 1) == -1 && errno == EBADF)
		printf("PASS: read() on O_WRONLY fd failed with EBADF\n");
	else {
		printf("FAIL: read() on O_WRONLY fd did not fail with EBADF\n");
		failed = 1;
	}
	close(fd);

	unlink("open09_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
