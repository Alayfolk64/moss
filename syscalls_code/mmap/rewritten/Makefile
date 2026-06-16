CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -std=c11 -D_FILE_OFFSET_BITS=64 -pthread

PROGRAMS = \
	mmap001_simple \
	mmap01_simple \
	mmap02_simple \
	mmap03_simple \
	mmap04_simple \
	mmap05_simple \
	mmap06_simple \
	mmap08_simple \
	mmap09_simple \
	mmap10_simple \
	mmap11_simple \
	mmap12_simple \
	mmap13_simple \
	mmap14_simple \
	mmap15_simple \
	mmap16_simple \
	mmap17_simple \
	mmap18_simple \
	mmap19_simple \
	mmap20_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c mmap_simple_common.h
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
