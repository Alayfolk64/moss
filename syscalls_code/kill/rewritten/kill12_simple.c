// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * kill12 的简化转写版。
 *
 * 测试目标：
 *   子进程主动忽略 1..13 号信号。父进程发送这些信号时，
 *   除了不能忽略的 SIGKILL，其他信号都不应该杀死子进程。
 */

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define READY 'R'

static volatile sig_atomic_t child_can_exit;

static void sigchld_handler(int signal_number)
{
	(void)signal_number;
	child_can_exit = 1;
}

static void child_process(int signal_to_ignore, int ready_fd)
{
	char ready = READY;

	if (signal(SIGCHLD, sigchld_handler) == SIG_ERR)
		_exit(EXIT_FAILURE);

	if (signal_to_ignore != SIGKILL &&
	    signal(signal_to_ignore, SIG_IGN) == SIG_ERR)
		_exit(EXIT_FAILURE);

	if (write(ready_fd, &ready, 1) != 1)
		_exit(EXIT_FAILURE);

	while (!child_can_exit)
		pause();

	_exit(1);
}

static int wait_until_ready(int fd)
{
	char ready;
	ssize_t nread;

	do {
		nread = read(fd, &ready, 1);
	} while (nread == -1 && errno == EINTR);

	return nread == 1 && ready == READY ? 0 : -1;
}

static int run_one_signal(int signal_number)
{
	int ready_pipe[2];
	pid_t pid;
	int status;

	if (pipe(ready_pipe) == -1) {
		perror("pipe");
		return 1;
	}

	pid = fork();
	if (pid == -1) {
		perror("fork");
		return 1;
	}

	if (pid == 0) {
		close(ready_pipe[0]);
		child_process(signal_number, ready_pipe[1]);
	}

	close(ready_pipe[1]);
	if (wait_until_ready(ready_pipe[0]) == -1) {
		printf("FAIL: child was not ready for signal %d\n", signal_number);
		kill(pid, SIGKILL);
		waitpid(pid, NULL, 0);
		return 1;
	}

	kill(pid, signal_number);
	usleep(100000);
	kill(pid, SIGCHLD);

	if (waitpid(pid, &status, 0) == -1) {
		perror("waitpid");
		return 1;
	}

	if (signal_number == SIGKILL) {
		if (WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL) {
			printf("PASS: SIGKILL could not be ignored\n");
			return 0;
		}

		printf("FAIL: SIGKILL did not kill the child\n");
		return 1;
	}

	if (WIFEXITED(status) && WEXITSTATUS(status) == 1) {
		printf("PASS: signal %d was ignored, child exited later\n",
		       signal_number);
		return 0;
	}

	printf("FAIL: signal %d killed or changed the child unexpectedly\n",
	       signal_number);
	return 1;
}

int main(void)
{
	int signal_number;
	int failed = 0;

	for (signal_number = 1; signal_number <= 13; signal_number++)
		failed += run_one_signal(signal_number);

	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
