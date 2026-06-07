// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * open11 的简化转写版。
 *
 * 测试目标：
 *   汇总测试 open() 打开普通文件、目录、硬链接、符号链接、设备文件、
 *   O_CREAT、O_DIRECTORY、O_TRUNC 等常见场景。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef O_DIRECTORY
#define O_DIRECTORY 0
#endif

#define MSG "this is a test string"

struct testcase {
	const char *name;
	const char *path;
	int flags;
	mode_t mode;
	int expected_errno;
	int ignore_result;
};

static int failed;

static struct testcase tests[] = {
	{"regular O_RDONLY", "t_reg_empty", O_RDONLY, 0644, 0, 0},
	{"regular O_WRONLY", "t_reg_empty", O_WRONLY, 0644, 0, 0},
	{"regular O_RDWR", "t_reg_empty", O_RDWR, 0644, 0, 0},
	{"regular O_RDWR|O_SYNC", "t_reg_empty", O_RDWR | O_SYNC, 0644, 0, 0},
	{"regular O_RDWR|O_TRUNC", "t_reg_empty", O_RDWR | O_TRUNC, 0644, 0, 0},
	{"directory O_RDONLY", "t_dir", O_RDONLY, 0755, 0, 0},
	{"directory O_RDWR", "t_dir", O_RDWR, 0755, EISDIR, 0},
	{"regular O_DIRECTORY", "t_reg_empty", O_RDONLY | O_DIRECTORY, 0644, ENOTDIR, 0},
	{"hardlink O_RDONLY", "t_link_reg", O_RDONLY, 0644, 0, 0},
	{"hardlink O_WRONLY", "t_link_reg", O_WRONLY, 0644, 0, 0},
	{"hardlink O_RDWR", "t_link_reg", O_RDWR, 0644, 0, 0},
	{"symlink file O_RDONLY", "t_symlink_reg", O_RDONLY, 0644, 0, 0},
	{"symlink file O_WRONLY", "t_symlink_reg", O_WRONLY, 0644, 0, 0},
	{"symlink file O_RDWR", "t_symlink_reg", O_RDWR, 0644, 0, 0},
	{"symlink dir O_RDONLY", "t_symlink_dir", O_RDONLY, 0644, 0, 0},
	{"symlink dir O_WRONLY", "t_symlink_dir", O_WRONLY, 0644, EISDIR, 0},
	{"symlink dir O_RDWR", "t_symlink_dir", O_RDWR, 0644, EISDIR, 0},
	{"device O_RDONLY", "/dev/null", O_RDONLY, 0644, 0, 0},
	{"device O_WRONLY", "/dev/null", O_WRONLY, 0644, 0, 0},
	{"device O_RDWR", "/dev/null", O_RDWR, 0644, 0, 0},
	{"create new file in dir", "t_dir/t_new_reg", O_RDWR | O_CREAT, 0644, 0, 0},
	{"hardlink O_RDONLY|O_CREAT", "t_link_reg", O_RDONLY | O_CREAT, 0644, 0, 0},
	{"symlink file O_RDONLY|O_CREAT", "t_symlink_reg", O_RDONLY | O_CREAT, 0644, 0, 0},
	{"regular O_RDONLY|O_CREAT", "t_reg_empty", O_RDONLY | O_CREAT, 0644, 0, 0},
	{"symlink dir O_RDONLY|O_CREAT", "t_symlink_dir", O_RDONLY | O_CREAT, 0644, EISDIR, 0},
	{"directory O_RDONLY|O_CREAT", "t_dir", O_RDONLY | O_CREAT, 0644, EISDIR, 0},
	{"regular O_RDONLY|O_TRUNC", "t_reg_empty", O_RDONLY | O_TRUNC, 0644, 0, 1},
	{"non-empty regular O_RDONLY|O_TRUNC", "t_reg", O_RDONLY | O_TRUNC, 0644, 0, 1},
};

static void setup_files(void)
{
	int fd = open("t_reg", O_CREAT | O_WRONLY | O_TRUNC, 0644);

	if (fd == -1) {
		perror("create t_reg");
		exit(EXIT_FAILURE);
	}
	write(fd, MSG, sizeof(MSG));
	close(fd);

	fd = open("t_reg_empty", O_CREAT | O_WRONLY | O_TRUNC, 0644);
	if (fd == -1) {
		perror("create t_reg_empty");
		exit(EXIT_FAILURE);
	}
	close(fd);

	link("t_reg", "t_link_reg");
	symlink("t_reg", "t_symlink_reg");
	mkdir("t_dir", 0755);
	symlink("t_dir", "t_symlink_dir");
}

static void cleanup_files(void)
{
	unlink("t_reg");
	unlink("t_reg_empty");
	unlink("t_link_reg");
	unlink("t_symlink_reg");
	unlink("t_symlink_dir");
	unlink("t_dir/t_new_reg");
	rmdir("t_dir");
}

static void run_one(const struct testcase *tc)
{
	int fd;

	errno = 0;
	fd = open(tc->path, tc->flags, tc->mode);

	if (tc->ignore_result) {
		if (fd >= 0)
			close(fd);
		printf("PASS: %s returned without hanging\n", tc->name);
		return;
	}

	if (tc->expected_errno == 0) {
		if (fd >= 0) {
			printf("PASS: %s succeeded\n", tc->name);
			close(fd);
		} else {
			printf("FAIL: %s failed with %s\n", tc->name, strerror(errno));
			failed = 1;
		}
		return;
	}

	if (fd >= 0) {
		close(fd);
#ifndef __linux__
		if (tc->expected_errno == EISDIR) {
			printf("SKIP: %s succeeds on this OS but fails with EISDIR on Linux\n",
			       tc->name);
			return;
		}
#endif
		printf("FAIL: %s unexpectedly succeeded\n", tc->name);
		failed = 1;
	} else if (errno == tc->expected_errno) {
		printf("PASS: %s failed with %s\n",
		       tc->name, strerror(tc->expected_errno));
	} else {
		printf("FAIL: %s expected %s but got %s\n",
		       tc->name, strerror(tc->expected_errno), strerror(errno));
		failed = 1;
	}
}

int main(void)
{
	size_t i;

	cleanup_files();
	setup_files();

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++)
		run_one(&tests[i]);

	cleanup_files();
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
