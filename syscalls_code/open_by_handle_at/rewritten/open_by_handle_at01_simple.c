// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open_by_handle_at01 的简化转写版。
 *
 * 测试目标：
 *   name_to_handle_at() 取得文件句柄后，open_by_handle_at() 能通过句柄
 *   重新打开文件，并且 fstat() 能拿到合理的文件信息。
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

static int get_handle(int dirfd, const char *path, struct handle_storage *store,
		      int flags, int *mount_id)
{
	memset(store, 0, sizeof(*store));
	store->handle.handle_bytes = HANDLE_BYTES;
	return name_to_handle_at(dirfd, path, &store->handle, mount_id, flags);
}

int main(void)
{
	struct handle_storage handle;
	struct stat st;
	int mount_id;
	int mount_fd;
	int fd;
	int filefd;

	if (geteuid() != 0) {
		printf("SKIP: open_by_handle_at usually needs root/CAP_DAC_READ_SEARCH\n");
		return EXIT_SKIP;
	}

	mkdir("test_dir", 0700);
	filefd = open("test_dir/test_file", O_CREAT | O_RDWR | O_TRUNC, 0600);
	if (filefd == -1) {
		perror("create test file");
		return EXIT_FAILURE;
	}
	write(filefd, "x", 1);
	close(filefd);

	if (get_handle(AT_FDCWD, "test_dir/test_file", &handle, 0,
		       &mount_id) == -1) {
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

	fd = open_by_handle_at(mount_fd, &handle.handle, O_RDONLY);
	if (fd == -1) {
		if (errno == EPERM) {
			printf("SKIP: missing CAP_DAC_READ_SEARCH for open_by_handle_at\n");
			return EXIT_SKIP;
		}
		perror("open_by_handle_at");
		return EXIT_FAILURE;
	}

	if (fstat(fd, &st) == -1) {
		perror("fstat");
		return EXIT_FAILURE;
	}

	close(fd);
	close(mount_fd);
	unlink("test_dir/test_file");
	rmdir("test_dir");

	if (st.st_size == 1) {
		printf("PASS: open_by_handle_at reopened the file by handle\n");
		return EXIT_SUCCESS;
	}

	printf("FAIL: reopened file has unexpected size %ld\n", (long)st.st_size);
	return EXIT_FAILURE;
}
#endif
