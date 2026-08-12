CC = gcc
CFLAGS = -O2 -Wall -Wextra

TARGET = calc
OBJS = main.o add.o mul.o

.PHONY: all clean
.DELETE_ON_ERROR:

all: $(TARGET)

$(TARGET): $(OBJS)
	@printf '%s\n' '[LD] calc'
	$(CC) $(LDFLAGS) -o $@ $(OBJS) $(LDLIBS)

main.o: main.c add.h mul.h
add.o: add.c add.h
mul.o: mul.c mul.h

%.o: %.c
	@printf '%s\n' '[CC] $<'
	$(CC) $(CPPFLAGS) $(CFLAGS) -c -o $@ $<

clean:
	@printf '%s\n' '[CLEAN] build products'
	rm -f $(TARGET) $(OBJS)
