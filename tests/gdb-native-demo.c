#include <stdio.h>

__attribute__((noinline)) static int native_marker(int value)
{
    volatile int marked_value = value + 1;
    return marked_value;
}

__attribute__((noinline)) static int demo_worker(int value)
{
    volatile int worker_value = value;
    return native_marker(worker_value);
}

__attribute__((noinline)) static int demo_entry(int value)
{
    volatile int entry_value = value;
    return demo_worker(entry_value);
}

int main(void)
{
    int value = demo_entry(41);
    printf("gdb-native-demo value=%d\n", value);
    return value == 42 ? 0 : 1;
}
