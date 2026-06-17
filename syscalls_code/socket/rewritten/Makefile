CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -std=c11

PROGRAMS = \
	socket01_simple \
	socket02_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
