// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * kill05 的简化转写版。
 *
 * 测试目标：
 *   普通用户不能用 kill() 杀掉另一个用户拥有的进程。
 *
 * 这个测试需要 root 启动，因为它要创建两个不同 uid 的子进程。
 * 如果当前不是 root，程序会输出 SKIP 并返回 77。
 */

#include <errno.h>
#include <pwd.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define EXIT_SKIP 77
#define READY 'R'

struct kill_result {
	int ret;
	int err;
};

static void fail_now(const char *message)
{
	perror(message);
	exit(EXIT_FAILURE);
}

static int pick_two_non_root_users(uid_t users[2])
{
	struct passwd *pw;
	int count = 0;

	setpwent();
	while ((pw = getpwent()) != NULL) {
		if (pw->pw_uid == 0)
			continue;

		if (count == 0 || pw->pw_uid != users[0])
			users[count++] = pw->pw_uid;

		if (count == 2)
			break;
	}
	endpwent();

	return count == 2 ? 0 : -1;
}

static void drop_to_user(uid_t uid)
{
	if (setreuid(uid, uid) == -1)
		fail_now("setreuid");
}

static void write_all(int fd, const void *buf, size_t size)
{
	const char *p = buf;

	while (size > 0) {
		ssize_t written = write(fd, p, size);

		if (written == -1 && errno == EINTR)
			continue;
		if (written <= 0)
			fail_now("write");

		p += written;
		size -= (size_t)written;
	}
}

static void read_all(int fd, void *buf, size_t size)
{
	char *p = buf;

	while (size > 0) {
		ssize_t nread = read(fd, p, size);

		if (nread == -1 && errno == EINTR)
			continue;
		if (nread <= 0)
			fail_now("read");

		p += nread;
		size -= (size_t)nread;
	}
}

int main(void)
{
	uid_t users[2];
	int ready_pipe[2];
	int result_pipe[2];
	char ready;
	pid_t target_pid;
	pid_t killer_pid;
	struct kill_result result;

	if (geteuid() != 0) {
		printf("SKIP: this test needs root so it can switch users\n");
		return EXIT_SKIP;
	}

	if (pick_two_non_root_users(users) == -1) {
		printf("SKIP: could not find two non-root users in passwd\n");
		return EXIT_SKIP;
	}

	if (pipe(ready_pipe) == -1 || pipe(result_pipe) == -1)
		fail_now("pipe");

	target_pid = fork();
	if (target_pid == -1)
		fail_now("fork target");

	if (target_pid == 0) {
		close(ready_pipe[0]);
		close(result_pipe[0]);
		close(result_pipe[1]);

		drop_to_user(users[0]);
		ready = READY;
		write_all(ready_pipe[1], &ready, 1);

		for (;;)
			pause();
	}

	close(ready_pipe[1]);
	read_all(ready_pipe[0], &ready, 1);
	if (ready != READY)
		fail_now("target child did not become ready");

	killer_pid = fork();
	if (killer_pid == -1)
		fail_now("fork killer");

	if (killer_pid == 0) {
		close(result_pipe[0]);
		drop_to_user(users[1]);

		errno = 0;
		result.ret = kill(target_pid, SIGKILL);
		result.err = errno;

		write_all(result_pipe[1], &result, sizeof(result));
		_exit(EXIT_SUCCESS);
	}

	close(result_pipe[1]);
	read_all(result_pipe[0], &result, sizeof(result));
	waitpid(killer_pid, NULL, 0);

	kill(target_pid, SIGKILL);
	waitpid(target_pid, NULL, 0);

	if (result.ret == -1 && result.err == EPERM) {
		printf("PASS: user %ld could not kill process owned by user %ld\n",
		       (long)users[1], (long)users[0]);
		return EXIT_SUCCESS;
	}

	printf("FAIL: kill returned %d, errno=%d (%s), expected EPERM\n",
	       result.ret, result.err, strerror(result.err));
	return EXIT_FAILURE;
}
