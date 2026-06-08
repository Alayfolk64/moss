// SPDX-License-Identifier: GPL-2.0-only
/*
 * kill09 的简化转写版。
 *
 * 测试目标：
 *   父进程调用 kill(child_pid, SIGKILL) 应该成功，子进程应该死于 SIGKILL。
 */

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void)
{
	pid_t pid;
	int status;

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
		perror("kill(child, SIGKILL)");
		return EXIT_FAILURE;
	}

	if (waitpid(pid, &status, 0) == -1) {
		perror("waitpid");
		return EXIT_FAILURE;
	}

	if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGKILL) {
		printf("FAIL: child did not die from SIGKILL\n");
		return EXIT_FAILURE;
	}

	printf("PASS: kill(%ld, SIGKILL) succeeded and child died from SIGKILL\n",
	       (long)pid);
	return EXIT_SUCCESS;
}
