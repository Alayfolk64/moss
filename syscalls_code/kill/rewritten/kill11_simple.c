// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * kill11 的简化转写版。
 *
 * 测试目标：
 *   父进程用不同信号杀死子进程，waitpid() 返回的终止信号和 core dump
 *   标志应该符合预期。
 */

#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif

#define EXIT_SKIP 77
#define MIN_CORE_LIMIT (512 * 1024)

struct signal_case {
	int signal_number;
	int should_dump_core;
	const char *name;
};

static const struct signal_case tests[] = {
	{ SIGHUP, 0, "SIGHUP" },
	{ SIGINT, 0, "SIGINT" },
	{ SIGQUIT, 1, "SIGQUIT" },
	{ SIGILL, 1, "SIGILL" },
	{ SIGTRAP, 1, "SIGTRAP" },
	{ SIGABRT, 1, "SIGABRT" },
#if defined(SIGIOT) && SIGIOT != SIGABRT
	{ SIGIOT, 1, "SIGIOT" },
#endif
	{ SIGBUS, 1, "SIGBUS" },
	{ SIGFPE, 1, "SIGFPE" },
	{ SIGKILL, 0, "SIGKILL" },
	{ SIGUSR1, 0, "SIGUSR1" },
	{ SIGSEGV, 1, "SIGSEGV" },
	{ SIGUSR2, 0, "SIGUSR2" },
	{ SIGPIPE, 0, "SIGPIPE" },
	{ SIGALRM, 0, "SIGALRM" },
	{ SIGTERM, 0, "SIGTERM" },
#ifdef SIGSTKFLT
	{ SIGSTKFLT, 0, "SIGSTKFLT" },
#endif
	{ SIGXCPU, 1, "SIGXCPU" },
	{ SIGXFSZ, 1, "SIGXFSZ" },
	{ SIGVTALRM, 0, "SIGVTALRM" },
	{ SIGPROF, 0, "SIGPROF" },
	{ SIGIO, 0, "SIGIO" },
#ifdef SIGPWR
	{ SIGPWR, 0, "SIGPWR" },
#endif
	{ SIGSYS, 1, "SIGSYS" },
};

static char old_cwd[PATH_MAX];
static char temp_dir[] = "/tmp/kill11_simple_XXXXXX";
static int temp_dir_ready;

static void fail_now(const char *message)
{
	perror(message);
	exit(EXIT_FAILURE);
}

static int prepare_core_dump_dir(void)
{
	struct rlimit limit;

	if (getrlimit(RLIMIT_CORE, &limit) == -1)
		fail_now("getrlimit(RLIMIT_CORE)");

	if (limit.rlim_max == 0) {
		printf("SKIP: hard RLIMIT_CORE is 0, core-dump bit cannot be tested\n");
		return -1;
	}

	if (limit.rlim_cur < MIN_CORE_LIMIT) {
		rlim_t new_limit = limit.rlim_max < MIN_CORE_LIMIT ?
				   limit.rlim_max : MIN_CORE_LIMIT;

		limit.rlim_cur = new_limit;
		if (setrlimit(RLIMIT_CORE, &limit) == -1)
			fail_now("setrlimit(RLIMIT_CORE)");
	}

	if (!getcwd(old_cwd, sizeof(old_cwd)))
		fail_now("getcwd");

	if (!mkdtemp(temp_dir))
		fail_now("mkdtemp");

	if (chdir(temp_dir) == -1)
		fail_now("chdir temp dir");

	temp_dir_ready = 1;
	return 0;
}

static void cleanup_core_dump_dir(void)
{
	DIR *dir;
	struct dirent *entry;

	if (!temp_dir_ready)
		return;

	chdir(old_cwd);

	dir = opendir(temp_dir);
	if (dir) {
		while ((entry = readdir(dir)) != NULL) {
			char path[PATH_MAX];

			if (!strcmp(entry->d_name, ".") ||
			    !strcmp(entry->d_name, ".."))
				continue;

			snprintf(path, sizeof(path), "%s/%s", temp_dir,
				 entry->d_name);
			unlink(path);
		}
		closedir(dir);
	}

	rmdir(temp_dir);
}

static int run_one_signal(const struct signal_case *test)
{
	pid_t pid;
	int status;
	int got_signal;
	int dumped_core = 0;
	int waited;

	pid = fork();
	if (pid == -1)
		fail_now("fork");

	if (pid == 0) {
		struct sigaction action;

		memset(&action, 0, sizeof(action));
		action.sa_handler = SIG_DFL;
		sigemptyset(&action.sa_mask);
		sigaction(test->signal_number, &action, NULL);

		for (;;)
			pause();
	}

	if (kill(pid, test->signal_number) == -1)
		fail_now("kill child");

	for (waited = 0; waited < 50; waited++) {
		pid_t ret = waitpid(pid, &status, WNOHANG);

		if (ret == pid)
			break;
		if (ret == -1)
			fail_now("waitpid");

		usleep(100000);
	}

	if (waited == 50) {
		kill(pid, SIGKILL);
		waitpid(pid, NULL, 0);
#ifdef __linux__
		printf("FAIL: %s did not terminate the child within timeout\n",
		       test->name);
		return 1;
#else
		printf("SKIP: %s does not terminate the child by default on this OS\n",
		       test->name);
		return 0;
#endif
	}

	if (!WIFSIGNALED(status)) {
		printf("FAIL: %s did not terminate the child by signal\n",
		       test->name);
		return 1;
	}

	got_signal = WTERMSIG(status);
	if (got_signal != test->signal_number) {
		printf("FAIL: %s expected signal %d but waitpid reported %d\n",
		       test->name, test->signal_number, got_signal);
		return 1;
	}

#ifdef WCOREDUMP
	dumped_core = WCOREDUMP(status) ? 1 : 0;
	if (dumped_core != test->should_dump_core) {
#ifdef __linux__
		printf("FAIL: %s core bit expected %d but got %d\n",
		       test->name, test->should_dump_core, dumped_core);
		return 1;
#else
		printf("SKIP: %s core bit differs on this OS "
		       "(expected %d, got %d)\n",
		       test->name, test->should_dump_core, dumped_core);
		return 0;
#endif
	}
#endif

	printf("PASS: %-8s terminated child%s\n", test->name,
	       dumped_core ? " and set core bit" : "");
	return 0;
}

int main(void)
{
	size_t i;
	int failed = 0;

	if (prepare_core_dump_dir() == -1)
		return EXIT_SKIP;

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++)
		failed += run_one_signal(&tests[i]);

	cleanup_core_dump_dir();
	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
