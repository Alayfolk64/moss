// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * link05 的简化转写版。
 *
 * 测试目标：
 *   对同一个源文件连续创建 1000 个硬链接；所有路径都应指向同一个
 *   inode，并看到相同的链接计数。
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define BASE_NAME "link05_lkfile"
#define NLINKS 1000

static void link_name(char *buf, size_t size, int index)
{
	snprintf(buf, size, "%s_%d", BASE_NAME, index);
}

static void cleanup_links(int created)
{
	char name[128];
	int i;

	for (i = 1; i < created; i++) {
		link_name(name, sizeof(name), i);
		unlink(name);
	}
	unlink(BASE_NAME);
}

int main(void)
{
	struct stat base_stat;
	struct stat link_stat;
	char name[128];
	int created = 1;
	int i;
	int fd;

	cleanup_links(NLINKS);

	fd = open(BASE_NAME, O_CREAT | O_WRONLY | O_TRUNC, 0700);
	if (fd == -1) {
		perror("create base file");
		return EXIT_FAILURE;
	}
	close(fd);

	for (i = 1; i < NLINKS; i++) {
		link_name(name, sizeof(name), i);
		if (link(BASE_NAME, name) == -1) {
			printf("FAIL: link %d failed: %s\n", i, strerror(errno));
			cleanup_links(created);
			return EXIT_FAILURE;
		}
		created++;
	}

	if (stat(BASE_NAME, &base_stat) == -1) {
		perror("stat base file");
		cleanup_links(created);
		return EXIT_FAILURE;
	}

	for (i = 1; i < created; i++) {
		link_name(name, sizeof(name), i);
		if (stat(name, &link_stat) == -1) {
			perror("stat link");
			cleanup_links(created);
			return EXIT_FAILURE;
		}

		if (base_stat.st_dev != link_stat.st_dev ||
		    base_stat.st_ino != link_stat.st_ino ||
		    base_stat.st_nlink != link_stat.st_nlink) {
			printf("FAIL: %s does not match the base file metadata\n",
			       name);
			cleanup_links(created);
			return EXIT_FAILURE;
		}
	}

	if ((int)base_stat.st_nlink == NLINKS) {
		printf("PASS: created %d hard links with matching link counts\n",
		       NLINKS);
		cleanup_links(created);
		return EXIT_SUCCESS;
	}

	printf("FAIL: expected nlink=%d but got %lu\n",
	       NLINKS, (unsigned long)base_stat.st_nlink);
	cleanup_links(created);
	return EXIT_FAILURE;
}
