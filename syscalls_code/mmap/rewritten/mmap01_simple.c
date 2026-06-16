// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap01 的简化转写版。
 *
 * 测试目标：
 *   文件长度不是页大小整数倍时，EOF 到页尾的映射区域应为 0；
 *   写入这段页尾区域并 msync 后，不应扩展或污染原文件。
 */

#include "mmap_simple_common.h"

int main(void)
{
	long page_size = page_size_or_die();
	const char payload[] = "hello world\n";
	char *zero = calloc(1, page_size);
	char *addr;
	struct stat st;
	int fd;
	int failed = 0;

	if (!zero)
		fail_now("calloc");

	unlink("mmap01_file");
	fd = open("mmap01_file", O_RDWR | O_CREAT | O_TRUNC, 0666);
	if (fd == -1)
		fail_now("open mmap01_file");
	if (write_full(fd, payload, strlen(payload)) == -1)
		fail_now("write mmap01_file");
	if (fstat(fd, &st) == -1)
		fail_now("fstat mmap01_file");

	addr = mmap(NULL, page_size, PROT_READ | PROT_WRITE,
		    MAP_FILE | MAP_SHARED, fd, 0);
	if (addr == MAP_FAILED)
		fail_now("mmap");

	if (memcmp(addr + st.st_size, zero, page_size - st.st_size) == 0)
		pass_msg("bytes beyond EOF are zero-filled");
	else
		failed = fail_msg("bytes beyond EOF are not zero-filled");

	addr[st.st_size] = 'X';
	addr[st.st_size + 1] = 'Y';
	addr[st.st_size + 2] = 'Z';
	if (msync(addr, page_size, MS_SYNC) == -1)
		fail_now("msync");

	if (fstat(fd, &st) == -1)
		fail_now("fstat after msync");
	if (st.st_size == (off_t)strlen(payload))
		pass_msg("writes beyond EOF did not extend the file");
	else
		failed = fail_msg("writes beyond EOF changed file size");

	munmap(addr, page_size);
	close(fd);
	unlink("mmap01_file");
	free(zero);
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
