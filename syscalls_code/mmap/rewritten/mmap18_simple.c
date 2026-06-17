// 本测例验证 MAP_GROWSDOWN 栈映射：线程栈触碰 guard page 时应能向未映射区域增长；若增长区域提前被占用，子进程应被 SIGSEGV 杀死。

#include "mmap_simple_common.h"
#include <stdbool.h>

#ifdef __linux__
// 当前系统页大小。
static long page_size;

// 判断当前架构的栈是否向上增长；向上增长架构不能执行本测例。
static bool __attribute__((noinline)) check_stackgrow_up(void)
{
    // 第一次调用记录一个栈变量地址，第二次递归比较新变量地址。
    char local_var;
    static char *addr;

    // 递归一次后比较两个栈地址的方向。
    if (!addr) {
        addr = &local_var;
        return check_stackgrow_up();
    }

    return addr < &local_var;
}

// 按页大小向上对齐数值，用来替代原 LTP_ALIGN。
static size_t align_up(size_t value, size_t align)
{
    // 加上 align - 1 后清掉低位，得到不小于 value 的页对齐值。
    return (value + align - 1) / align * align;
}

// 分配可增长栈，并返回最低栈地址。
static void *allocate_stack(size_t stack_size, size_t mapped_size)
{
    // 预留 stack_guard_gap 默认 256 页，再加上完整栈空间。
    size_t reserved_size = (size_t)256 * (size_t)page_size + stack_size;
    char *start = mmap(NULL, reserved_size, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (start == MAP_FAILED) {
        fprintf(stderr, "mmap: %s\n", strerror(errno));
        _exit(EXIT_FAILURE);
    }

    // 释放预留空间，让后续 MAP_FIXED 只映射栈顶部的一部分。
    munmap(start, reserved_size);

    // 在栈顶部映射 mapped_size，并带上 MAP_GROWSDOWN。
    char *mapped = mmap(start + reserved_size - mapped_size, mapped_size,
                        PROT_READ | PROT_WRITE,
                        MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS | MAP_GROWSDOWN,
                        -1, 0);
    if (mapped == MAP_FAILED) {
        fprintf(stderr, "mmap: %s\n", strerror(errno));
        _exit(EXIT_FAILURE);
    }

    // 计算栈顶和栈底，并保留原测例的信息输出。
    char *stack_top = start + reserved_size;
    char *stack_bottom = start + reserved_size - stack_size;
    printf("start = %p, stack_top = %p, stack bottom = %p\n",
           start, stack_top, stack_bottom);
    printf("mapped pages %zu, stack pages %zu\n",
           mapped_size / (size_t)page_size, stack_size / (size_t)page_size);

    return stack_bottom;
}

// 递归消耗线程栈，直到栈帧地址低于限制地址。
static __attribute__((noinline)) void *check_depth_recursive(void *limit)
{
    // 栈向下增长时，局部变量地址低于 limit 表示已经触碰到目标区域。
    if ((char *)&limit < (char *)limit) {
        printf("&limit = %p, limit = %p\n", &limit, limit);
        return NULL;
    }

    // 继续递归，迫使线程栈向下增长。
    return check_depth_recursive(limit);
}

// 用给定栈地址启动线程，让线程递归触碰 guard page。
static void grow_stack(void *stack, size_t size)
{
    // 把 limit 放在栈底上一页，避免继续越过目标页。
    pthread_t test_thread;
    pthread_attr_t attr;
    void *limit = (char *)stack + page_size;

    // 初始化线程属性。
    int ret = pthread_attr_init(&attr);
    if (ret) {
        fprintf(stderr, "pthread_attr_init failed during setup\n");
        _exit(EXIT_FAILURE);
    }

    // 指定线程使用 MAP_GROWSDOWN 栈区域。
    ret = pthread_attr_setstack(&attr, stack, size);
    if (ret) {
        fprintf(stderr, "pthread_attr_setstack failed during setup\n");
        _exit(EXIT_FAILURE);
    }

    // 创建线程并等待递归结束。
    ret = pthread_create(&test_thread, &attr, check_depth_recursive, limit);
    if (ret) {
        errno = ret;
        fprintf(stderr, "pthread_create: %s\n", strerror(errno));
        _exit(EXIT_FAILURE);
    }
    pthread_join(test_thread, NULL);
    pthread_attr_destroy(&attr);

    // 子进程正常退出表示栈增长成功。
    exit(0);
}

// 期望栈能向未映射区域成功增长的场景。
static void grow_stack_success(size_t stack_size, size_t mapped_size)
{
    // 子进程中分配 growdown 栈并递归触碰。
    pid_t child_pid = fork();
    if (child_pid < 0)
        simple_die_errno("fork");
    if (!child_pid) {
        void *stack = allocate_stack(stack_size, mapped_size);
        grow_stack(stack, stack_size);
    }

    // 父进程检查子进程是否正常退出。
    int wstatus;
    if (waitpid(child_pid, &wstatus, 0) < 0)
        simple_die_errno("wait");
    if (WIFEXITED(wstatus) && WEXITSTATUS(wstatus) == 0) {
        printf("Stack grows in unmapped region\n");
    } else {
        char status_buf[128];
        fprintf(stderr, "Child: %s\n",
                simple_strstatus(wstatus, status_buf, sizeof(status_buf)));
    }
}

// 期望栈增长撞到已有映射并触发 SIGSEGV 的场景。
static void grow_stack_fail(size_t stack_size, size_t mapped_size)
{
    // 子进程中分配 growdown 栈，并在栈底额外映射一页制造冲突。
    pid_t child_pid = fork();
    if (child_pid < 0)
        simple_die_errno("fork");
    if (!child_pid) {
        void *stack = allocate_stack(stack_size, mapped_size);
        void *mapped = mmap(stack, (size_t)page_size, PROT_READ | PROT_WRITE,
                            MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (mapped == MAP_FAILED) {
            fprintf(stderr, "mmap: %s\n", strerror(errno));
            _exit(EXIT_FAILURE);
        }
        printf("mapped page at %p\n", stack);
        grow_stack(stack, stack_size);
    }

    // 父进程检查子进程是否被 SIGSEGV 杀死。
    int wstatus;
    if (waitpid(child_pid, &wstatus, 0) < 0)
        simple_die_errno("wait");
    if (WIFSIGNALED(wstatus) && WTERMSIG(wstatus) == SIGSEGV) {
        printf("Child killed by %s as expected\n", simple_strsig(SIGSEGV));
    } else {
        char status_buf[128];
        fprintf(stderr, "Child: %s\n",
                simple_strstatus(wstatus, status_buf, sizeof(status_buf)));
    }
}

// 执行原测例的四个 stack/mapped size 组合。
static void run_test(void)
{
    // 计算 pthread 最小栈的页对齐大小，并扩展出 8 倍栈空间。
    size_t pthread_stack = align_up((size_t)PTHREAD_STACK_MIN, (size_t)page_size);
    size_t stack_size = 8 * pthread_stack;

    // 两个成功增长场景和两个失败增长场景。
    grow_stack_success(stack_size, pthread_stack);
    grow_stack_success(stack_size, stack_size / 2);
    grow_stack_fail(stack_size, pthread_stack);
    grow_stack_fail(stack_size, stack_size / 2);
}
#endif

// 入口函数检查架构和 Linux 支持后运行 MAP_GROWSDOWN 测试。
int main(void)
{
#ifndef __linux__
    // MAP_GROWSDOWN 是 Linux 栈映射语义。
    fprintf(stderr, "SKIP: MAP_GROWSDOWN requires Linux\n");
    return EXIT_SKIP;
#else
    // 栈向上增长的架构不适合本测例。
    if (check_stackgrow_up()) {
        fprintf(stderr, "Test can't be performed with stack grows up architecture\n");
        return EXIT_SKIP;
    }

    // 初始化页大小并执行测试。
    page_size = simple_page_size();
    run_test();
    return EXIT_SUCCESS;
#endif
}
