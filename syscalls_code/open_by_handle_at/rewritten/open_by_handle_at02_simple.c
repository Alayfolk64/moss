// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open_by_handle_at02 的简化转写版。
 *
 * 测试目标：
 *   open_by_handle_at() 对无效 fd、过大/过小 file_handle、无权限等
 *   场景返回预期错误。
 */

#ifndef __linux__
#include <stdio.h>
#include <stdlib.h>
#define EXIT_SKIP 77
int main(void)
{
	printf("SKIP: open_by_handle_at is Linux-specific\n");
	return EXIT_SKIP;
}
#else
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#define EXIT_SKIP 77
#define HANDLE_BYTES 128

struct handle_storage {
	struct file_handle handle;
	unsigned char bytes[HANDLE_BYTES];
};

static int failed;

static void expect_errno_case(const char *name, int mount_fd,
			      struct file_handle *handle, int expected)
{
	int fd;

	errno = 0;
	fd = open_by_handle_at(mount_fd, handle, O_RDONLY);
	if (fd >= 0) {
		close(fd);
		printf("FAIL: %s unexpectedly succeeded\n", name);
		failed = 1;
		return;
	}

	if (errno == expected)
		printf("PASS: %s failed with %s\n", name, strerror(expected));
	else {
		printf("FAIL: %s expected %s but got %s\n",
		       name, strerror(expected), strerror(errno));
		failed = 1;
	}
}

int main(void)
{
	struct handle_storage valid;
	struct handle_storage zero;
	struct handle_storage high;
	int mount_id;
	int mount_fd;
	int fd;

	if (geteuid() != 0) {
		printf("SKIP: open_by_handle_at failure tests need root/capabilities\n");
		return EXIT_SKIP;
	}

	fd = open("test_file", O_CREAT | O_RDWR | O_TRUNC, 0600);
	if (fd == -1) {
		perror("create test_file");
		return EXIT_FAILURE;
	}
	close(fd);

	memset(&valid, 0, sizeof(valid));
	valid.handle.handle_bytes = HANDLE_BYTES;
	if (name_to_handle_at(AT_FDCWD, "test_file", &valid.handle,
			      &mount_id, 0) == -1) {
		if (errno == EOPNOTSUPP || errno == ENOSYS) {
			printf("SKIP: filesystem/kernel does not support file handles\n");
			return EXIT_SKIP;
		}
		perror("name_to_handle_at");
		return EXIT_FAILURE;
	}

	mount_fd = open("/", O_RDONLY | O_DIRECTORY);
	if (mount_fd == -1) {
		perror("open mount fd");
		return EXIT_FAILURE;
	}

	expect_errno_case("invalid mount fd", -1, &valid.handle, EBADF);

	memset(&zero, 0, sizeof(zero));
	zero.handle.handle_bytes = 0;
	expect_errno_case("zero handle size", mount_fd, &zero.handle, EINVAL);

	memset(&high, 0, sizeof(high));
	high.handle.handle_bytes = 255;
	expect_errno_case("too large handle size", mount_fd, &high.handle, EINVAL);

	if (seteuid(65534) == -1) {
		printf("SKIP: could not drop euid for EPERM case: %s\n",
		       strerror(errno));
	} else {
		expect_errno_case("missing capability", mount_fd, &valid.handle, EPERM);
		seteuid(0);
	}

	close(mount_fd);
	unlink("test_file");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
#endif
