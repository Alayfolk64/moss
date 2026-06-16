CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -std=c11 -D_GNU_SOURCE -pthread

PROGRAMS = \
	bind01_simple \
	bind02_simple \
	bind03_simple \
	bind04_simple \
	bind05_simple \
	bind06_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c bind_simple_common.h
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
