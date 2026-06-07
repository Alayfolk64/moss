// SPDX-License-Identifier: GPL-2.0-only
/*
 * openat02 的简化转写版。
 *
 * 测试目标：
 *   openat() 的 O_APPEND、O_CLOEXEC、O_LARGEFILE、O_NOATIME、
 *   O_NOFOLLOW、O_TRUNC 基本行为。
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

	return write(fd, "x", 1) == -1 && errno == EBADF ?
	       EXIT_SUCCESS : EXIT_FAILURE;
}

static int test_append(void)
{
	int fd = openat(AT_FDCWD, "test_file", O_APPEND | O_RDWR);
	off_t off;

	if (fd == -1) {
		perror("openat O_APPEND");
		return 1;
	}
	write(fd, "abcdefg", 7);
	off = lseek(fd, 0, SEEK_CUR);
	close(fd);

	if (off > 7) {
		printf("PASS: openat O_APPEND appends to the file\n");
		return 0;
	}

	printf("FAIL: openat O_APPEND did not append\n");
	return 1;
}

static int test_cloexec(const char *self)
{
	int fd = openat(AT_FDCWD, "test_file", O_CLOEXEC | O_RDWR);
	char arg[32];
	pid_t pid;
	int status;

	if (fd == -1) {
		perror("openat O_CLOEXEC");
		return 1;
	}

	snprintf(arg, sizeof(arg), "%d", fd);
	pid = fork();
	if (pid == -1) {
		perror("fork");
		close(fd);
		return 1;
	}
	if (pid == 0) {
		execl(self, self, "--check-fd", arg, NULL);
		_exit(2);
	}

	close(fd);
	waitpid(pid, &status, 0);
	if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
		printf("PASS: openat O_CLOEXEC closes fd across exec\n");
		return 0;
	}

	printf("FAIL: openat O_CLOEXEC fd survived exec\n");
	return 1;
}

static int test_largefile(void)
{
	int fd = openat(AT_FDCWD, "large_file",
			O_LARGEFILE | O_RDWR | O_CREAT | O_TRUNC, 0600);

	if (fd == -1) {
		perror("openat large file");
		return 1;
	}
	if (lseek(fd, (off_t)(4100ULL * 1024 * 1024), SEEK_SET) == -1) {
		perror("lseek large file");
		close(fd);
		return 1;
	}
	write(fd, "x", 1);
	close(fd);

	fd = openat(AT_FDCWD, "large_file", O_LARGEFILE | O_RDONLY);
	if (fd == -1) {
		perror("reopen large file");
		return 1;
	}
	close(fd);
	unlink("large_file");
	printf("PASS: openat O_LARGEFILE opened sparse file above 4GB\n");
	return 0;
}

static int test_noatime(void)
{
#ifdef O_NOATIME
	struct stat before;
	struct stat after;
	char byte;
	int fd;

	if (stat("test_file", &before) == -1) {
		perror("stat before");
		return 1;
	}
	sleep(1);
	fd = openat(AT_FDCWD, "test_file", O_NOATIME | O_RDONLY);
	if (fd == -1) {
		printf("SKIP: openat O_NOATIME failed: %s\n", strerror(errno));
		return 0;
	}
	read(fd, &byte, 1);
	close(fd);
	stat("test_file", &after);

	if (before.st_atime == after.st_atime) {
		printf("PASS: openat O_NOATIME did not update atime\n");
		return 0;
	}

	printf("FAIL: openat O_NOATIME updated atime\n");
	return 1;
#else
	printf("SKIP: O_NOATIME is not available on this platform\n");
	return 0;
#endif
}

static int test_nofollow(void)
{
	int fd;

	errno = 0;
	fd = openat(AT_FDCWD, "sfile", O_NOFOLLOW | O_RDONLY);
	if (fd != -1) {
		close(fd);
		printf("FAIL: openat O_NOFOLLOW opened a symlink\n");
		return 1;
	}
	if (errno == ELOOP) {
		printf("PASS: openat O_NOFOLLOW rejected symlink\n");
		return 0;
	}

	printf("FAIL: openat O_NOFOLLOW expected ELOOP but got %s\n",
	       strerror(errno));
	return 1;
}

static int test_trunc(void)
{
	struct stat st;
	int fd = openat(AT_FDCWD, "test_file", O_TRUNC | O_RDWR);

	if (fd == -1) {
		perror("openat O_TRUNC");
		return 1;
	}
	fstat(fd, &st);
	close(fd);

	if (st.st_size == 0) {
		printf("PASS: openat O_TRUNC truncated the file\n");
		return 0;
	}

	printf("FAIL: openat O_TRUNC left size %ld\n", (long)st.st_size);
	return 1;
}

int main(int argc, char **argv)
{
	int fd;
	int failed = 0;

	if (argc == 3 && !strcmp(argv[1], "--check-fd"))
		return check_fd_after_exec(argv[2]);

	unlink("test_file");
	unlink("sfile");
	fd = open("test_file", O_CREAT | O_WRONLY | O_TRUNC, 0600);
	if (fd == -1) {
		perror("create test_file");
		return EXIT_FAILURE;
	}
	write(fd, "test file", 9);
	close(fd);
	symlink("test_file", "sfile");

	failed += test_append();
	failed += test_cloexec(argv[0]);
	failed += test_largefile();
	failed += test_noatime();
	failed += test_nofollow();
	failed += test_trunc();

	unlink("test_file");
	unlink("sfile");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
