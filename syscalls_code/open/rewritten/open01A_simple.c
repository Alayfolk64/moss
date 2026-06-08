// SPDX-License-Identifier: GPL-2.0-only
/*
 * open01A 的简化转写版。
 *
 * 原始 runtest 里 open01A 实际运行的是 symlink01 -T open01。
 * 这里专门保留 symlink 场景下 open(2) 的 5 个测试点。
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define TEXT "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz"

static int failed;

static void cleanup(void)
{
	unlink("object");
	unlink("symbolic");
}

static void expect_errno_case(const char *name, int got_errno, int expected)
{
	if (got_errno == expected)
		printf("PASS: %s failed with %s\n", name, strerror(expected));
	else {
		printf("FAIL: %s expected %s but got %s\n",
		       name, strerror(expected), strerror(got_errno));
		failed = 1;
	}
}

static void write_read_check(int fd, const char *name)
{
	char buf[sizeof(TEXT)];

	if (write(fd, TEXT, strlen(TEXT)) != (ssize_t)strlen(TEXT)) {
		printf("FAIL: %s write failed\n", name);
		failed = 1;
		return;
	}

	if (lseek(fd, 0, SEEK_SET) == -1) {
		printf("FAIL: %s lseek failed\n", name);
		failed = 1;
		return;
	}

	memset(buf, 0, sizeof(buf));
	if (read(fd, buf, strlen(TEXT)) != (ssize_t)strlen(TEXT) ||
	    memcmp(buf, TEXT, strlen(TEXT))) {
		printf("FAIL: %s read did not match written data\n", name);
		failed = 1;
		return;
	}

	printf("PASS: %s can write and read through symlink\n", name);
}

int main(void)
{
	int fd;

	cleanup();

	if (symlink("object", "symbolic") == -1) {
		perror("symlink");
		return EXIT_FAILURE;
	}

	fd = open("symbolic", O_CREAT | O_RDWR, 0700);
	if (fd == -1) {
		perror("open symlink to create target");
		return EXIT_FAILURE;
	}
	write_read_check(fd, "create target through symlink");
	close(fd);
	cleanup();

	fd = open("object", O_CREAT | O_RDWR, 0700);
	if (fd == -1) {
		perror("create object");
		return EXIT_FAILURE;
	}
	close(fd);
	if (symlink("object", "symbolic") == -1) {
		perror("symlink existing object");
		return EXIT_FAILURE;
	}

	fd = open("symbolic", O_RDWR);
	if (fd == -1) {
		perror("open existing object through symlink");
		failed = 1;
	} else {
		write_read_check(fd, "open existing target through symlink");
		close(fd);
	}

	errno = 0;
	fd = open("symbolic", O_CREAT | O_EXCL | O_RDWR, 0700);
	if (fd != -1) {
		close(fd);
		printf("FAIL: O_CREAT|O_EXCL through existing symlink succeeded\n");
		failed = 1;
	} else {
		expect_errno_case("O_CREAT|O_EXCL through symlink", errno, EEXIST);
	}
	cleanup();

	if (symlink("missing_object", "symbolic") == -1) {
		perror("symlink dangling target");
		return EXIT_FAILURE;
	}
	errno = 0;
	fd = open("symbolic", O_RDWR);
	if (fd != -1) {
		close(fd);
		printf("FAIL: open dangling symlink succeeded\n");
		failed = 1;
	} else {
		expect_errno_case("open dangling symlink", errno, ENOENT);
	}
	cleanup();

	if (symlink("symbolic", "symbolic") == -1) {
		perror("symlink self loop");
		return EXIT_FAILURE;
	}
	errno = 0;
	fd = open("symbolic", O_RDWR);
	if (fd != -1) {
		close(fd);
		printf("FAIL: open symlink loop succeeded\n");
		failed = 1;
	} else {
		expect_errno_case("open symlink loop", errno, ELOOP);
	}

	cleanup();
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
