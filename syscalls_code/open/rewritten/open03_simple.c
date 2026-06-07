// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open03 的简化转写版。
 *
 * 测试目标：
 *   open(path, O_RDWR | O_CREAT, 0700) 能成功创建并打开文件。
 */

#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(void)
{
	int fd;

	unlink("open03_file");
	fd = open("open03_file", O_RDWR | O_CREAT, 0700);
	if (fd == -1) {
		perror("open");
		return EXIT_FAILURE;
	}

	close(fd);
	unlink("open03_file");
	printf("PASS: open(O_RDWR | O_CREAT) created and opened the file\n");
	return EXIT_SUCCESS;
}
