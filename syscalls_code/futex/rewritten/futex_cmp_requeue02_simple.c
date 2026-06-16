// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * futex_cmp_requeue02 的简化转写版。
 *
 * 测试目标：
 *   直接调用 FUTEX_CMP_REQUEUE 的错误路径，确认内核按 Linux 语义返回：
 *   1. wake 数为负数时返回 EINVAL；
 *   2. requeue 数为负数时返回 EINVAL；
 *   3. 比较值与 futex0 当前值不匹配时返回 EAGAIN。
 */

#define _GNU_SOURCE
#include <errno.h>
#ifdef __linux__
#include <linux/futex.h>
#endif
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SYS_futex
#ifdef __NR_futex
#define SYS_futex __NR_futex
#endif
#endif

#ifndef SYS_futex
#define SYS_futex -1
#define HAVE_FUTEX_SYSCALL 0
#else
#define HAVE_FUTEX_SYSCALL 1
#endif

#ifndef FUTEX_CMP_REQUEUE
#define FUTEX_CMP_REQUEUE 4
#endif

#ifndef MAP_ANONYMOUS
#define MAP_ANONYMOUS MAP_ANON
#endif

#define EXIT_SKIP 77
#define FUTEX_INITIAL 0

struct testcase {
	const char *name;
	int set_wakes;
	int set_requeues;
	int cmpval;
	int expected_errno;
};

static const struct testcase tests[] = {
	{"negative requeue count", 1, -1, FUTEX_INITIAL, EINVAL},
	{"negative wake count", -1, 1, FUTEX_INITIAL, EINVAL},
	{"mismatched cmp value", 1, 1, FUTEX_INITIAL + 1, EAGAIN},
};

static atomic_int *futexes;
static int failed;

#if HAVE_FUTEX_SYSCALL
static int futex_cmp_requeue(atomic_int *uaddr, int cmpval,
			     atomic_int *uaddr2, int wake_nr, int requeue_nr)
{
	return syscall(SYS_futex, (int *)uaddr, FUTEX_CMP_REQUEUE,
		       wake_nr, requeue_nr, (int *)uaddr2, cmpval);
}
#else
static int futex_cmp_requeue(atomic_int *uaddr, int cmpval,
			     atomic_int *uaddr2, int wake_nr, int requeue_nr)
{
	(void)uaddr;
	(void)cmpval;
	(void)uaddr2;
	(void)wake_nr;
	(void)requeue_nr;
	errno = ENOSYS;
	return -1;
}
#endif

static void run_one(const struct testcase *tc)
{
	int ret;

	/* Reset both futex words before every errno case. */
	atomic_store(&futexes[0], FUTEX_INITIAL);
	atomic_store(&futexes[1], FUTEX_INITIAL + 1);

	errno = 0;
	ret = futex_cmp_requeue(&futexes[0], tc->cmpval, &futexes[1],
				tc->set_wakes, tc->set_requeues);

	if (ret != -1) {
		printf("FAIL: %s unexpectedly returned %d\n", tc->name, ret);
		failed = 1;
		return;
	}

	if (errno == tc->expected_errno) {
		printf("PASS: %s failed with %s\n",
		       tc->name, strerror(tc->expected_errno));
		return;
	}

	printf("FAIL: %s expected %s but got %s\n",
	       tc->name, strerror(tc->expected_errno), strerror(errno));
	failed = 1;
}

int main(void)
{
	size_t i;

#if !HAVE_FUTEX_SYSCALL
	printf("SKIP: SYS_futex is not available\n");
	return EXIT_SKIP;
#endif

	futexes = mmap(NULL, sizeof(*futexes) * 2, PROT_READ | PROT_WRITE,
		       MAP_ANONYMOUS | MAP_SHARED, -1, 0);
	if (futexes == MAP_FAILED) {
		perror("mmap futex words");
		return EXIT_FAILURE;
	}

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++)
		run_one(&tests[i]);

	munmap(futexes, sizeof(*futexes) * 2);
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
