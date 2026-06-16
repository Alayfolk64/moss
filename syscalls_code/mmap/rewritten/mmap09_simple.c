// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap09 的简化转写版。
 *
 * 测试目标：
 *   文件处于 mmap 状态时，ftruncate() 缩小、扩大、清零文件均能返回成功。
 */

#include "mmap_simple_common.h"

#define MAPSIZE (64 * 1024)

int main(void)
{
	off_t sizes[] = {MAPSIZE - 8192, MAPSIZE + 1024, 0};
	const char *names[] = {"shrink mapped file", "grow mapped file", "truncate mapped file to zero"};
	char *addr;
	int fd;
	int failed = 0;
	size_t i;

	unlink("mmap09_file");
	fd = open("mmap09_file", O_RDWR | O_CREAT | O_TRUNC, 0666);
	if (fd == -1)
		fail_now("open mmap09_file");
	if (ftruncate(fd, MAPSIZE) == -1)
		fail_now("initial ftruncate");

	addr = mmap(NULL, MAPSIZE, PROT_READ | PROT_WRITE, MAP_FILE | MAP_SHARED, fd, 0);
	if (addr == MAP_FAILED)
		fail_now("mmap");
	memset(addr, 'A', MAPSIZE);

	for (i = 0; i < sizeof(sizes) / sizeof(sizes[0]); i++) {
		if (ftruncate(fd, sizes[i]) == 0)
			printf("PASS: %s succeeded\n", names[i]);
		else {
			printf("FAIL: %s failed with %s\n", names[i], strerror(errno));
			failed = 1;
		}
	}

	munmap(addr, MAPSIZE);
	close(fd);
	unlink("mmap09_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
