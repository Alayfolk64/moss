/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef MMAP_SIMPLE_COMMON_H
#define MMAP_SIMPLE_COMMON_H

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef MAP_FILE
#define MAP_FILE 0
#endif

#ifndef MAP_FIXED_NOREPLACE
#define MAP_FIXED_NOREPLACE 0x100000
#endif

#ifndef MAP_SHARED_VALIDATE
#define MAP_SHARED_VALIDATE 0x03
#endif

#ifndef MAP_POPULATE
#define MAP_POPULATE 0x8000
#endif

#ifndef MAP_LOCKED
#define MAP_LOCKED 0x2000
#endif

#ifndef MAP_GROWSDOWN
#define MAP_GROWSDOWN 0x0100
#endif

#define EXIT_SKIP 77

static void fail_now(const char *msg)
{
	perror(msg);
	exit(EXIT_FAILURE);
}

static void pass_msg(const char *msg)
{
	printf("PASS: %s\n", msg);
}

static int fail_msg(const char *msg)
{
	printf("FAIL: %s\n", msg);
	return 1;
}

static int expect_errno_name(const char *name, int got, int expected)
{
	if (got == expected) {
		printf("PASS: %s failed with %s\n", name, strerror(expected));
		return 0;
	}

	printf("FAIL: %s expected %s but got %s\n",
	       name, strerror(expected), strerror(got));
	return 1;
}

static int write_full(int fd, const void *buf, size_t len)
{
	const char *p = buf;

	while (len > 0) {
		ssize_t n = write(fd, p, len);

		if (n < 0)
			return -1;
		if (n == 0) {
			errno = EIO;
			return -1;
		}
		p += n;
		len -= n;
	}
	return 0;
}

static int create_filled_file(const char *path, size_t size, int byte)
{
	int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0666);
	char buf[4096];

	if (fd == -1)
		return -1;

	memset(buf, byte, sizeof(buf));
	while (size > 0) {
		size_t chunk = size < sizeof(buf) ? size : sizeof(buf);

		if (write_full(fd, buf, chunk) == -1) {
			close(fd);
			return -1;
		}
		size -= chunk;
	}
	if (lseek(fd, 0, SEEK_SET) == -1) {
		close(fd);
		return -1;
	}
	return fd;
}

static long page_size_or_die(void)
{
	long page_size = sysconf(_SC_PAGESIZE);

	if (page_size <= 0)
		fail_now("sysconf(_SC_PAGESIZE)");
	return page_size;
}

#endif
