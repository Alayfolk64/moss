CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -std=c11 -D_GNU_SOURCE

PROGRAMS = socket01_simple socket02_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
