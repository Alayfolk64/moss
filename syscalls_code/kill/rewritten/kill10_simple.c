// SPDX-License-Identifier: GPL-2.0-only
/*
 * kill10 的简化转写版。
 *
 * 测试目标：
 *   创建多个进程组，每个进程组里放多个子进程。
 *   父进程用 kill(-pgid, SIGUSR1) 给每个进程组发信号，
 *   每个子进程收到 SIGUSR1 后都向父进程报告。
 */

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define GROUP_COUNT 2
#define CHILDREN_PER_GROUP 8
#define TOTAL_CHILDREN (GROUP_COUNT * CHILDREN_PER_GROUP)
#define READY 'R'
#define GOT_SIGNAL 'S'
#define TIMEOUT_MS 5000

static pid_t group_leaders[GROUP_COUNT];
static pid_t children[TOTAL_CHILDREN];
static int report_fd = -1;
static volatile sig_atomic_t reported;

static void fail_now(const char *message)
{
	perror(message);
	exit(EXIT_FAILURE);
}

static void set_nonblock(int fd)
{
	int flags = fcntl(fd, F_GETFL);

	if (flags == -1)
		fail_now("fcntl(F_GETFL)");
	if (fcntl(fd, F_SETFL, flags | O_NONBLOCK) == -1)
		fail_now("fcntl(F_SETFL)");
}

static void sigusr1_handler(int signal_number)
{
	const char byte = GOT_SIGNAL;

	(void)signal_number;
	if (!reported && report_fd >= 0) {
		reported = 1;
		(void)write(report_fd, &byte, 1);
	}
}

static void signal_child(int ready_fd, int report_write_fd, pid_t pgid)
{
	struct sigaction action;
	char ready = READY;

	if (pgid == 0)
		pgid = getpid();

	if (setpgid(0, pgid) == -1)
		_exit(EXIT_FAILURE);

	memset(&action, 0, sizeof(action));
	action.sa_handler = sigusr1_handler;
	sigemptyset(&action.sa_mask);

	report_fd = report_write_fd;
	if (sigaction(SIGUSR1, &action, NULL) == -1)
		_exit(EXIT_FAILURE);

	if (write(ready_fd, &ready, 1) != 1)
		_exit(EXIT_FAILURE);

	for (;;)
		pause();
}

static void wait_ready(int fd)
{
	char byte;

	if (read(fd, &byte, 1) != 1 || byte != READY)
		fail_now("read ready");
}

static pid_t start_child(int index, int ready_fd, int report_write_fd,
			 pid_t pgid)
{
	pid_t pid = fork();

	if (pid == -1)
		fail_now("fork");

	if (pid == 0)
		signal_child(ready_fd, report_write_fd, pgid);

	children[index] = pid;

	if (pgid == 0)
		pgid = pid;
	if (setpgid(pid, pgid) == -1 && errno != EACCES)
		fail_now("setpgid");

	return pid;
}

static int wait_for_reports(int report_read_fd, int expected)
{
	int received = 0;

	while (received < expected) {
		struct pollfd pollfd = {
			.fd = report_read_fd,
			.events = POLLIN,
		};
		int ret = poll(&pollfd, 1, TIMEOUT_MS);

		if (ret == -1 && errno == EINTR)
			continue;
		if (ret <= 0)
			break;

		for (;;) {
			char byte;
			ssize_t nread = read(report_read_fd, &byte, 1);

			if (nread == 1 && byte == GOT_SIGNAL) {
				received++;
				continue;
			}

			if (nread == -1 &&
			    (errno == EAGAIN || errno == EWOULDBLOCK)) {
				break;
			}

			if (nread == -1 && errno == EINTR)
				continue;

			break;
		}
	}

	return received;
}

static void cleanup_children(void)
{
	int i;

	for (i = 0; i < TOTAL_CHILDREN; i++) {
		if (children[i] > 0)
			kill(children[i], SIGTERM);
	}

	for (i = 0; i < TOTAL_CHILDREN; i++) {
		if (children[i] > 0)
			waitpid(children[i], NULL, 0);
	}
}

int main(void)
{
	int ready_pipe[2];
	int report_pipe[2];
	int group;
	int child;
	int index = 0;
	int received;

	if (pipe(ready_pipe) == -1 || pipe(report_pipe) == -1)
		fail_now("pipe");
	set_nonblock(report_pipe[0]);

	for (group = 0; group < GROUP_COUNT; group++) {
		pid_t pgid;

		pgid = start_child(index, ready_pipe[1], report_pipe[1], 0);
		group_leaders[group] = pgid;
		wait_ready(ready_pipe[0]);
		index++;

		for (child = 1; child < CHILDREN_PER_GROUP; child++) {
			start_child(index, ready_pipe[1], report_pipe[1], pgid);
			wait_ready(ready_pipe[0]);
			index++;
		}
	}

	for (group = 0; group < GROUP_COUNT; group++) {
		if (kill(-group_leaders[group], SIGUSR1) == -1) {
			cleanup_children();
			fail_now("kill(-pgid, SIGUSR1)");
		}
	}

	received = wait_for_reports(report_pipe[0], TOTAL_CHILDREN);
	cleanup_children();

	if (received != TOTAL_CHILDREN) {
		printf("FAIL: got %d/%d SIGUSR1 reports\n",
		       received, TOTAL_CHILDREN);
		return EXIT_FAILURE;
	}

	printf("PASS: every child in %d process groups reported SIGUSR1\n",
	       GROUP_COUNT);
	return EXIT_SUCCESS;
}
