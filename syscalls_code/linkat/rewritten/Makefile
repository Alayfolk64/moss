CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -D_FILE_OFFSET_BITS=64

PROGRAMS = \
	linkat01_simple \
	linkat02_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
