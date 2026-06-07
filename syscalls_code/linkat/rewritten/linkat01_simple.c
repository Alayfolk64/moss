// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * linkat01 的简化转写版。
 *
 * 测试目标：
 *   验证 linkat() 对目录文件描述符、绝对路径、AT_FDCWD、无效 fd、
 *   已删除目录 fd、目录源路径、非法 flags、跨文件系统来源的基本处理。
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif
#ifndef AT_FDCWD
#define AT_FDCWD -100
#endif
#ifndef O_DIRECTORY
#define O_DIRECTORY 0
#endif

#define OLD_DIR "linkat01_olddir"
#define NEW_DIR "linkat01_newdir"
#define DEL_DIR "linkat01_deldir"
#define OLD_FILE "oldfile"
#define NEW_FILE "newfile"
#define OLD_PATH OLD_DIR "/" OLD_FILE
#define NEW_PATH NEW_DIR "/" NEW_FILE

static int failed;
static int olddirfd = -1;
static int newdirfd = -1;
static int deldirfd = -1;
static int filefd = -1;
static char abs_old[PATH_MAX];
static char abs_new[PATH_MAX];

struct linkat_case {
	const char *name;
	int *oldfd;
	const char *oldpath;
	int *newfd;
	const char *newpath;
	int flags;
	int expected_errno;
};

static void close_if_open(int *fd)
{
	if (*fd != -1) {
		close(*fd);
		*fd = -1;
	}
}

static void cleanup(void)
{
	close_if_open(&filefd);
	close_if_open(&olddirfd);
	close_if_open(&newdirfd);
	close_if_open(&deldirfd);

	unlink(NEW_PATH);
	unlink(OLD_PATH);
	rmdir(NEW_DIR);
	rmdir(OLD_DIR);
	rmdir(DEL_DIR);
}

static void fail_now(const char *msg)
{
	perror(msg);
	cleanup();
	exit(EXIT_FAILURE);
}

static void create_source_file(void)
{
	int fd = open(OLD_PATH, O_CREAT | O_WRONLY | O_TRUNC, 0600);

	if (fd == -1)
		fail_now("create old file");
	if (write(fd, "linkat01 source\n", 16) != 16) {
		close(fd);
		fail_now("write old file");
	}
	close(fd);
}

static void setup(void)
{
	char cwd[PATH_MAX];

	cleanup();

	if (mkdir(OLD_DIR, 0700) == -1)
		fail_now("mkdir olddir");
	if (mkdir(NEW_DIR, 0700) == -1)
		fail_now("mkdir newdir");
	if (mkdir(DEL_DIR, 0700) == -1)
		fail_now("mkdir deldir");

	create_source_file();

	olddirfd = open(OLD_DIR, O_RDONLY | O_DIRECTORY);
	if (olddirfd == -1)
		fail_now("open olddir");
	newdirfd = open(NEW_DIR, O_RDONLY | O_DIRECTORY);
	if (newdirfd == -1)
		fail_now("open newdir");
	deldirfd = open(DEL_DIR, O_RDONLY | O_DIRECTORY);
	if (deldirfd == -1)
		fail_now("open deldir");
	if (rmdir(DEL_DIR) == -1)
		fail_now("remove deldir while fd is still open");

	filefd = open(OLD_PATH, O_RDONLY);
	if (filefd == -1)
		fail_now("open regular file fd");

	if (!getcwd(cwd, sizeof(cwd)))
		fail_now("getcwd");
	snprintf(abs_old, sizeof(abs_old), "%s/%s", cwd, OLD_PATH);
	snprintf(abs_new, sizeof(abs_new), "%s/%s", cwd, NEW_PATH);
}

static void reset_destination(void)
{
	unlink(NEW_PATH);
}

static void verify_created_link(const char *name)
{
	struct stat old_stat;
	struct stat new_stat;

	if (stat(OLD_PATH, &old_stat) == -1 ||
	    stat(NEW_PATH, &new_stat) == -1) {
		printf("FAIL: %s could not stat both paths: %s\n",
		       name, strerror(errno));
		failed = 1;
		return;
	}

	if (old_stat.st_dev == new_stat.st_dev &&
	    old_stat.st_ino == new_stat.st_ino &&
	    old_stat.st_nlink == new_stat.st_nlink &&
	    old_stat.st_nlink > 1) {
		printf("PASS: %s created a hard link to the same inode\n", name);
		return;
	}

	printf("FAIL: %s created mismatched metadata\n", name);
	failed = 1;
}

static void run_one_case(const struct linkat_case *tc)
{
	reset_destination();
	errno = 0;

	if (linkat(*tc->oldfd, tc->oldpath, *tc->newfd, tc->newpath,
		   tc->flags) == 0) {
		if (tc->expected_errno == 0) {
			verify_created_link(tc->name);
		} else {
			printf("FAIL: %s unexpectedly succeeded\n", tc->name);
			failed = 1;
		}
		reset_destination();
		return;
	}

	if (tc->expected_errno == 0) {
		printf("FAIL: %s failed unexpectedly: %s\n",
		       tc->name, strerror(errno));
		failed = 1;
		return;
	}

	if (errno == tc->expected_errno) {
		printf("PASS: %s failed with %s\n",
		       tc->name, strerror(tc->expected_errno));
		return;
	}

	printf("FAIL: %s expected %s but got %s\n",
	       tc->name, strerror(tc->expected_errno), strerror(errno));
	failed = 1;
}

static void test_cross_device(void)
{
	struct stat proc_stat;
	struct stat cwd_stat;

	reset_destination();

	/* 使用 /proc/cpuinfo 模拟跨文件系统来源；没有 /proc 时明确跳过。 */
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
	if (linkat(AT_FDCWD, "/proc/cpuinfo", newdirfd, NEW_FILE, 0) == 0) {
		printf("FAIL: cross-device linkat unexpectedly succeeded\n");
		reset_destination();
		failed = 1;
		return;
	}

	if (errno == EXDEV) {
		printf("PASS: cross-device linkat failed with EXDEV\n");
		return;
	}

	if (errno == EPERM || errno == EACCES) {
		printf("SKIP: /proc rejected hard links before EXDEV check: %s\n",
		       strerror(errno));
		return;
	}

	printf("FAIL: cross-device linkat expected EXDEV but got %s\n",
	       strerror(errno));
	failed = 1;
}

int main(void)
{
	int cwd_fd = AT_FDCWD;
	int badfd = -1;
	struct linkat_case cases[] = {
		{
			"相对路径分别按 olddirfd/newdirfd 解释",
			&olddirfd,
			OLD_FILE,
			&newdirfd,
			NEW_FILE,
			0,
			0,
		},
		{
			"源路径是绝对路径时忽略 olddirfd",
			&badfd,
			abs_old,
			&newdirfd,
			NEW_FILE,
			0,
			0,
		},
		{
			"目标路径是绝对路径时忽略 newdirfd",
			&olddirfd,
			OLD_FILE,
			&badfd,
			abs_new,
			0,
			0,
		},
		{
			"源路径相对当前目录",
			&cwd_fd,
			OLD_PATH,
			&newdirfd,
			NEW_FILE,
			0,
			0,
		},
		{
			"目标路径相对当前目录",
			&olddirfd,
			OLD_FILE,
			&cwd_fd,
			NEW_PATH,
			0,
			0,
		},
		{
			"非目录 oldfd 搭配相对源路径",
			&filefd,
			OLD_FILE,
			&newdirfd,
			NEW_FILE,
			0,
			ENOTDIR,
		},
		{
			"非目录 newfd 搭配相对目标路径",
			&olddirfd,
			OLD_FILE,
			&filefd,
			NEW_FILE,
			0,
			ENOTDIR,
		},
		{
			"无效 oldfd 搭配相对源路径",
			&badfd,
			OLD_FILE,
			&newdirfd,
			NEW_FILE,
			0,
			EBADF,
		},
		{
			"无效 newfd 搭配相对目标路径",
			&olddirfd,
			OLD_FILE,
			&badfd,
			NEW_FILE,
			0,
			EBADF,
		},
		{
			"已经删除的 oldfd 目录",
			&deldirfd,
			OLD_FILE,
			&newdirfd,
			NEW_FILE,
			0,
			ENOENT,
		},
		{
			"已经删除的 newfd 目录",
			&olddirfd,
			OLD_FILE,
			&deldirfd,
			NEW_FILE,
			0,
			ENOENT,
		},
		{
			"源路径是目录",
			&olddirfd,
			".",
			&newdirfd,
			NEW_FILE,
			0,
			EPERM,
		},
		{
			"flags 包含非法位",
			&olddirfd,
			OLD_FILE,
			&newdirfd,
			NEW_FILE,
			1,
			EINVAL,
		},
	};
	size_t i;

	setup();

	for (i = 0; i < sizeof(cases) / sizeof(cases[0]); i++)
		run_one_case(&cases[i]);

	test_cross_device();
	cleanup();
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
