// SPDX-License-Identifier: GPL-2.0-only
/*
 * kill02 的简化转写版。
 *
 * 测试目标：
 *   kill(0, SIGUSR1) 只会把 SIGUSR1 发给“调用者所在进程组”里的进程，
 *   不会发给其他进程组里的进程。
 *
 * 运行方式：
 *   cc -Wall -Wextra -O2 -o kill02_simple kill02_simple.c
 *   ./kill02_simple
 */

#define _GNU_SOURCE
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

#define READY 'R'
#define GOT_SIGNAL 'S'
#define SETUP_FAILED 'F'

#define CHILD_COUNT 4
#define READY_TIMEOUT_MS 3000
#define SIGNAL_WAIT_SECONDS 1

struct child_info {
	const char *name;
	int same_process_group;
	int pipefd[2];
	pid_t pid;
	int received_signal;
};

static struct child_info children[CHILD_COUNT] = {
	{ "child 1: same process group", 1, { -1, -1 }, -1, 0 },
	{ "child 2: same process group", 1, { -1, -1 }, -1, 0 },
	{ "child 3: different process group", 0, { -1, -1 }, -1, 0 },
	{ "child 4: different process group", 0, { -1, -1 }, -1, 0 },
};

static int current_child_pipe = -1;
static volatile sig_atomic_t sigusr1_received;

static void fail_now(const char *message)
{
	perror(message);
	exit(EXIT_FAILURE);
}

static void close_if_open(int *fd)
{
	if (*fd >= 0) {
		close(*fd);
		*fd = -1;
	}
}

static void make_pipe_nonblocking_on_read_end(int pipefd[2])
{
	int flags;

	if (pipe(pipefd) == -1)
		fail_now("pipe");

	flags = fcntl(pipefd[0], F_GETFL);
	if (flags == -1)
		fail_now("fcntl(F_GETFL)");

	if (fcntl(pipefd[0], F_SETFL, flags | O_NONBLOCK) == -1)
		fail_now("fcntl(F_SETFL)");
}

static void sigusr1_handler(int signal_number)
{
	const char byte = GOT_SIGNAL;

	(void)signal_number;
	sigusr1_received = 1;

	if (current_child_pipe >= 0)
		(void)write(current_child_pipe, &byte, 1);
}

static void child_write(int fd, char byte)
{
	if (write(fd, &byte, 1) != 1)
		_exit(EXIT_FAILURE);
}

static void close_pipes_not_used_by_this_child(int child_index)
{
	int i;

	for (i = 0; i < CHILD_COUNT; i++) {
		close_if_open(&children[i].pipefd[0]);

		if (i != child_index)
			close_if_open(&children[i].pipefd[1]);
	}
}

static void child_process(int child_index)
{
	struct sigaction action;
	struct child_info *child = &children[child_index];

	close_pipes_not_used_by_this_child(child_index);

	if (!child->same_process_group && setpgid(0, 0) == -1) {
		child_write(child->pipefd[1], SETUP_FAILED);
		_exit(EXIT_FAILURE);
	}

	memset(&action, 0, sizeof(action));
	action.sa_handler = sigusr1_handler;
	sigemptyset(&action.sa_mask);

	current_child_pipe = child->pipefd[1];
	if (sigaction(SIGUSR1, &action, NULL) == -1) {
		child_write(child->pipefd[1], SETUP_FAILED);
		_exit(EXIT_FAILURE);
	}

	child_write(child->pipefd[1], READY);

	while (!sigusr1_received)
		pause();

	_exit(EXIT_SUCCESS);
}

static int read_one_byte_with_timeout(int fd, char *byte)
{
	struct pollfd pollfd = {
		.fd = fd,
		.events = POLLIN,
	};
	int ret;
	ssize_t nread;

	do {
		ret = poll(&pollfd, 1, READY_TIMEOUT_MS);
	} while (ret == -1 && errno == EINTR);

	if (ret <= 0)
		return ret;

	do {
		nread = read(fd, byte, 1);
	} while (nread == -1 && errno == EINTR);

	return nread == 1 ? 1 : -1;
}

static int wait_until_child_is_ready(struct child_info *child)
{
	char byte = 0;
	int ret = read_one_byte_with_timeout(child->pipefd[0], &byte);

	if (ret == 0) {
		fprintf(stderr, "FAIL: waiting for %s timed out\n", child->name);
		return -1;
	}

	if (ret < 0 || byte == SETUP_FAILED) {
		fprintf(stderr, "FAIL: %s failed during setup\n", child->name);
		return -1;
	}

	if (byte != READY) {
		fprintf(stderr, "FAIL: %s sent unexpected byte '%c'\n",
			child->name, byte);
		return -1;
	}

	return 0;
}

static void start_children(void)
{
	int i;

	for (i = 0; i < CHILD_COUNT; i++)
		make_pipe_nonblocking_on_read_end(children[i].pipefd);

	for (i = 0; i < CHILD_COUNT; i++) {
		children[i].pid = fork();
		if (children[i].pid == -1)
			fail_now("fork");

		if (children[i].pid == 0)
			child_process(i);
	}

	for (i = 0; i < CHILD_COUNT; i++)
		close_if_open(&children[i].pipefd[1]);
}

static int wait_until_all_children_are_ready(void)
{
	int i;

	for (i = 0; i < CHILD_COUNT; i++) {
		if (wait_until_child_is_ready(&children[i]) == -1)
			return -1;
	}

	return 0;
}

static int child_reported_signal(struct child_info *child)
{
	char byte;
	ssize_t nread = read(child->pipefd[0], &byte, 1);

	if (nread == 1)
		return byte == GOT_SIGNAL ? 1 : -1;

	if (nread == -1 && (errno == EAGAIN || errno == EWOULDBLOCK))
		return 0;

	if (nread == 0)
		return 0;

	return -1;
}

static void read_signal_reports(void)
{
	int i;

	sleep(SIGNAL_WAIT_SECONDS);

	for (i = 0; i < CHILD_COUNT; i++) {
		int ret = child_reported_signal(&children[i]);

		if (ret == -1) {
			fprintf(stderr, "FAIL: could not read report from %s\n",
				children[i].name);
			continue;
		}

		children[i].received_signal = ret;
	}
}

static int print_and_check_result(void)
{
	int i;
	int same_group_ok = 1;
	int different_group_ok = 1;

	for (i = 0; i < CHILD_COUNT; i++) {
		struct child_info *child = &children[i];

		printf("%s -> %s\n", child->name,
		       child->received_signal ? "got SIGUSR1" : "no SIGUSR1");

		if (child->same_process_group && !child->received_signal)
			same_group_ok = 0;

		if (!child->same_process_group && child->received_signal)
			different_group_ok = 0;
	}

	printf("%s: children in the caller's process group got SIGUSR1\n",
	       same_group_ok ? "PASS" : "FAIL");
	printf("%s: children in other process groups did not get SIGUSR1\n",
	       different_group_ok ? "PASS" : "FAIL");

	return same_group_ok && different_group_ok ? EXIT_SUCCESS : EXIT_FAILURE;
}

static void cleanup_children(void)
{
	int i;

	for (i = 0; i < CHILD_COUNT; i++) {
		if (children[i].pid > 0 &&
		    kill(children[i].pid, SIGKILL) == -1 &&
		    errno != ESRCH) {
			fprintf(stderr, "WARN: kill(%ld) failed: %s\n",
				(long)children[i].pid, strerror(errno));
		}
	}

	for (i = 0; i < CHILD_COUNT; i++) {
		if (children[i].pid > 0) {
			while (waitpid(children[i].pid, NULL, 0) == -1 &&
			       errno == EINTR) {
			}
		}

		close_if_open(&children[i].pipefd[0]);
	}
}

int main(void)
{
	struct sigaction ignore_sigusr1;
	int result = EXIT_FAILURE;

	if (setpgid(0, 0) == -1)
		fail_now("setpgid");

	memset(&ignore_sigusr1, 0, sizeof(ignore_sigusr1));
	ignore_sigusr1.sa_handler = SIG_IGN;
	sigemptyset(&ignore_sigusr1.sa_mask);

	if (sigaction(SIGUSR1, &ignore_sigusr1, NULL) == -1)
		fail_now("sigaction(SIGUSR1)");

	start_children();

	if (wait_until_all_children_are_ready() == 0) {
		if (kill(0, SIGUSR1) == -1) {
			perror("kill(0, SIGUSR1)");
		} else {
			read_signal_reports();
			result = print_and_check_result();
		}
	}

	cleanup_children();
	return result;
}
