CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -std=c11 -D_GNU_SOURCE

PROGRAMS = \
	accept01_simple \
	accept02_simple \
	accept03_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
