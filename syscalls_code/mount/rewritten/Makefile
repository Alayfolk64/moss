CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -std=c11 -D_GNU_SOURCE

PROGRAMS = \
	mount01_simple \
	mount02_simple \
	mount03_simple \
	mount03_suid_child_simple \
	mount04_simple \
	mount05_simple \
	mount06_simple \
	mount07_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c mount_simple_common.h
	$(CC) $(CFLAGS) -o $@ $<

mount03_suid_child_simple: mount03_suid_child_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
