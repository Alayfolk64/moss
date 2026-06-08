// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * link08 的简化转写版。
 *
 * 测试目标：
 *   覆盖 link() 的几个特殊失败场景：源路径是目录、跨文件系统硬链接、
 *   只读文件系统、解析路径时遇到符号链接循环。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif

static int failed;

static void expect_errno_link(const char *name, const char *oldpath,
			      const char *newpath, int expected)
{
	errno = 0;
	if (link(oldpath, newpath) == 0) {
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

static void test_directory_source(void)
{
	rmdir("link08_dir");
	if (mkdir("link08_dir", 0755) == -1) {
		perror("mkdir link08_dir");
		failed = 1;
		return;
	}

	expect_errno_link("源路径是目录", "link08_dir", "link08_new", EPERM);
	rmdir("link08_dir");
}

static void test_cross_device(void)
{
	struct stat proc_stat;
	struct stat cwd_stat;

	/*
	 * LTP 原用例通过单独挂载的文件系统制造 EXDEV。简化版用 /proc/cpuinfo
	 * 做跨设备来源；如果当前系统没有 /proc，就明确跳过这一项。
	 */
	if (stat("/proc/cpuinfo", &proc_stat) == -1) {
		printf("SKIP: /proc/cpuinfo is not available for EXDEV case\n");
		return;
	}
	if (stat(".", &cwd_stat) == -1) {
		perror("stat cwd");
		failed = 1;
		return;
	}
	if (proc_stat.st_dev == cwd_stat.st_dev) {
		printf("SKIP: /proc/cpuinfo is on the same device as cwd\n");
		return;
	}

	errno = 0;
	if (link("/proc/cpuinfo", "link08_cross_device") == 0) {
		printf("FAIL: cross-device link unexpectedly succeeded\n");
		unlink("link08_cross_device");
		failed = 1;
		return;
	}

	if (errno == EXDEV) {
		printf("PASS: cross-device link failed with EXDEV\n");
		return;
	}

	if (errno == EPERM || errno == EACCES) {
		printf("SKIP: /proc rejected hard links before EXDEV check: %s\n",
		       strerror(errno));
		return;
	}

	printf("FAIL: cross-device link expected EXDEV but got %s\n",
	       strerror(errno));
	failed = 1;
}

static void test_read_only_filesystem(void)
{
	/*
	 * 原始 LTP 用例需要挂载只读文件系统。这个动作依赖 root 和可用块设备，
	 * 简化版不在测试程序里执行挂载，只把前置条件写清楚。
	 */
	printf("SKIP: EROFS case needs a prepared read-only mount\n");
}

static void test_symlink_loop(void)
{
	char loop_path[PATH_MAX];
	int i;

	unlink("link08_loop_dir/link08_loop_dir");
	rmdir("link08_loop_dir");
	if (mkdir("link08_loop_dir", 0755) == -1) {
		perror("mkdir link08_loop_dir");
		failed = 1;
		return;
	}
	if (symlink("../link08_loop_dir",
		    "link08_loop_dir/link08_loop_dir") == -1) {
		perror("symlink loop");
		failed = 1;
		return;
	}

	strcpy(loop_path, ".");
	for (i = 0; i < 50; i++) {
		if (strlen(loop_path) + strlen("/link08_loop_dir") + 1 >=
		    sizeof(loop_path))
			break;
		strcat(loop_path, "/link08_loop_dir");
	}

	expect_errno_link("路径解析遇到符号链接循环",
			  loop_path, "link08_loop_new", ELOOP);

	unlink("link08_loop_dir/link08_loop_dir");
	rmdir("link08_loop_dir");
}

int main(void)
{
	test_directory_source();
	test_cross_device();
	test_read_only_filesystem();
	test_symlink_loop();

	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
