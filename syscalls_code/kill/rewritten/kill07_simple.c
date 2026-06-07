// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * kill07 的简化转写版。
 *
 * 测试目标：
 *   SIGKILL 不能被捕获，也不能被处理函数拦截。
 */

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static void impossible_handler(int signal_number)
{
	(void)signal_number;
}

int main(void)
{
	struct sigaction action;
	pid_t pid;
	int status;
	int handler_was_rejected;

	memset(&action, 0, sizeof(action));
	action.sa_handler = impossible_handler;
	sigemptyset(&action.sa_mask);

	errno = 0;
	handler_was_rejected = sigaction(SIGKILL, &action, NULL) == -1 &&
			       errno == EINVAL;

	pid = fork();
	if (pid == -1) {
		perror("fork");
		return EXIT_FAILURE;
	}

	if (pid == 0) {
		for (;;)
			pause();
	}

	if (kill(pid, SIGKILL) == -1) {
		perror("kill(SIGKILL)");
		return EXIT_FAILURE;
	}

	if (waitpid(pid, &status, 0) == -1) {
		perror("waitpid");
		return EXIT_FAILURE;
	}

	if (!handler_was_rejected) {
		printf("FAIL: sigaction(SIGKILL) should fail with EINVAL\n");
		return EXIT_FAILURE;
	}

	if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGKILL) {
		printf("FAIL: child was not terminated by SIGKILL\n");
		return EXIT_FAILURE;
	}

	printf("PASS: SIGKILL handler was rejected and child died from SIGKILL\n");
	return EXIT_SUCCESS;
}
