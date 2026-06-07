CC ?= cc
CFLAGS ?= -Wall -Wextra -O2

PROGRAMS = \
	kill02_simple \
	kill03_simple \
	kill05_simple \
	kill06_simple \
	kill07_simple \
	kill08_simple \
	kill09_simple \
	kill10_simple \
	kill11_simple \
	kill12_simple \
	kill13_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
