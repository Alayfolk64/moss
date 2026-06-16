// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap10 的简化转写版。
 *
 * 测试目标：
 *   /dev/zero 或匿名页映射后可以读写、fork 子进程后局部 munmap 不应影响父进程。
 */

#include "mmap_simple_common.h"

#define SIZE (5 * 1024 * 1024)

static int run_one(int use_anon)
{
	long page_size = page_size_or_die();
	size_t len = SIZE + SIZE - page_size;
	char *addr;
	int fd = -1;
	pid_t child;
	int status;

	if (use_anon) {
		addr = mmap(NULL, len, PROT_READ | PROT_WRITE,
			    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	} else {
		fd = open("/dev/zero", O_RDWR);
		if (fd == -1) {
			printf("SKIP: /dev/zero is not available\n");
			return 0;
		}
		addr = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
	}
	if (addr == MAP_FAILED)
		fail_now("mmap");

	addr[SIZE] = 1;
	child = fork();
	if (child == -1)
		fail_now("fork");
	if (child == 0) {
		if (munmap(addr + SIZE + page_size, SIZE - page_size * 2) == -1)
			_exit(1);
		_exit(0);
	}

	if (waitpid(child, &status, 0) == -1)
		fail_now("waitpid");
	if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
		munmap(addr, len);
		if (fd >= 0)
			close(fd);
		return fail_msg(use_anon ? "anonymous child munmap failed" : "/dev/zero child munmap failed");
	}

	if (addr[SIZE] == 1)
		printf("PASS: %s mapping survived child munmap\n", use_anon ? "anonymous" : "/dev/zero");
	else {
		munmap(addr, len);
		if (fd >= 0)
			close(fd);
		return fail_msg("parent mapping data changed");
	}

	munmap(addr, len);
	if (fd >= 0)
		close(fd);
	return 0;
}

int main(void)
{
	int failed = 0;

	failed += run_one(0);
	failed += run_one(1);
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
