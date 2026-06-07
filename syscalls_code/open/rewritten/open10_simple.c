// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open10 的简化转写版。
 *
 * 测试目标：
 *   新文件的 group id 和 setgid bit 应按父目录 setgid 规则设置。
 *   该测试需要 root 切换到 nobody 用户。
 */

#include <errno.h>
#include <fcntl.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#define EXIT_SKIP 77

static int failed;
static uid_t nobody_uid;
static gid_t nobody_gid;
static gid_t test_gid;

static void fail_now(const char *msg)
{
	perror(msg);
	exit(EXIT_FAILURE);
}

static void create_and_check(const char *path, mode_t mode, gid_t expected_gid,
			     int expect_sgid)
{
	struct stat st;
	int fd = open(path, O_CREAT | O_EXCL | O_RDWR, mode);

	if (fd == -1)
		fail_now("open test file");
	close(fd);

	if (stat(path, &st) == -1)
		fail_now("stat test file");

	if (st.st_gid == expected_gid)
		printf("PASS: %s has expected gid %ld\n",
		       path, (long)expected_gid);
	else {
		printf("FAIL: %s gid is %ld, expected %ld\n",
		       path, (long)st.st_gid, (long)expected_gid);
		failed = 1;
	}

	if (expect_sgid < 0)
		return;

	if (!!(st.st_mode & S_ISGID) == !!expect_sgid)
		printf("PASS: %s setgid bit is %s\n",
		       path, expect_sgid ? "set" : "clear");
	else {
		printf("FAIL: %s setgid bit is %s, expected %s\n",
		       path, (st.st_mode & S_ISGID) ? "set" : "clear",
		       expect_sgid ? "set" : "clear");
		failed = 1;
	}
}

int main(void)
{
	struct passwd *pw;

	if (geteuid() != 0) {
		printf("SKIP: open10 needs root to chown directories and switch user\n");
		return EXIT_SKIP;
	}

	pw = getpwnam("nobody");
	if (!pw) {
		printf("SKIP: user nobody not found\n");
		return EXIT_SKIP;
	}

	nobody_uid = pw->pw_uid;
	nobody_gid = pw->pw_gid;
	test_gid = nobody_gid == 65534 ? 65533 : nobody_gid + 1;

	unlink("dir_a/nosetgid");
	unlink("dir_a/setgid");
	unlink("dir_b/nosetgid");
	unlink("dir_b/setgid");
	unlink("dir_b/root_setgid");
	rmdir("dir_a");
	rmdir("dir_b");

	if (mkdir("dir_a", 0777) == -1)
		fail_now("mkdir dir_a");
	if (mkdir("dir_b", 0777) == -1)
		fail_now("mkdir dir_b");

	if (chown("dir_a", nobody_uid, test_gid) == -1 ||
	    chown("dir_b", nobody_uid, test_gid) == -1)
		fail_now("chown test dirs");
	if (chmod("dir_b", S_ISGID | 0777) == -1)
		fail_now("chmod dir_b");

	if (setegid(nobody_gid) == -1 || seteuid(nobody_uid) == -1)
		fail_now("switch to nobody");

	create_and_check("dir_a/nosetgid", 0777, nobody_gid, 0);
	create_and_check("dir_a/setgid", S_ISGID | 0777, nobody_gid, 1);
	create_and_check("dir_b/nosetgid", 0777, test_gid, 0);
	create_and_check("dir_b/setgid", S_ISGID | 0777, test_gid, -1);

	if (seteuid(0) == -1 || setegid(0) == -1)
		fail_now("switch back to root");

	create_and_check("dir_b/root_setgid", S_ISGID | 0777, test_gid, 1);

	unlink("dir_a/nosetgid");
	unlink("dir_a/setgid");
	unlink("dir_b/nosetgid");
	unlink("dir_b/setgid");
	unlink("dir_b/root_setgid");
	rmdir("dir_a");
	rmdir("dir_b");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
