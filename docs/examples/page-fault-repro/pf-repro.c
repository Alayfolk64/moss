#include <sys/mman.h>
#include <unistd.h>

#define TRACE_ADDRESS ((void *)0x3000000000UL)
#define TRACE_LENGTH 4096UL

/*
 * Keep the faulting store in a separate function so its instruction address
 * is unambiguous in objdump and in the kernel's page-fault log.
 */
__attribute__((noinline))
static void first_store(volatile unsigned char *page)
{
    page[0] = 0x5a;
}

static void fail(const char *message, unsigned long length)
{
    write(STDERR_FILENO, message, length);
    _exit(1);
}

/*
 * This sample intentionally supplies its own ELF entry point.  That lets the
 * repository's RISC-V compiler link directly against the glibc runtime in the
 * test image without requiring a separate glibc cross-compiler sysroot.
 */
void _start(void)
{
    static const char before_mmap[] =
        "[USER] calling glibc mmap: private anonymous, populate=false\n";
    static const char mmap_failed[] = "[USER] mmap failed\n";
    static const char wrong_address[] = "[USER] mmap returned the wrong address\n";
    static const char before_store[] =
        "[USER] mmap returned 0x3000000000; executing first store now\n";
    static const char after_store[] =
        "[USER] first store returned; byte=0x5a\n";

    write(STDOUT_FILENO, before_mmap, sizeof(before_mmap) - 1);
    volatile unsigned char *page = mmap(
        TRACE_ADDRESS,
        TRACE_LENGTH,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE,
        -1,
        0);
    if (page == MAP_FAILED)
        fail(mmap_failed, sizeof(mmap_failed) - 1);
    if (page != TRACE_ADDRESS)
        fail(wrong_address, sizeof(wrong_address) - 1);

    write(STDOUT_FILENO, before_store, sizeof(before_store) - 1);
    first_store(page);
    if (page[0] != 0x5a)
        fail(mmap_failed, sizeof(mmap_failed) - 1);
    write(STDOUT_FILENO, after_store, sizeof(after_store) - 1);

    munmap((void *)page, TRACE_LENGTH);
    _exit(0);
}
