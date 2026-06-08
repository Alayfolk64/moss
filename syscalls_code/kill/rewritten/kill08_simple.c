// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * kill08 的简化转写版。
 *
 * 测试目标：
 *   进程调用 kill(0, SIGKILL) 时，会杀掉自己所在进程组里的所有进程。
 */

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHILD_COUNT 4
#define READY 'R'

static pid_t children[CHILD_COUNT];

static void fail_now(const char *message)
{
	perror(message);
	exit(EXIT_FAILURE);
}

static void write_ready(int fd)
{
	char byte = READY;

	if (write(fd, &byte, 1) != 1)
		_exit(EXIT_FAILURE);
}

static void wait_for_ready(int fd)
{
	char byte;

	if (read(fd, &byte, 1) != 1 || byte != READY)
		fail_now("read ready");
}

static void killer_child(int ready_fd, int start_fd)
{
	char byte;

	if (setpgid(0, 0) == -1)
		_exit(EXIT_FAILURE);

	write_ready(ready_fd);

	if (read(start_fd, &byte, 1) != 1)
		_exit(EXIT_FAILURE);

	kill(0, SIGKILL);
	_exit(EXIT_FAILURE);
}

static void quiet_child(int ready_fd, pid_t pgid)
{
	if (setpgid(0, pgid) == -1)
		_exit(EXIT_FAILURE);

	write_ready(ready_fd);

	for (;;)
		pause();
}

static void cleanup_leftovers(void)
{
	int i;

	for (i = 0; i < CHILD_COUNT; i++) {
		if (children[i] > 0)
			kill(children[i], SIGKILL);
	}
}

int main(void)
{
	int ready_pipe[2];
	int start_pipe[2];
	pid_t pgid;
	int i;
	int failed = 0;
	char start = 'S';

	if (pipe(ready_pipe) == -1 || pipe(start_pipe) == -1)
		fail_now("pipe");

	children[0] = fork();
	if (children[0] == -1)
		fail_now("fork");

	if (children[0] == 0) {
		close(ready_pipe[0]);
		close(start_pipe[1]);
		killer_child(ready_pipe[1], start_pipe[0]);
	}

	pgid = children[0];
	if (setpgid(children[0], pgid) == -1 && errno != EACCES)
		fail_now("setpgid leader");
	wait_for_ready(ready_pipe[0]);

	for (i = 1; i < CHILD_COUNT; i++) {
		children[i] = fork();
		if (children[i] == -1) {
			cleanup_leftovers();
			fail_now("fork");
		}

		if (children[i] == 0) {
			close(ready_pipe[0]);
			close(start_pipe[0]);
			close(start_pipe[1]);
			quiet_child(ready_pipe[1], pgid);
		}

		if (setpgid(children[i], pgid) == -1 && errno != EACCES) {
			cleanup_leftovers();
			fail_now("setpgid child");
		}
		wait_for_ready(ready_pipe[0]);
	}

	if (write(start_pipe[1], &start, 1) != 1) {
		cleanup_leftovers();
		fail_now("write start");
	}

	for (i = 0; i < CHILD_COUNT; i++) {
		int status;

		if (waitpid(children[i], &status, 0) == -1) {
			perror("waitpid");
			failed = 1;
			continue;
		}

		if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGKILL) {
			printf("FAIL: child %d did not die from SIGKILL\n", i + 1);
			failed = 1;
		}
	}

	if (failed)
		return EXIT_FAILURE;

	printf("PASS: kill(0, SIGKILL) killed the caller's whole process group\n");
	return EXIT_SUCCESS;
}
