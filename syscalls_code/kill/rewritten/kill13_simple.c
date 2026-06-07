// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * kill13 的简化转写版。
 *
 * 测试目标：
 *   kill(INT_MIN, 0) 不应该触发内核里的 INT_MIN 取反问题；
 *   对用户态来说，它应该失败并返回 errno=ESRCH。
 */

#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>

int main(void)
{
	errno = 0;
	if (kill((pid_t)INT_MIN, 0) != -1) {
		printf("FAIL: kill(INT_MIN, 0) unexpectedly succeeded\n");
		return EXIT_FAILURE;
	}

	if (errno != ESRCH) {
		printf("FAIL: expected ESRCH but got errno=%d (%s)\n",
		       errno, strerror(errno));
		return EXIT_FAILURE;
	}

	printf("PASS: kill(INT_MIN, 0) failed with ESRCH\n");
	return EXIT_SUCCESS;
}
