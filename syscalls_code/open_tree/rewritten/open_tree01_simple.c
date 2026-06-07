// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open_tree01 的简化转写版。
 *
 * 测试目标：
 *   open_tree(..., OPEN_TREE_CLONE) 能克隆一个挂载树，move_mount()
 *   能把克隆结果移动到另一个挂载点。
 */

#ifndef __linux__
#include <stdio.h>
#include <stdlib.h>
#define EXIT_SKIP 77
int main(void)
{
	printf("SKIP: open_tree is Linux-specific\n");
	return EXIT_SKIP;
}
#else
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/mount.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/syscall.h>
#include <unistd.h>

#define EXIT_SKIP 77

static int open_tree_call(int dfd, const char *path, unsigned int flags)
{
#ifdef SYS_open_tree
	return syscall(SYS_open_tree, dfd, path, flags);
#else
	errno = ENOSYS;
	return -1;
#endif
}

static int move_mount_call(int from_dfd, const char *from_path, int to_dfd,
			   const char *to_path, unsigned int flags)
{
#ifdef SYS_move_mount
	return syscall(SYS_move_mount, from_dfd, from_path, to_dfd, to_path,
		       flags);
#else
	errno = ENOSYS;
	return -1;
#endif
}

int main(void)
{
	int fd;

	if (geteuid() != 0) {
		printf("SKIP: open_tree clone/move_mount needs root privileges\n");
		return EXIT_SKIP;
	}

	mkdir("mntpoint", 0755);
	mkdir("ot_mntpoint", 0755);

	fd = open_tree_call(AT_FDCWD, ".", OPEN_TREE_CLONE | OPEN_TREE_CLOEXEC);
	if (fd == -1) {
		if (errno == ENOSYS) {
			printf("SKIP: open_tree syscall is not available\n");
			return EXIT_SKIP;
		}
		if (errno == EPERM) {
			printf("SKIP: open_tree needs mount privileges\n");
			return EXIT_SKIP;
		}
		perror("open_tree");
		return EXIT_FAILURE;
	}

	if (move_mount_call(fd, "", AT_FDCWD, "ot_mntpoint",
			    MOVE_MOUNT_F_EMPTY_PATH) == -1) {
		if (errno == ENOSYS || errno == EPERM) {
			printf("SKIP: move_mount is unavailable or not permitted\n");
			close(fd);
			return EXIT_SKIP;
		}
		perror("move_mount");
		close(fd);
		return EXIT_FAILURE;
	}

	close(fd);
	umount("ot_mntpoint");
	rmdir("ot_mntpoint");
	rmdir("mntpoint");
	printf("PASS: open_tree cloned a mount and move_mount attached it\n");
	return EXIT_SUCCESS;
}
#endif
