// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap06 的简化转写版。
 *
 * 测试目标：
 *   对只写 fd 做文件映射、length 为 0、缺少 MAP_PRIVATE/MAP_SHARED
 *   等非法参数应返回预期 errno。
 */

#include "mmap_simple_common.h"

struct testcase {
	size_t length;
	int prot;
	int flags;
	int expected;
	const char *name;
};

int main(void)
{
	struct testcase tests[] = {
		{1024, PROT_WRITE, MAP_FILE | MAP_PRIVATE, EACCES, "write-only fd private write"},
		{1024, PROT_WRITE, MAP_FILE | MAP_SHARED, EACCES, "write-only fd shared write"},
		{1024, PROT_READ, MAP_FILE | MAP_PRIVATE, EACCES, "write-only fd private read"},
		{1024, PROT_READ, MAP_FILE | MAP_SHARED, EACCES, "write-only fd shared read"},
		{1024, PROT_READ | PROT_WRITE, MAP_FILE | MAP_PRIVATE, EACCES, "write-only fd private rw"},
		{1024, PROT_READ | PROT_WRITE, MAP_FILE | MAP_SHARED, EACCES, "write-only fd shared rw"},
		{0, PROT_READ | PROT_WRITE, MAP_FILE | MAP_SHARED, EINVAL, "zero length"},
		{1024, PROT_READ | PROT_WRITE, MAP_FILE, EINVAL, "missing map type"},
	};
	long page_size = page_size_or_die();
	int fd;
	int failed = 0;
	size_t i;

	unlink("mmap06_file");
	fd = open("mmap06_file", O_WRONLY | O_CREAT | O_TRUNC, 0666);
	if (fd == -1)
		fail_now("open mmap06_file");
	if (ftruncate(fd, page_size) == -1)
		fail_now("ftruncate mmap06_file");

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++) {
		void *addr;

		errno = 0;
		addr = mmap(NULL, tests[i].length, tests[i].prot, tests[i].flags, fd, 0);
		if (addr != MAP_FAILED) {
			printf("FAIL: %s unexpectedly succeeded\n", tests[i].name);
			munmap(addr, tests[i].length ? tests[i].length : page_size);
			failed = 1;
		} else {
			failed += expect_errno_name(tests[i].name, errno, tests[i].expected);
		}
	}

	close(fd);
	unlink("mmap06_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
