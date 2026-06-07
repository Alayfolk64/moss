// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open04 的简化转写版。
 *
 * 测试目标：
 *   当进程打开文件描述符数量达到 RLIMIT_NOFILE 后，open() 返回 EMFILE。
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>

#define MAX_FDS 256

int main(void)
{
	struct rlimit old_limit;
	struct rlimit new_limit;
	int fds[MAX_FDS];
	int count = 0;
	int fd;
	int i;

	for (i = 0; i < MAX_FDS; i++)
		fds[i] = -1;

	if (getrlimit(RLIMIT_NOFILE, &old_limit) == -1) {
		perror("getrlimit");
		return EXIT_FAILURE;
	}

	new_limit = old_limit;
	if (new_limit.rlim_cur > 64)
		new_limit.rlim_cur = 64;

	if (setrlimit(RLIMIT_NOFILE, &new_limit) == -1) {
		perror("setrlimit");
		return EXIT_FAILURE;
	}

	for (;;) {
		fd = open("/dev/null", O_RDONLY);
		if (fd == -1)
			break;

		if (count == MAX_FDS) {
			close(fd);
			break;
		}
		fds[count++] = fd;
	}

	if (fd == -1 && errno == EMFILE) {
		printf("PASS: open() failed with EMFILE after %d extra fds\n",
		       count);
	} else {
		printf("FAIL: expected EMFILE, got %s\n",
		       fd == -1 ? strerror(errno) : "a successful open");
		fd = EXIT_FAILURE;
		goto out;
	}

	fd = EXIT_SUCCESS;

out:
	for (i = 0; i < count; i++)
		close(fds[i]);

	setrlimit(RLIMIT_NOFILE, &old_limit);
	return fd;
}
