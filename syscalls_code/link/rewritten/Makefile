CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -D_FILE_OFFSET_BITS=64

PROGRAMS = \
	link02_simple \
	link04_simple \
	link05_simple \
	link08_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
