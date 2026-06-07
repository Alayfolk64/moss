// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * linkat02 的简化转写版。
 *
 * 测试目标：
 *   覆盖 linkat() 常见失败路径：ENAMETOOLONG、EEXIST、ELOOP、
 *   EACCES，以及原始用例中需要特殊环境的 EROFS、EMLINK 场景。
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
#include <sys/types.h>
#include <unistd.h>

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif
#ifndef AT_FDCWD
#define AT_FDCWD -100
#endif
#ifndef AT_SYMLINK_FOLLOW
#define AT_SYMLINK_FOLLOW 0x400
#endif

#define TEST_FILE "linkat02_testfile"
#define TEST_EXIST "linkat02_testexist"
#define TEST_ELOOP "linkat02_eloop"
#define TEST_ELOOP2 "linkat02_eloop2"

static int failed;
static char long_path[PATH_MAX + 2];

static void cleanup(void)
{
	chmod("linkat02_tmp", 0755);

	unlink("linkat02_new");
	unlink("linkat02_tmp/source");
	unlink("linkat02_tmp/new");
	unlink(TEST_FILE);
	unlink(TEST_EXIST);
	unlink(TEST_ELOOP);
	unlink(TEST_ELOOP2);

	rmdir("linkat02_tmp");
}

static void create_file(const char *path, mode_t mode)
{
	int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, mode);

	if (fd == -1) {
		perror(path);
		cleanup();
		exit(EXIT_FAILURE);
	}
	close(fd);
}

static void expect_errno_linkat(const char *name, const char *oldpath,
				const char *newpath, int flags, int expected)
{
	errno = 0;
	if (linkat(AT_FDCWD, oldpath, AT_FDCWD, newpath, flags) == 0) {
		printf("FAIL: %s unexpectedly succeeded\n", name);
		unlink(newpath);
		failed = 1;
		return;
	}

	if (errno == expected) {
		printf("PASS: %s failed with %s\n", name, strerror(expected));
		return;
	}

	printf("FAIL: %s expected %s but got %s\n",
	       name, strerror(expected), strerror(errno));
	failed = 1;
}

static void run_basic_error_cases(void)
{
	expect_errno_linkat("目标路径过长",
			    TEST_FILE, long_path, 0, ENAMETOOLONG);
	expect_errno_linkat("源路径过长",
			    long_path, "linkat02_new", 0, ENAMETOOLONG);
	expect_errno_linkat("目标文件已经存在",
			    TEST_EXIST, TEST_EXIST, 0, EEXIST);
	expect_errno_linkat("跟随符号链接时遇到循环",
			    TEST_ELOOP, "linkat02_new",
			    AT_SYMLINK_FOLLOW, ELOOP);
}

static void run_eacces_case(void)
{
	struct passwd *pw;

	if (geteuid() != 0) {
		printf("SKIP: EACCES case needs root to switch to nobody\n");
		return;
	}

	pw = getpwnam("nobody");
	if (!pw) {
		printf("SKIP: user nobody was not found\n");
		return;
	}

	if (mkdir("linkat02_tmp", 0755) == -1) {
		perror("mkdir linkat02_tmp");
		failed = 1;
		return;
	}
	create_file("linkat02_tmp/source", 0644);
	chown("linkat02_tmp/source", pw->pw_uid, pw->pw_gid);

	/* tmp 目录可搜索但不可写，nobody 不能在里面创建新的硬链接名。 */
	chmod("linkat02_tmp", 0555);

	if (seteuid(pw->pw_uid) == -1) {
		perror("seteuid nobody");
		failed = 1;
		return;
	}

	expect_errno_linkat("目标目录不可写",
			    "linkat02_tmp/source",
			    "linkat02_tmp/new",
			    0,
			    EACCES);

	if (seteuid(0) == -1) {
		perror("restore root euid");
		failed = 1;
	}
}

static void explain_environment_only_cases(void)
{
	/*
	 * EROFS 需要真实的只读挂载点；EMLINK 需要填满文件系统硬链接上限。
	 * 简化版不主动 mount，也不暴力创建成千上万个链接，避免测试程序本身
	 * 变得不透明。
	 */
	printf("SKIP: EROFS case needs a prepared read-only mount\n");
	printf("SKIP: EMLINK case depends on filesystem hard-link limit\n");
}

int main(void)
{
	cleanup();
	memset(long_path, 'a', sizeof(long_path) - 1);
	long_path[sizeof(long_path) - 1] = '\0';

	create_file(TEST_FILE, 0644);
	create_file(TEST_EXIST, 0644);
	if (symlink(TEST_ELOOP2, TEST_ELOOP) == -1 ||
	    symlink(TEST_ELOOP, TEST_ELOOP2) == -1) {
		perror("create symlink loop");
		cleanup();
		return EXIT_FAILURE;
	}

	run_basic_error_cases();
	run_eacces_case();
	explain_environment_only_cases();

	cleanup();
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
