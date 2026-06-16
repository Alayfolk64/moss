// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap11 的简化转写版。
 *
 * 测试目标：
 *   反复创建 detached 线程，让线程退出路径触发 libc 的栈 munmap 清理，
 *   确认该路径不会异常失败。
 */

#include "mmap_simple_common.h"
#include <pthread.h>

#define THREADS 128
#define MAL_SIZE (100 * 1024)

static void *worker_alloc(void *arg)
{
	void *p;

	(void)arg;
	p = malloc(MAL_SIZE);
	if (p) {
		memset(p, 1, MAL_SIZE);
		free(p);
	}
	return NULL;
}

static void *worker_empty(void *arg)
{
	(void)arg;
	return NULL;
}

int main(void)
{
	pthread_attr_t attr;
	pthread_t th;
	int failed = 0;
	int i;
	int ret;

	ret = pthread_attr_init(&attr);
	if (ret) {
		errno = ret;
		fail_now("pthread_attr_init");
	}
	ret = pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
	if (ret) {
		errno = ret;
		fail_now("pthread_attr_setdetachstate");
	}

	for (i = 0; i < THREADS; i++) {
		ret = pthread_create(&th, &attr, worker_alloc, NULL);
		if (ret) {
			errno = ret;
			perror("pthread_create worker_alloc");
			failed = 1;
			break;
		}
		ret = pthread_create(&th, &attr, worker_empty, NULL);
		if (ret) {
			errno = ret;
			perror("pthread_create worker_empty");
			failed = 1;
			break;
		}
	}

	pthread_attr_destroy(&attr);
	sleep(1);
	if (!failed)
		pass_msg("thread exit munmap path completed");
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
