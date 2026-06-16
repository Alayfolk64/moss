// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap08 的简化转写版。
 *
 * 测试目标：
 *   文件映射使用已经关闭的 fd 时，mmap() 应返回 EBADF。
 */

#include "mmap_simple_common.h"

int main(void)
{
	long page_size = page_size_or_die();
	void *addr;
	int fd;
	int failed = 0;

	unlink("mmap08_file");
	fd = open("mmap08_file", O_RDWR | O_CREAT | O_TRUNC, 0666);
	if (fd == -1)
		fail_now("open mmap08_file");
	close(fd);

	errno = 0;
	addr = mmap(NULL, page_size, PROT_WRITE, MAP_FILE | MAP_SHARED, fd, 0);
	if (addr != MAP_FAILED) {
		munmap(addr, page_size);
		failed = fail_msg("mmap with closed fd unexpectedly succeeded");
	} else {
		failed += expect_errno_name("mmap with closed fd", errno, EBADF);
	}

	unlink("mmap08_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
