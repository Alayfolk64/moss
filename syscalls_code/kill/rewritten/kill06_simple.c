// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * kill06 的简化转写版。
 *
 * 测试目标：
 *   kill(-pgid, SIGKILL) 会杀掉指定进程组里的所有进程。
 */

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHILD_COUNT 4
#define READY 'R'
#define SETUP_FAILED 'F'

static pid_t children[CHILD_COUNT];

static void fail_now(const char *message)
{
	perror(message);
	exit(EXIT_FAILURE);
}

static void write_byte(int fd, char byte)
{
	if (write(fd, &byte, 1) != 1)
		_exit(EXIT_FAILURE);
}

static void read_ready(int fd)
{
	char byte;
	ssize_t nread;

	do {
		nread = read(fd, &byte, 1);
	} while (nread == -1 && errno == EINTR);

	if (nread != 1 || byte != READY) {
		fprintf(stderr, "FAIL: a child could not enter the process group\n");
		exit(EXIT_FAILURE);
	}
}

static void child_process(int ready_fd, pid_t pgid)
{
	if (pgid == 0)
		pgid = getpid();

	if (setpgid(0, pgid) == -1) {
		write_byte(ready_fd, SETUP_FAILED);
		_exit(EXIT_FAILURE);
	}

	write_byte(ready_fd, READY);

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
	pid_t pgid;
	int i;
	int failed = 0;

	if (pipe(ready_pipe) == -1)
		fail_now("pipe");

	children[0] = fork();
	if (children[0] == -1)
		fail_now("fork");

	if (children[0] == 0) {
		close(ready_pipe[0]);
		child_process(ready_pipe[1], 0);
	}

	pgid = children[0];
	if (setpgid(children[0], pgid) == -1 && errno != EACCES)
		fail_now("setpgid leader");
	read_ready(ready_pipe[0]);

	for (i = 1; i < CHILD_COUNT; i++) {
		children[i] = fork();
		if (children[i] == -1) {
			cleanup_leftovers();
			fail_now("fork");
		}

		if (children[i] == 0) {
			close(ready_pipe[0]);
			child_process(ready_pipe[1], pgid);
		}

		if (setpgid(children[i], pgid) == -1 && errno != EACCES) {
			cleanup_leftovers();
			fail_now("setpgid child");
		}
		read_ready(ready_pipe[0]);
	}

	close(ready_pipe[1]);

	if (kill(-pgid, SIGKILL) == -1) {
		cleanup_leftovers();
		fail_now("kill(-pgid, SIGKILL)");
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

	printf("PASS: kill(-%ld, SIGKILL) killed every process in the group\n",
	       (long)pgid);
	return EXIT_SUCCESS;
}
