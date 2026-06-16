// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap02 的简化转写版。
 *
 * 测试目标：
 *   只读打开的文件可以用 PROT_READ | MAP_SHARED 成功映射，
 *   映射内容应与文件内容一致。
 */

#include "mmap_simple_common.h"

int main(void)
{
	long page_size = page_size_or_die();
	char *buf = malloc(page_size);
	char *addr;
	int fd;
	int failed = 0;

	if (!buf)
		fail_now("malloc");
	memset(buf, 'A', page_size);

	unlink("mmap02_file");
	fd = create_filled_file("mmap02_file", page_size, 'A');
	if (fd == -1)
		fail_now("create mmap02_file");
	close(fd);

	if (chmod("mmap02_file", 0444) == -1)
		fail_now("chmod mmap02_file");
	fd = open("mmap02_file", O_RDONLY);
	if (fd == -1)
		fail_now("open read-only file");

	addr = mmap(NULL, page_size, PROT_READ, MAP_FILE | MAP_SHARED, fd, 0);
	if (addr == MAP_FAILED)
		fail_now("mmap");

	if (memcmp(buf, addr, page_size) == 0)
		pass_msg("read-only file mapping contains expected data");
	else
		failed = fail_msg("read-only file mapping contains wrong data");

	munmap(addr, page_size);
	close(fd);
	chmod("mmap02_file", 0666);
	unlink("mmap02_file");
	free(buf);
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
