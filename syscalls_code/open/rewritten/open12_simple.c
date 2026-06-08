// SPDX-License-Identifier: GPL-2.0-only
/*
 * open12 的简化转写版。
 *
 * 测试目标：
 *   验证 O_APPEND、O_NOATIME、O_CLOEXEC 和 O_LARGEFILE 的基本行为。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef O_LARGEFILE
#define O_LARGEFILE 0
#endif

static int check_fd_after_exec(const char *fd_arg)
{
	int fd = atoi(fd_arg);
	char byte = 'x';

	if (write(fd, &byte, 1) == -1 && errno == EBADF)
		return EXIT_SUCCESS;

	return EXIT_FAILURE;
}

static int test_append(void)
{
	int fd = open("open12_file", O_RDWR | O_APPEND);
	off_t before;
	off_t after;

	if (fd == -1) {
		perror("open O_APPEND");
		return 1;
	}

	before = lseek(fd, 0, SEEK_CUR);
	write(fd, "abc", 3);
	after = lseek(fd, 0, SEEK_CUR);
	close(fd);

	if (after > before) {
		printf("PASS: O_APPEND writes at end of file\n");
		return 0;
	}

	printf("FAIL: O_APPEND did not advance offset as expected\n");
	return 1;
}

static int test_cloexec(const char *self)
{
	int fd = open("open12_file", O_RDWR | O_CLOEXEC);
	char fd_arg[32];
	pid_t pid;
	int status;

	if (fd == -1) {
		perror("open O_CLOEXEC");
		return 1;
	}

	snprintf(fd_arg, sizeof(fd_arg), "%d", fd);
	pid = fork();
	if (pid == -1) {
		perror("fork");
		close(fd);
		return 1;
	}
	if (pid == 0) {
		execl(self, self, "--check-fd", fd_arg, NULL);
		_exit(2);
	}

	close(fd);
	waitpid(pid, &status, 0);

	if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
		printf("PASS: O_CLOEXEC closed fd across exec\n");
		return 0;
	}

	printf("FAIL: O_CLOEXEC fd was still usable after exec\n");
	return 1;
}

static int test_largefile(void)
{
	int fd = open("open12_large", O_LARGEFILE | O_RDWR | O_CREAT | O_TRUNC,
		      0600);

	if (fd == -1) {
		perror("open large file");
		return 1;
	}
	if (lseek(fd, (off_t)(4100ULL * 1024 * 1024), SEEK_SET) == -1) {
		perror("lseek large offset");
		close(fd);
		return 1;
	}
	write(fd, "x", 1);
	close(fd);

	fd = open("open12_large", O_LARGEFILE | O_RDONLY);
	if (fd == -1) {
		perror("reopen large file");
		return 1;
	}
	close(fd);
	unlink("open12_large");
	printf("PASS: O_LARGEFILE opened a sparse file above 4GB\n");
	return 0;
}

static int test_noatime(void)
{
#ifdef O_NOATIME
	struct stat before;
	struct stat after;
	char byte;
	int fd;

	if (stat("open12_file", &before) == -1) {
		perror("stat before O_NOATIME");
		return 1;
	}

	sleep(1);
	fd = open("open12_file", O_RDONLY | O_NOATIME);
	if (fd == -1) {
		printf("SKIP: O_NOATIME open failed: %s\n", strerror(errno));
		return 0;
	}
	read(fd, &byte, 1);
	close(fd);

	if (stat("open12_file", &after) == -1) {
		perror("stat after O_NOATIME");
		return 1;
	}

	if (before.st_atime == after.st_atime) {
		printf("PASS: O_NOATIME read did not update atime\n");
		return 0;
	}

	printf("FAIL: O_NOATIME read updated atime\n");
	return 1;
#else
	printf("SKIP: O_NOATIME is not available on this platform\n");
	return 0;
#endif
}

int main(int argc, char **argv)
{
	int fd;
	int failed = 0;

	if (argc == 3 && !strcmp(argv[1], "--check-fd"))
		return check_fd_after_exec(argv[2]);

	fd = open("open12_file", O_CREAT | O_RDWR | O_TRUNC, 0600);
	if (fd == -1) {
		perror("create open12_file");
		return EXIT_FAILURE;
	}
	write(fd, "open12", 6);
	close(fd);

	failed += test_append();
	failed += test_noatime();
	failed += test_cloexec(argv[0]);
	failed += test_largefile();

	unlink("open12_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
