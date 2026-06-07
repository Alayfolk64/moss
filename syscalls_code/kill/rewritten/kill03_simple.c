// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * kill03 的简化转写版。
 *
 * 测试目标：
 *   1. 非法信号编号会让 kill() 失败，并设置 errno=EINVAL。
 *   2. 不存在的 pid 会让 kill() 失败，并设置 errno=ESRCH。
 *   3. INT_MIN 作为 pid 时也应返回 ESRCH。
 */

#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

struct test_case {
	const char *name;
	pid_t pid;
	int signal_number;
	int expected_errno;
};

static pid_t find_unused_pid(void)
{
	FILE *file;
	long pid_max = 4194304;
	long candidate;
	int checked = 0;

	file = fopen("/proc/sys/kernel/pid_max", "r");
	if (file) {
		if (fscanf(file, "%ld", &pid_max) != 1)
			pid_max = 4194304;
		fclose(file);
	}

	if (pid_max > INT_MAX)
		pid_max = INT_MAX;

	for (candidate = pid_max; candidate > 2 && checked < 100000;
	     candidate--, checked++) {
		errno = 0;
		if (kill((pid_t)candidate, 0) == -1 && errno == ESRCH)
			return (pid_t)candidate;
	}

	return -1;
}

static int run_one_case(const struct test_case *test)
{
	errno = 0;
	if (kill(test->pid, test->signal_number) != -1) {
		printf("FAIL: %s unexpectedly succeeded\n", test->name);
		return 1;
	}

	if (errno != test->expected_errno) {
		printf("FAIL: %s expected errno=%d but got errno=%d (%s)\n",
		       test->name, test->expected_errno, errno, strerror(errno));
		return 1;
	}

	printf("PASS: %s failed with expected errno=%d (%s)\n",
	       test->name, errno, strerror(errno));
	return 0;
}

int main(void)
{
	pid_t unused_pid = find_unused_pid();
	struct test_case tests[] = {
		{ "invalid signal number", getpid(), 2000, EINVAL },
		{ "unused pid", unused_pid, SIGKILL, ESRCH },
		{ "pid is INT_MIN", (pid_t)INT_MIN, SIGKILL, ESRCH },
	};
	size_t i;
	int failed = 0;

	if (unused_pid == -1) {
		fprintf(stderr, "FAIL: could not find an unused pid\n");
		return EXIT_FAILURE;
	}

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++)
		failed += run_one_case(&tests[i]);

	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
