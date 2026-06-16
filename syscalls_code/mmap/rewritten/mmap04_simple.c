// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mmap04 的简化转写版。
 *
 * 测试目标：
 *   mmap() 的 prot/flags 组合应在 /proc/self/maps 中反映为预期权限位。
 */

#include <inttypes.h>
#include "mmap_simple_common.h"

struct testcase {
	int prot;
	int flags;
	const char *expected;
};

static struct testcase tests[] = {
	{PROT_NONE, MAP_ANONYMOUS | MAP_PRIVATE, "---p"},
	{PROT_NONE, MAP_ANONYMOUS | MAP_SHARED, "---s"},
	{PROT_READ, MAP_ANONYMOUS | MAP_PRIVATE, "r--p"},
	{PROT_READ, MAP_ANONYMOUS | MAP_SHARED, "r--s"},
	{PROT_WRITE, MAP_ANONYMOUS | MAP_PRIVATE, "-w-p"},
	{PROT_WRITE, MAP_ANONYMOUS | MAP_SHARED, "-w-s"},
	{PROT_READ | PROT_WRITE, MAP_ANONYMOUS | MAP_PRIVATE, "rw-p"},
	{PROT_READ | PROT_WRITE, MAP_ANONYMOUS | MAP_SHARED, "rw-s"},
	{PROT_READ | PROT_EXEC, MAP_ANONYMOUS | MAP_PRIVATE, "r-xp"},
	{PROT_READ | PROT_EXEC, MAP_ANONYMOUS | MAP_SHARED, "r-xs"},
	{PROT_WRITE | PROT_EXEC, MAP_ANONYMOUS | MAP_PRIVATE, "-wxp"},
	{PROT_WRITE | PROT_EXEC, MAP_ANONYMOUS | MAP_SHARED, "-wxs"},
	{PROT_READ | PROT_WRITE | PROT_EXEC, MAP_ANONYMOUS | MAP_PRIVATE, "rwxp"},
	{PROT_READ | PROT_WRITE | PROT_EXEC, MAP_ANONYMOUS | MAP_SHARED, "rwxs"},
};

static int read_mapping_perms(void *addr, char perms[8])
{
	FILE *fp = fopen("/proc/self/maps", "r");
	unsigned long start;
	char line[256];

	if (!fp)
		return -1;

	start = (unsigned long)addr;
	while (fgets(line, sizeof(line), fp)) {
		unsigned long lo, hi;
		char p[8];

		if (sscanf(line, "%lx-%lx %7s", &lo, &hi, p) == 3 &&
		    lo == start) {
			strcpy(perms, p);
			fclose(fp);
			return 0;
		}
	}
	fclose(fp);
	return -1;
}

int main(void)
{
	long page_size = page_size_or_die();
	int failed = 0;
	size_t i;

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++) {
		void *base;
		void *addr;
		char perms[8] = "";
		int guard_flags;

		guard_flags = (tests[i].flags & MAP_PRIVATE) ? MAP_SHARED : MAP_PRIVATE;
		base = mmap(NULL, page_size * 2, PROT_NONE,
			    MAP_ANONYMOUS | guard_flags, -1, 0);
		if (base == MAP_FAILED)
			fail_now("mmap guard");

		addr = mmap((char *)base + page_size, page_size, tests[i].prot,
			    tests[i].flags | MAP_FIXED, -1, 0);
		if (addr == MAP_FAILED)
			fail_now("mmap test page");

		if (read_mapping_perms(addr, perms) == 0 &&
		    strcmp(perms, tests[i].expected) == 0) {
			printf("PASS: mapping permissions matched %s\n", perms);
		} else {
			printf("FAIL: expected %s but found %s\n",
			       tests[i].expected, perms[0] ? perms : "<none>");
			failed = 1;
		}
		munmap(base, page_size * 2);
	}

	return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
