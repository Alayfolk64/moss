CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -std=c11 -D_GNU_SOURCE

PROGRAMS = \
	umount01_simple \
	umount02_simple \
	umount03_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c umount_simple_common.h
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
