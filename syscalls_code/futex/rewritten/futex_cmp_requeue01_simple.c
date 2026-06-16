// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * futex_cmp_requeue01 的简化转写版。
 *
 * 测试目标：
 *   创建一批阻塞在 futex0 上的子进程，调用 FUTEX_CMP_REQUEUE
 *   唤醒其中一部分，并把其余 waiters 移到 futex1 队列上。
 *   随后分别 wake futex1/futex0，检查返回值和子进程退出情况。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <limits.h>
#ifdef __linux__
#include <linux/futex.h>
#endif
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
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

#ifndef FUTEX_WAIT
#define FUTEX_WAIT 0
#endif

#ifndef FUTEX_WAKE
#define FUTEX_WAKE 1
#endif

#ifndef FUTEX_CMP_REQUEUE
#define FUTEX_CMP_REQUEUE 4
#endif

#ifndef MAP_ANONYMOUS
#define MAP_ANONYMOUS MAP_ANON
#endif

#define EXIT_SKIP 77
#define FUTEX1_INITIAL 1000
#define WAIT_SLEEP_TIMEOUT_US 5000000
#define CHILD_DONE_TIMEOUT_US 5000000

struct shared_data {
	atomic_int futexes[2];
	atomic_int spurious;
	atomic_int test_done;
};

struct testcase {
	int num_waiters;
	int set_wakes;
	int set_requeues;
};

static const struct testcase tests[] = {
	{10, 3, 7},
	{10, 0, 10},
	{10, 2, 6},
	{100, 50, 50},
	{100, 0, 70},
	{1000, 100, 900},
	{1000, 300, 500},
};

static struct shared_data *sd;
static int failed;

#if HAVE_FUTEX_SYSCALL
static int futex_wait(atomic_int *uaddr, int val)
{
	return syscall(SYS_futex, (int *)uaddr, FUTEX_WAIT, val, NULL, NULL, 0);
}

static int futex_wake(atomic_int *uaddr, int nr)
{
	return syscall(SYS_futex, (int *)uaddr, FUTEX_WAKE, nr, NULL, NULL, 0);
}

static int futex_cmp_requeue(atomic_int *uaddr, int cmpval,
			     atomic_int *uaddr2, int wake_nr, int requeue_nr)
{
	return syscall(SYS_futex, (int *)uaddr, FUTEX_CMP_REQUEUE,
		       wake_nr, requeue_nr, (int *)uaddr2, cmpval);
}
#else
static int futex_wait(atomic_int *uaddr, int val)
{
	(void)uaddr;
	(void)val;
	errno = ENOSYS;
	return -1;
}

static int futex_wake(atomic_int *uaddr, int nr)
{
	(void)uaddr;
	(void)nr;
	errno = ENOSYS;
	return -1;
}

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

static int read_proc_state(pid_t pid, char *state)
{
	char path[64];
	char buf[256];
	char *end;
	FILE *fp;

	snprintf(path, sizeof(path), "/proc/%d/stat", pid);
	fp = fopen(path, "r");
	if (!fp)
		return -1;

	if (!fgets(buf, sizeof(buf), fp)) {
		fclose(fp);
		return -1;
	}
	fclose(fp);

	end = strrchr(buf, ')');
	if (!end || end[1] != ' ')
		return -1;

	*state = end[2];
	return 0;
}

static int wait_for_sleeping(pid_t pid)
{
	int waited = 0;

	while (waited < WAIT_SLEEP_TIMEOUT_US) {
		char state;

		if (read_proc_state(pid, &state) == 0 && state == 'S')
			return 0;

		usleep(10000);
		waited += 10000;
	}

	return -1;
}

static void kill_children(pid_t *pids, int nr)
{
	int i;

	for (i = 0; i < nr; i++) {
		if (pids[i] > 0)
			kill(pids[i], SIGKILL);
	}

	for (i = 0; i < nr; i++) {
		if (pids[i] > 0)
			waitpid(pids[i], NULL, 0);
	}
}

static void child_main(void)
{
	int ret = 0;
	int slept_us = 0;
	int expected = atomic_load(&sd->futexes[0]);

	if (futex_wait(&sd->futexes[0], expected) == -1) {
		if (errno == EAGAIN) {
			atomic_fetch_add(&sd->spurious, 1);
		} else {
			fprintf(stderr, "child %d: futex_wait failed: %s\n",
				getpid(), strerror(errno));
			ret = 1;
		}
	}

	while (!atomic_load(&sd->test_done) && slept_us < CHILD_DONE_TIMEOUT_US) {
		usleep(50000);
		slept_us += 50000;
	}

	_exit(ret);
}

static int wait_children(pid_t *pids, int nr)
{
	int i;
	int ok = 0;

	for (i = 0; i < nr; i++) {
		int status;

		if (waitpid(pids[i], &status, 0) == -1) {
			printf("FAIL: waitpid(%d) failed: %s\n",
			       pids[i], strerror(errno));
			continue;
		}

		if (WIFEXITED(status) && WEXITSTATUS(status) == 0)
			ok++;
	}

	return ok;
}

static void run_one(size_t index, const struct testcase *tc)
{
	pid_t *pids;
	int i;
	int ret;
	int num_waits;
	int num_requeues;
	int num_total;
	int spurious;
	int woken_up;
	int exp_ret = tc->set_wakes + tc->set_requeues;

	printf("INFO: Test %zu: waiters=%d wakes=%d requeues=%d\n",
	       index, tc->num_waiters, tc->set_wakes, tc->set_requeues);

	pids = calloc(tc->num_waiters, sizeof(*pids));
	if (!pids) {
		perror("calloc pids");
		exit(EXIT_FAILURE);
	}

	atomic_store(&sd->futexes[0], 0);
	atomic_store(&sd->futexes[1], FUTEX1_INITIAL);
	atomic_store(&sd->spurious, 0);
	atomic_store(&sd->test_done, 0);

	for (i = 0; i < tc->num_waiters; i++) {
		pids[i] = fork();
		if (pids[i] == -1) {
			printf("FAIL: fork waiter %d/%d failed: %s\n",
			       i, tc->num_waiters, strerror(errno));
			kill_children(pids, i);
			free(pids);
			failed = 1;
			return;
		}

		if (pids[i] == 0)
			child_main();
	}

	for (i = 0; i < tc->num_waiters; i++) {
		if (wait_for_sleeping(pids[i]) == -1) {
			printf("FAIL: waiter pid %d did not enter sleeping state\n",
			       pids[i]);
			kill_children(pids, tc->num_waiters);
			free(pids);
			failed = 1;
			return;
		}
	}

	atomic_fetch_add(&sd->futexes[0], 1);
	atomic_fetch_add(&sd->futexes[1], 1);

	ret = futex_cmp_requeue(&sd->futexes[0], atomic_load(&sd->futexes[0]),
				&sd->futexes[1], tc->set_wakes,
				tc->set_requeues);
	if (ret == -1) {
		printf("FAIL: futex_cmp_requeue failed: %s\n", strerror(errno));
		atomic_store(&sd->test_done, 1);
		futex_wake(&sd->futexes[0], INT_MAX);
		futex_wake(&sd->futexes[1], INT_MAX);
		kill_children(pids, tc->num_waiters);
		free(pids);
		failed = 1;
		return;
	}

	if (ret > exp_ret) {
		printf("FAIL: futex_cmp_requeue returned %d, expected <= %d\n",
		       ret, exp_ret);
		failed = 1;
	} else {
		printf("INFO: futex_cmp_requeue returned %d\n", ret);
	}

	num_requeues = futex_wake(&sd->futexes[1], tc->num_waiters);
	num_waits = futex_wake(&sd->futexes[0], tc->num_waiters);
	atomic_store(&sd->test_done, 1);
	futex_wake(&sd->futexes[1], INT_MAX);
	futex_wake(&sd->futexes[0], INT_MAX);

	num_total = wait_children(pids, tc->num_waiters);
	spurious = atomic_load(&sd->spurious);

	printf("INFO: children woken, futex0=%d futex1=%d spurious=%d\n",
	       num_waits, num_requeues, spurious);

	if (num_total != tc->num_waiters) {
		printf("FAIL: %d waiters were not woken normally\n",
		       tc->num_waiters - num_total);
		failed = 1;
		free(pids);
		return;
	}

	if (num_requeues < tc->set_requeues - spurious ||
	    num_requeues > tc->set_requeues) {
		printf("FAIL: requeued %d waiters, expected range (%d, %d)\n",
		       num_requeues, tc->set_requeues - spurious,
		       tc->set_requeues);
		failed = 1;
		free(pids);
		return;
	}

	woken_up = ret - num_requeues;
	if (woken_up < tc->set_wakes - spurious ||
	    woken_up > tc->set_wakes + spurious) {
		printf("FAIL: woke %d waiters, expected range (%d, %d)\n",
		       woken_up, tc->set_wakes - spurious,
		       tc->set_wakes + spurious);
		failed = 1;
		free(pids);
		return;
	}

	printf("PASS: futex_cmp_requeue case %zu\n", index);
	free(pids);
}

int main(void)
{
	size_t i;

#if !HAVE_FUTEX_SYSCALL
	printf("SKIP: SYS_futex is not available\n");
	return EXIT_SKIP;
#endif

	sd = mmap(NULL, sizeof(*sd), PROT_READ | PROT_WRITE,
		  MAP_ANONYMOUS | MAP_SHARED, -1, 0);
	if (sd == MAP_FAILED) {
		perror("mmap shared data");
		return EXIT_FAILURE;
	}

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++)
		run_one(i, &tests[i]);

	munmap(sd, sizeof(*sd));
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
