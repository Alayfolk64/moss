// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * link02 的简化转写版。
 *
 * 测试目标：
 *   link(oldpath, newpath) 能创建硬链接；创建后两个路径应指向同一个
 *   inode，并且链接计数应同步增加。
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define OLD_PATH "link02_oldpath"
#define NEW_PATH "link02_newpath"

static void cleanup(void)
{
	unlink(NEW_PATH);
	unlink(OLD_PATH);
}

static void fail_now(const char *msg)
{
	perror(msg);
	cleanup();
	exit(EXIT_FAILURE);
}

int main(void)
{
	struct stat old_stat;
	struct stat new_stat;
	int fd;
	int same_inode;
	int link_count_ok;

	cleanup();

	fd = open(OLD_PATH, O_CREAT | O_WRONLY | O_TRUNC, 0700);
	if (fd == -1)
		fail_now("create source file");
	if (write(fd, "link02\n", 7) != 7) {
		close(fd);
		fail_now("write source file");
	}
	close(fd);

	/* 核心动作：给同一个文件增加第二个目录项。 */
	if (link(OLD_PATH, NEW_PATH) == -1) {
		printf("FAIL: link() failed unexpectedly: %s\n", strerror(errno));
		cleanup();
		return EXIT_FAILURE;
	}

	if (stat(OLD_PATH, &old_stat) == -1)
		fail_now("stat old path");
	if (stat(NEW_PATH, &new_stat) == -1)
		fail_now("stat new path");

	same_inode = old_stat.st_dev == new_stat.st_dev &&
		     old_stat.st_ino == new_stat.st_ino;
	link_count_ok = old_stat.st_nlink > 1 &&
			old_stat.st_nlink == new_stat.st_nlink;

	if (same_inode && link_count_ok) {
		printf("PASS: link() created a hard link to the same inode\n");
		cleanup();
		return EXIT_SUCCESS;
	}

	printf("FAIL: hard-link metadata mismatch: old nlink=%lu, new nlink=%lu\n",
	       (unsigned long)old_stat.st_nlink,
	       (unsigned long)new_stat.st_nlink);
	cleanup();
	return EXIT_FAILURE;
}
