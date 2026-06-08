// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open06 的简化转写版。
 *
 * 测试目标：
 *   FIFO 没有读者时，用 O_NONBLOCK | O_WRONLY 打开应返回 ENXIO。
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void)
{
	int fd;

	unlink("open06_fifo");
	if (mkfifo("open06_fifo", 0644) == -1) {
		perror("mkfifo");
		return EXIT_FAILURE;
	}

	errno = 0;
	fd = open("open06_fifo", O_NONBLOCK | O_WRONLY);
	if (fd != -1) {
		close(fd);
		printf("FAIL: FIFO open for nonblocking write succeeded without reader\n");
		unlink("open06_fifo");
		return EXIT_FAILURE;
	}

	unlink("open06_fifo");
	if (errno != ENXIO) {
		printf("FAIL: expected ENXIO but got %s\n", strerror(errno));
		return EXIT_FAILURE;
	}

	printf("PASS: FIFO open failed with ENXIO when no reader exists\n");
	return EXIT_SUCCESS;
}
