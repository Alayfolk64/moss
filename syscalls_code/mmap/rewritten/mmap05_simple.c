// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap05 的简化转写版。
 *
 * 测试目标：
 *   PROT_NONE 文件映射建立成功后，访问映射内容应触发 SIGSEGV。
 */

#include "mmap_simple_common.h"

static sigjmp_buf env;
static volatile sig_atomic_t got_sigsegv;

static void sigsegv_handler(int sig)
{
	(void)sig;
	got_sigsegv = 1;
	siglongjmp(env, 1);
}

int main(void)
{
	long page_size = page_size_or_die();
	volatile char *addr;
	int fd;
	int failed = 0;

	signal(SIGSEGV, sigsegv_handler);
	unlink("mmap05_file");
	fd = create_filled_file("mmap05_file", page_size, 'A');
	if (fd == -1)
		fail_now("create mmap05_file");

	addr = mmap(NULL, page_size, PROT_NONE, MAP_FILE | MAP_SHARED, fd, 0);
	if (addr == MAP_FAILED)
		fail_now("mmap PROT_NONE");

	if (sigsetjmp(env, 1) == 0) {
		volatile char c = addr[0];
		(void)c;
	}

	if (got_sigsegv)
		pass_msg("PROT_NONE access raised SIGSEGV");
	else
		failed = fail_msg("PROT_NONE access did not raise SIGSEGV");

	munmap((void *)addr, page_size);
	close(fd);
	unlink("mmap05_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
