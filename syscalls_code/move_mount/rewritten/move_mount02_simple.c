// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * move_mount02 的简化转写版。
 *
 * 测试目标：
 *   move_mount() 系统调用的错误处理。
 *   验证各种非法参数是否返回正确的错误码。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

/* move_mount() 系统调用号 */
#ifndef __NR_move_mount
#define __NR_move_mount 429
#endif

/* fsopen() 系统调用号 */
#ifndef __NR_fsopen
#define __NR_fsopen 430
#endif

/* fsconfig() 系统调用号 */
#ifndef __NR_fsconfig
#define __NR_fsconfig 431
#endif

/* fsmount() 系统调用号 */
#ifndef __NR_fsmount
#define __NR_fsmount 432
#endif

/* move_mount() 标志 */
#ifndef MOVE_MOUNT_F_EMPTY_PATH
#define MOVE_MOUNT_F_EMPTY_PATH     0x00000004
#endif

/* fsconfig() 命令 */
#ifndef FSCONFIG_CMD_CREATE
#define FSCONFIG_CMD_CREATE    6
#endif

#define MNTPOINT "mntpoint"

static int failed;

/* 系统调用包装函数 */
static int move_mount(int from_dirfd, const char *from_pathname,
                      int to_dirfd, const char *to_pathname,
                      unsigned int flags)
{
	return syscall(__NR_move_mount, from_dirfd, from_pathname,
	               to_dirfd, to_pathname, flags);
}

static int fsopen(const char *fsname, unsigned int flags)
{
	return syscall(__NR_fsopen, fsname, flags);
}

static int fsconfig(int fd, unsigned int cmd, const char *key,
                    const void *value, int aux)
{
	return syscall(__NR_fsconfig, fd, cmd, key, value, aux);
}

static int fsmount(int fd, unsigned int flags, unsigned int ms_flags)
{
	return syscall(__NR_fsmount, fd, flags, ms_flags);
}

/* 测试用例结构 */
struct test_case {
	const char *name;
	int from_dirfd;
	const char *from_pathname;
	int to_dirfd;
	const char *to_pathname;
	unsigned int flags;
	int expected_errno;
};

/* 期望失败的测试 */
static void test_should_fail(struct test_case *tc, int valid_fsmfd)
{
	int from_fd = tc->from_dirfd;
	int ret;

	/* 如果测试用例需要有效的 fsmfd，使用传入的 */
	if (tc->from_dirfd == -2)
		from_fd = valid_fsmfd;

	errno = 0;
	ret = move_mount(from_fd, tc->from_pathname, tc->to_dirfd,
	                 tc->to_pathname, tc->flags);

	if (ret != -1) {
		printf("FAIL: %s - move_mount() succeeded unexpectedly\n", tc->name);
		umount(MNTPOINT);
		failed = 1;
		return;
	}

	if (errno == tc->expected_errno) {
		printf("PASS: %s - move_mount() failed with %s as expected\n",
		       tc->name, strerror(errno));
	} else {
		printf("FAIL: %s - expected %s but got %s\n",
		       tc->name, strerror(tc->expected_errno), strerror(errno));
		failed = 1;
	}
}

int main(void)
{
	struct stat st;
	int fd, fsmfd;

	/* 创建挂载点目录 */
	if (stat(MNTPOINT, &st) == -1) {
		if (mkdir(MNTPOINT, 0777) == -1) {
			perror("mkdir mntpoint");
			return EXIT_FAILURE;
		}
	}

	/* 创建一个有效的 fsmount fd 供测试使用 */
	fd = fsopen("tmpfs", 0);
	if (fd == -1) {
		printf("SKIP: fsopen() not supported: %s\n", strerror(errno));
		rmdir(MNTPOINT);
		return EXIT_SUCCESS;
	}

	if (fsconfig(fd, FSCONFIG_CMD_CREATE, NULL, NULL, 0) == -1) {
		printf("SKIP: fsconfig() failed: %s\n", strerror(errno));
		close(fd);
		rmdir(MNTPOINT);
		return EXIT_SUCCESS;
	}

	fsmfd = fsmount(fd, 0, 0);
	close(fd);
	if (fsmfd == -1) {
		printf("SKIP: fsmount() failed: %s\n", strerror(errno));
		rmdir(MNTPOINT);
		return EXIT_SUCCESS;
	}

	printf("Testing move_mount() error handling...\n\n");

	/* 定义测试用例
	 * 注意：from_dirfd == -2 表示使用有效的 fsmfd
	 */
	struct test_case tests[] = {
		{
			.name = "invalid-from-fd",
			.from_dirfd = -1,
			.from_pathname = "",
			.to_dirfd = AT_FDCWD,
			.to_pathname = MNTPOINT,
			.flags = MOVE_MOUNT_F_EMPTY_PATH,
			.expected_errno = EBADF,
		},
		{
			.name = "invalid-from-path",
			.from_dirfd = -2,  /* 使用有效的 fsmfd */
			.from_pathname = "invalid",
			.to_dirfd = AT_FDCWD,
			.to_pathname = MNTPOINT,
			.flags = MOVE_MOUNT_F_EMPTY_PATH,
			.expected_errno = EINVAL,  /* 当设置 EMPTY_PATH 时，路径必须为空 */
		},
		{
			.name = "invalid-to-fd",
			.from_dirfd = -2,  /* 使用有效的 fsmfd */
			.from_pathname = "",
			.to_dirfd = -1,
			.to_pathname = MNTPOINT,
			.flags = MOVE_MOUNT_F_EMPTY_PATH,
			.expected_errno = EBADF,
		},
		{
			.name = "invalid-to-path",
			.from_dirfd = -2,  /* 使用有效的 fsmfd */
			.from_pathname = "",
			.to_dirfd = AT_FDCWD,
			.to_pathname = "invalid",
			.flags = MOVE_MOUNT_F_EMPTY_PATH,
			.expected_errno = ENOENT,
		},
		{
			.name = "invalid-flags",
			.from_dirfd = -2,  /* 使用有效的 fsmfd */
			.from_pathname = "",
			.to_dirfd = AT_FDCWD,
			.to_pathname = MNTPOINT,
			.flags = 0x08,  /* 无效的标志 */
			.expected_errno = EINVAL,
		},
	};

	/* 运行所有测试 */
	for (size_t i = 0; i < sizeof(tests) / sizeof(tests[0]); i++) {
		test_should_fail(&tests[i], fsmfd);
	}

	/* 清理 */
	close(fsmfd);
	rmdir(MNTPOINT);

	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
