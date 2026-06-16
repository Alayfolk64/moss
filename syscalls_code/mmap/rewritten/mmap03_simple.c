// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap03 的简化转写版。
 *
 * 测试目标：
 *   可执行权限文件能用 PROT_EXEC 建立文件映射。这里同时带 PROT_READ，
 *   便于在简化用例中直接校验映射内容。
 */

#include "mmap_simple_common.h"

int main(void)
{
	long page_size = page_size_or_die();
	char *addr;
	int fd;
	int failed = 0;

	unlink("mmap03_file");
	fd = create_filled_file("mmap03_file", page_size, 'A');
	if (fd == -1)
		fail_now("create mmap03_file");
	if (fchmod(fd, 0555) == -1)
		fail_now("fchmod mmap03_file");

	addr = mmap(NULL, page_size, PROT_READ | PROT_EXEC,
		    MAP_FILE | MAP_SHARED, fd, 0);
	if (addr == MAP_FAILED)
		fail_now("mmap PROT_EXEC");

	if (addr[0] == 'A' && addr[page_size - 1] == 'A')
		pass_msg("PROT_EXEC file mapping succeeded and content is readable");
	else
		failed = fail_msg("PROT_EXEC file mapping content mismatch");

	munmap(addr, page_size);
	close(fd);
	chmod("mmap03_file", 0666);
	unlink("mmap03_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
