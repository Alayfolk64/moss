# Makefile for move_mount simplified tests

CC = gcc
CFLAGS = -Wall -Wextra -O2

TARGETS = move_mount01_simple move_mount02_simple

all: $(TARGETS)

move_mount01_simple: move_mount01_simple.c
	$(CC) $(CFLAGS) -o $@ $<

move_mount02_simple: move_mount02_simple.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f $(TARGETS)
	rm -rf mntpoint

test: all
	@echo "Running move_mount01_simple..."
	@sudo ./move_mount01_simple || true
	@echo ""
	@echo "Running move_mount02_simple..."
	@sudo ./move_mount02_simple || true

.PHONY: all clean test
