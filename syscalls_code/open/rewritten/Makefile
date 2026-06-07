CC ?= cc
CFLAGS ?= -Wall -Wextra -O2 -D_FILE_OFFSET_BITS=64

PROGRAMS = \
	open01_simple \
	open01A_simple \
	open02_simple \
	open03_simple \
	open04_simple \
	open06_simple \
	open07_simple \
	open08_simple \
	open09_simple \
	open10_simple \
	open11_simple \
	open12_simple \
	open13_simple \
	open14_simple

.PHONY: all clean

all: $(PROGRAMS)

%_simple: %_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(PROGRAMS)
