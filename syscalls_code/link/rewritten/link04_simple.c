// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * link04 的简化转写版。
 *
 * 测试目标：
 *   覆盖 link() 常见失败路径：ENOENT、ENOTDIR、ENAMETOOLONG、
 *   EFAULT、EEXIST、EACCES。
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

#define BAD_PATH ((const char *)-1)

static int failed;
static char long_path[PATH_MAX + 2];

struct test_case {
	const char *name;
	const char *oldpath;
	const char *newpath;
	int expected_errno;
};

static void cleanup(void)
{
	chmod("link04_no_write_dir", 0755);
	chmod("link04_no_search_dir", 0755);
	chmod("link04_no_search_dir/sub", 0755);

	unlink("link04_new");
	unlink("link04_regfile");
	unlink("link04_regfile2");
	unlink("link04_parent_file");
	unlink("link04_no_write_dir/source");
	unlink("link04_no_write_dir/new");
	unlink("link04_no_search_dir/sub/source");
	unlink("link04_nobody_dest/new");

	rmdir("link04_no_write_dir");
	rmdir("link04_no_search_dir/sub");
	rmdir("link04_no_search_dir");
	rmdir("link04_nobody_dest");
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

static int path_is_printable(const char *path)
{
	return path && path != BAD_PATH;
}

static void expect_errno_link(const char *name, const char *oldpath,
			      const char *newpath, int expected)
{
	errno = 0;
	if (link(oldpath, newpath) == 0) {
		printf("FAIL: %s unexpectedly succeeded\n", name);
		if (path_is_printable(newpath))
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

static void run_path_error_cases(void)
{
	struct test_case cases[] = {
		{
			"源文件不存在",
			"link04_missing_file",
			"link04_new",
			ENOENT,
		},
		{
			"源路径为空字符串",
			"",
			"link04_new",
			ENOENT,
		},
		{
			"源路径中间目录不存在",
			"link04_missing_dir/file",
			"link04_new",
			ENOENT,
		},
		{
			"源路径把普通文件当成目录",
			"link04_parent_file/child",
			"link04_new",
			ENOTDIR,
		},
		{
			"源路径过长",
			long_path,
			"link04_new",
			ENAMETOOLONG,
		},
		{
			"源路径指针非法",
			BAD_PATH,
			"link04_new",
			EFAULT,
		},
		{
			"目标路径为空字符串",
			"link04_regfile",
			"",
			ENOENT,
		},
		{
			"目标路径中间目录不存在",
			"link04_regfile",
			"link04_missing_dir/file",
			ENOENT,
		},
		{
			"目标路径把普通文件当成目录",
			"link04_regfile",
			"link04_parent_file/child",
			ENOTDIR,
		},
		{
			"目标路径过长",
			"link04_regfile",
			long_path,
			ENAMETOOLONG,
		},
		{
			"目标路径指针非法",
			"link04_regfile",
			BAD_PATH,
			EFAULT,
		},
		{
			"目标文件已经存在",
			"link04_regfile",
			"link04_regfile2",
			EEXIST,
		},
	};
	size_t i;

	for (i = 0; i < sizeof(cases) / sizeof(cases[0]); i++)
		expect_errno_link(cases[i].name, cases[i].oldpath,
				  cases[i].newpath, cases[i].expected_errno);
}

static void run_eacces_cases(void)
{
	struct passwd *pw;

	/*
	 * EACCES 用例需要先由 root 准备权限，再切到 nobody 触发权限错误。
	 * 普通用户运行时保留其它用例，权限用例明确标记为 SKIP。
	 */
	if (geteuid() != 0) {
		printf("SKIP: EACCES cases need root to switch to nobody\n");
		return;
	}

	pw = getpwnam("nobody");
	if (!pw) {
		printf("SKIP: user nobody was not found\n");
		return;
	}

	if (mkdir("link04_no_write_dir", 0755) == -1 ||
	    mkdir("link04_no_search_dir", 0755) == -1 ||
	    mkdir("link04_no_search_dir/sub", 0755) == -1 ||
	    mkdir("link04_nobody_dest", 0755) == -1) {
		perror("mkdir for EACCES cases");
		failed = 1;
		return;
	}

	create_file("link04_no_write_dir/source", 0644);
	create_file("link04_no_search_dir/sub/source", 0644);

	chown("link04_no_write_dir/source", pw->pw_uid, pw->pw_gid);
	chown("link04_no_search_dir/sub/source", pw->pw_uid, pw->pw_gid);
	chown("link04_nobody_dest", pw->pw_uid, pw->pw_gid);

	/* 目录可搜索但不可写：目标目录无法创建新目录项。 */
	chmod("link04_no_write_dir", 0555);
	/* 目录没有执行权限：路径解析阶段就会失败。 */
	chmod("link04_no_search_dir", 0666);

	if (seteuid(pw->pw_uid) == -1) {
		perror("seteuid nobody");
		failed = 1;
		return;
	}

	expect_errno_link("目标目录不可写",
			  "link04_no_write_dir/source",
			  "link04_no_write_dir/new",
			  EACCES);
	expect_errno_link("源路径目录不可搜索",
			  "link04_no_search_dir/sub/source",
			  "link04_nobody_dest/new",
			  EACCES);

	if (seteuid(0) == -1) {
		perror("restore root euid");
		failed = 1;
	}
}

int main(void)
{
	cleanup();
	memset(long_path, 'a', sizeof(long_path) - 1);
	long_path[sizeof(long_path) - 1] = '\0';

	create_file("link04_regfile", 0777);
	create_file("link04_regfile2", 0777);
	create_file("link04_parent_file", 0777);

	run_path_error_cases();
	run_eacces_cases();

	cleanup();
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
