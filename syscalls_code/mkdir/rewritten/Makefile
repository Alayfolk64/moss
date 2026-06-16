CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -std=c11

PROGRAMS = \
	mkdir02_simple \
	mkdir03_simple \
	mkdir04_simple \
	mkdir05_simple \
	mkdir09_simple

mkdir09_simple: LDLIBS += -pthread

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c
	$(CC) $(CFLAGS) -o $@ $< $(LDLIBS)

clean:
	rm -f $(PROGRAMS)
