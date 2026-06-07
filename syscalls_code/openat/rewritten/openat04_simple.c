// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * openat04 的简化转写版。
 *
 * 测试目标：
 *   在 setgid 目录里创建 O_TMPFILE 并链接出来后，检查 group id、
 *   setgid bit 和 group execute bit 是否按 umask/setgid 规则处理。
 *
 * 原始 LTP 用例还会挂载 noacl 文件系统；这里保留核心检查，避免引入
 * LTP 的设备和挂载框架。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define EXIT_SKIP 77

int main(void)
{
#if defined(O_TMPFILE) && defined(__linux__)
	struct passwd *pw;
	struct stat st;
	char path[PATH_MAX];
	gid_t test_gid;
	int dirfd;
	int fd;
	int failed = 0;

	if (geteuid() != 0) {
		printf("SKIP: openat04 needs root to chown and switch user\n");
		return EXIT_SKIP;
	}

	pw = getpwnam("nobody");
	if (!pw) {
		printf("SKIP: user nobody not found\n");
		return EXIT_SKIP;
	}

	test_gid = pw->pw_gid == 65534 ? 65533 : pw->pw_gid + 1;
	rmdir("openat04_dir");
	if (mkdir("openat04_dir", 0777) == -1) {
		perror("mkdir");
		return EXIT_FAILURE;
	}
	if (chown("openat04_dir", pw->pw_uid, test_gid) == -1 ||
	    chmod("openat04_dir", S_ISGID | 0777) == -1) {
		perror("prepare directory");
		return EXIT_FAILURE;
	}

	dirfd = open("openat04_dir", O_RDONLY | O_DIRECTORY);
	if (dirfd == -1) {
		perror("open dir");
		return EXIT_FAILURE;
	}

	if (setegid(pw->pw_gid) == -1 || seteuid(pw->pw_uid) == -1) {
		perror("switch user");
		return EXIT_FAILURE;
	}

	umask(S_IXGRP);
	fd = openat(dirfd, ".", O_TMPFILE | O_RDWR, S_ISGID | 0777);
	if (fd == -1) {
		printf("SKIP: O_TMPFILE open failed: %s\n", strerror(errno));
		return EXIT_SKIP;
	}

	snprintf(path, sizeof(path), "/proc/self/fd/%d", fd);
	if (linkat(AT_FDCWD, path, dirfd, "open.tmp", AT_SYMLINK_FOLLOW) == -1) {
		perror("linkat tmpfile");
		return EXIT_FAILURE;
	}

	if (fstatat(dirfd, "open.tmp", &st, 0) == -1) {
		perror("fstatat");
		return EXIT_FAILURE;
	}

	if (st.st_gid == test_gid)
		printf("PASS: linked tmpfile inherited setgid directory gid\n");
	else {
		printf("FAIL: gid is %ld, expected %ld\n",
		       (long)st.st_gid, (long)test_gid);
		failed = 1;
	}

	if (st.st_mode & S_ISGID) {
		printf("FAIL: setgid bit was kept on unprivileged tmpfile\n");
		failed = 1;
	} else {
		printf("PASS: setgid bit was stripped from tmpfile\n");
	}

	if (st.st_mode & S_IXGRP) {
		printf("FAIL: group execute bit survived umask\n");
		failed = 1;
	} else {
		printf("PASS: group execute bit was filtered by umask\n");
	}

	close(fd);
	close(dirfd);
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
#else
	printf("SKIP: openat04 is Linux O_TMPFILE-specific\n");
	return EXIT_SKIP;
#endif
}
