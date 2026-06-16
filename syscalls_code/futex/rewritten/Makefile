CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -std=c11

PROGRAMS = \
	futex_cmp_requeue01_simple \
	futex_cmp_requeue02_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
