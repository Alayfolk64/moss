// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * move_mount01 的简化转写版。
 *
 * 测试目标：
 *   move_mount() 系统调用的基本功能测试。
 *   验证各种标志组合是否正常工作。
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
#ifndef MOVE_MOUNT_F_SYMLINKS
#define MOVE_MOUNT_F_SYMLINKS       0x00000001
#define MOVE_MOUNT_F_AUTOMOUNTS     0x00000002
#define MOVE_MOUNT_F_EMPTY_PATH     0x00000004
#define MOVE_MOUNT_T_SYMLINKS       0x00000010
#define MOVE_MOUNT_T_AUTOMOUNTS     0x00000020
#define MOVE_MOUNT_T_EMPTY_PATH     0x00000040
#define MOVE_MOUNT_SET_GROUP        0x00000100
#endif

/* fsconfig() 命令 */
#ifndef FSCONFIG_SET_STRING
#define FSCONFIG_SET_STRING    1
#define FSCONFIG_CMD_CREATE    6
#endif

/* fsmount() 标志 */
#ifndef FSMOUNT_CLOEXEC
#define FSMOUNT_CLOEXEC 0x00000001
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

/* 检查路径是否已挂载 */
static int is_mounted(const char *path)
{
	FILE *fp;
	char line[256];
	int found = 0;

	fp = fopen("/proc/mounts", "r");
	if (!fp)
		return 0;

	while (fgets(line, sizeof(line), fp)) {
		if (strstr(line, path)) {
			found = 1;
			break;
		}
	}
	fclose(fp);
	return found;
}

/* 测试一个标志组合 */
static void test_flag(const char *name, unsigned int flag)
{
	int fd, fsmfd;

	/* 1. 创建文件系统上下文 */
	fd = fsopen("tmpfs", 0);
	if (fd == -1) {
		printf("SKIP: %s - fsopen() not supported: %s\n",
		       name, strerror(errno));
		return;
	}

	/* 2. 配置文件系统 */
	if (fsconfig(fd, FSCONFIG_CMD_CREATE, NULL, NULL, 0) == -1) {
		printf("FAIL: %s - fsconfig() failed: %s\n",
		       name, strerror(errno));
		close(fd);
		failed = 1;
		return;
	}

	/* 3. 创建挂载文件描述符 */
	fsmfd = fsmount(fd, 0, 0);
	close(fd);
	if (fsmfd == -1) {
		printf("FAIL: %s - fsmount() failed: %s\n",
		       name, strerror(errno));
		failed = 1;
		return;
	}

	/* 4. 移动挂载到目标路径 */
	if (move_mount(fsmfd, "", AT_FDCWD, MNTPOINT,
	               flag | MOVE_MOUNT_F_EMPTY_PATH) == -1) {
		printf("FAIL: %s - move_mount() failed: %s\n",
		       name, strerror(errno));
		close(fsmfd);
		failed = 1;
		return;
	}
	close(fsmfd);

	/* 5. 验证挂载成功 */
	if (is_mounted(MNTPOINT)) {
		printf("PASS: %s - move_mount() succeeded\n", name);
		umount(MNTPOINT);
	} else {
		printf("FAIL: %s - mount point not found after move_mount()\n", name);
		failed = 1;
	}
}

int main(void)
{
	struct stat st;

	/* 创建挂载点目录 */
	if (stat(MNTPOINT, &st) == -1) {
		if (mkdir(MNTPOINT, 0777) == -1) {
			perror("mkdir mntpoint");
			return EXIT_FAILURE;
		}
	}

	printf("Testing move_mount() with various flags...\n\n");

	/* 测试各种标志组合 */
	test_flag("MOVE_MOUNT_F_SYMLINKS", MOVE_MOUNT_F_SYMLINKS);
	test_flag("MOVE_MOUNT_F_AUTOMOUNTS", MOVE_MOUNT_F_AUTOMOUNTS);
	test_flag("MOVE_MOUNT_F_EMPTY_PATH", MOVE_MOUNT_F_EMPTY_PATH);
	test_flag("MOVE_MOUNT_T_SYMLINKS", MOVE_MOUNT_T_SYMLINKS);
	test_flag("MOVE_MOUNT_T_AUTOMOUNTS", MOVE_MOUNT_T_AUTOMOUNTS);
	test_flag("MOVE_MOUNT_T_EMPTY_PATH", MOVE_MOUNT_T_EMPTY_PATH);

	/* 清理 */
	rmdir(MNTPOINT);

	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
