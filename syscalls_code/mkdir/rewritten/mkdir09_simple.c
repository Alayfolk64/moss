// 本测例验证多个线程在同一目录下反复执行 mkdir/rmdir 时，已存在目录返回 EEXIST、不存在目录返回 ENOENT，创建后删除的目录操作能持续成功；这是原 LTP 并发 mkdir 压力测例的简化版。

// 启用 POSIX/GNU 接口声明，保证 mkdtemp 等函数可见。
#define _GNU_SOURCE

#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

// 所有并发操作发生在这个测试目录下。
#define MNTPOINT "mntpoint"

// mkdir 使用的权限位，保留原测例中的 rwx 权限形式。
#define MODE_RWX 07770

// 预先存在的目录名称格式，用于触发 EEXIST。
#define DIR_NAME MNTPOINT "/X.%d"

// 每个线程组独立创建和删除的目录名称格式。
#define DIR_NAME_GROUP MNTPOINT "/X.%d.%d"

// 每组包含三类线程：创建已存在目录、删除不存在目录、创建后删除目录。
#define NCHILD 3

// 并发线程组数量，原测例默认是 2 组。
static int child_groups = 2;

// 测试运行秒数，原测例默认是 1 秒。
static int test_time = 1;

// 每组目录编号上限，原测例默认是 10。
static int nfiles = 10;

// 用于创建临时工作目录的模板，mkdtemp 会替换末尾的 X。
static char temp_dir[] = "/tmp/mkdir09_XXXXXX";

// 通知所有线程停止循环的原子标志。
static atomic_int done;

// 记录线程中观察到的失败次数。
static atomic_int failures;

// 删除测试过程中可能留下的目录和临时目录。
static void cleanup(void)
{
    // 删除每个线程组可能创建后尚未删除的目录。
    for (int group = 0; group < child_groups; group++) {
        for (int j = 2; j < nfiles; j += NCHILD) {
            char path[PATH_MAX];
            snprintf(path, sizeof(path), DIR_NAME_GROUP, j, group);
            rmdir(path);
        }
    }

    // 删除预先创建的 EEXIST 目录。
    for (int j = 0; j < nfiles; j += NCHILD) {
        char path[PATH_MAX];
        snprintf(path, sizeof(path), DIR_NAME, j);
        rmdir(path);
    }

    // 删除并发操作的父目录。
    rmdir(MNTPOINT);

    // 离开临时目录后删除临时目录本身。
    if (temp_dir[0] != '\0') {
        chdir("/");
        rmdir(temp_dir);
    }
}

// 记录一次线程失败并通知其他线程尽快结束。
static void record_failure(const char *operation, const char *path, int expected_errno, int actual_errno)
{
    // 打印失败细节，说明哪个操作和路径没有得到预期 errno。
    fprintf(stderr, "FAIL: %s(%s) errno=%d (%s)，期望 errno=%d (%s)\n",
            operation, path, actual_errno, strerror(actual_errno),
            expected_errno, strerror(expected_errno));

    // 增加失败计数并设置停止标志。
    atomic_fetch_add(&failures, 1);
    atomic_store(&done, 1);
}

// 构造一个目录路径，并检查 snprintf 是否发生截断。
static int make_path(char *buf, size_t buflen, const char *fmt, int value, int group)
{
    // 根据是否需要线程组编号选择不同格式。
    int len;
    if (group >= 0)
        len = snprintf(buf, buflen, fmt, value, group);
    else
        len = snprintf(buf, buflen, fmt, value);

    // 路径被截断时返回失败，避免后续操作使用不完整路径。
    if (len < 0 || (size_t)len >= buflen) {
        fprintf(stderr, "FAIL: 路径缓冲区过小\n");
        atomic_fetch_add(&failures, 1);
        atomic_store(&done, 1);
        return -1;
    }

    return 0;
}

// 线程类型 1：反复对已经存在的目录调用 mkdir，预期返回 EEXIST。
static void test_existing_dirs(int child_num)
{
    // 在停止标志设置前不断覆盖所有预创建目录。
    while (!atomic_load(&done)) {
        for (int j = 0; j < nfiles && !atomic_load(&done); j += NCHILD) {
            char path[PATH_MAX];
            if (make_path(path, sizeof(path), DIR_NAME, j, -1) != 0)
                return;

            // mkdir 已存在目录必须失败，并把 errno 设为 EEXIST。
            errno = 0;
            if (mkdir(path, MODE_RWX) != -1 || errno != EEXIST) {
                record_failure("mkdir", path, EEXIST, errno);
                return;
            }
        }
    }

    // 当前线程类型在整个运行期间没有观察到错误。
    printf("PASS: [%d] 已存在目录 mkdir 持续返回 EEXIST\n", child_num);
}

// 线程类型 2：反复删除不存在的目录，预期返回 ENOENT。
static void test_missing_dirs(int child_num)
{
    // 在停止标志设置前不断覆盖不存在目录编号。
    while (!atomic_load(&done)) {
        for (int j = 1; j < nfiles && !atomic_load(&done); j += NCHILD) {
            char path[PATH_MAX];
            if (make_path(path, sizeof(path), DIR_NAME, j, -1) != 0)
                return;

            // rmdir 不存在目录必须失败，并把 errno 设为 ENOENT。
            errno = 0;
            if (rmdir(path) != -1 || errno != ENOENT) {
                record_failure("rmdir", path, ENOENT, errno);
                return;
            }
        }
    }

    // 当前线程类型在整个运行期间没有观察到错误。
    printf("PASS: [%d] 不存在目录 rmdir 持续返回 ENOENT\n", child_num);
}

// 线程类型 3：反复创建并删除本线程组独占的目录，预期两个操作都成功。
static void test_create_remove_dirs(int child_num)
{
    // 用 child_num 除以线程类型数，得到不和另一组冲突的组号。
    int group = child_num / NCHILD;

    // 在停止标志设置前不断创建一批独占目录。
    while (!atomic_load(&done)) {
        for (int j = 2; j < nfiles && !atomic_load(&done); j += NCHILD) {
            char path[PATH_MAX];
            if (make_path(path, sizeof(path), DIR_NAME_GROUP, j, group) != 0)
                return;

            // 创建独占目录应当成功。
            if (mkdir(path, MODE_RWX) != 0) {
                record_failure("mkdir", path, 0, errno);
                return;
            }
        }

        // 删除刚才创建的独占目录，为下一轮循环恢复初始状态。
        for (int j = 2; j < nfiles && !atomic_load(&done); j += NCHILD) {
            char path[PATH_MAX];
            if (make_path(path, sizeof(path), DIR_NAME_GROUP, j, group) != 0)
                return;

            // 删除独占目录应当成功。
            if (rmdir(path) != 0) {
                record_failure("rmdir", path, 0, errno);
                return;
            }
        }
    }

    // 当前线程类型在整个运行期间没有观察到错误。
    printf("PASS: [%d] 独占目录 mkdir/rmdir 持续成功\n", child_num);
}

// pthread 入口函数，根据线程编号选择三类并发操作之一。
static void *child_thread_func(void *arg)
{
    // 把传入的编号转换为整数，并按编号模 3 选择测试类型。
    int child_num = (int)(long)arg;
    switch (child_num % NCHILD) {
    case 0:
        test_existing_dirs(child_num);
        break;
    case 1:
        test_missing_dirs(child_num);
        break;
    default:
        test_create_remove_dirs(child_num);
        break;
    }

    return NULL;
}

// 创建并发测试需要的父目录和预先存在的目录。
static int setup(void)
{
    // 创建并进入独立临时目录，避免并发操作污染当前目录。
    if (mkdtemp(temp_dir) == NULL) {
        fprintf(stderr, "FAIL: mkdtemp 失败: %s\n", strerror(errno));
        return -1;
    }
    if (chdir(temp_dir) != 0) {
        fprintf(stderr, "FAIL: chdir(%s) 失败: %s\n", temp_dir, strerror(errno));
        return -1;
    }

    // 创建并发操作的父目录。
    if (mkdir(MNTPOINT, 0777) != 0) {
        fprintf(stderr, "FAIL: mkdir(%s) 失败: %s\n", MNTPOINT, strerror(errno));
        return -1;
    }

    // 预先创建编号为 0、3、6、9 的目录，供 EEXIST 线程反复测试。
    for (int j = 0; j < nfiles; j += NCHILD) {
        char path[PATH_MAX];
        if (make_path(path, sizeof(path), DIR_NAME, j, -1) != 0)
            return -1;
        if (mkdir(path, MODE_RWX) != 0) {
            fprintf(stderr, "FAIL: 预创建目录 %s 失败: %s\n", path, strerror(errno));
            return -1;
        }
    }

    return 0;
}

// 入口函数启动所有线程，运行指定时间后停止并汇总结果。
int main(void)
{
    // 准备测试目录结构。
    if (setup() != 0) {
        cleanup();
        return 1;
    }

    // 初始化线程间共享状态。
    atomic_store(&done, 0);
    atomic_store(&failures, 0);

    // 创建 child_groups 组线程，每组包含三种测试行为。
    int thread_count = child_groups * NCHILD;
    pthread_t threads[thread_count];
    for (long i = 0; i < thread_count; i++) {
        int rc = pthread_create(&threads[i], NULL, child_thread_func, (void *)i);
        if (rc != 0) {
            fprintf(stderr, "FAIL: pthread_create 失败: %s\n", strerror(rc));
            atomic_store(&done, 1);
            thread_count = (int)i;
            break;
        }
    }

    // 让并发操作持续运行一段时间，然后通知线程结束。
    sleep((unsigned int)test_time);
    atomic_store(&done, 1);

    // 等待已经创建的线程全部退出。
    for (int i = 0; i < thread_count; i++) {
        int rc = pthread_join(threads[i], NULL);
        if (rc != 0) {
            fprintf(stderr, "FAIL: pthread_join 失败: %s\n", strerror(rc));
            atomic_fetch_add(&failures, 1);
        }
    }

    // 根据线程失败计数决定测试结果。
    int failed = atomic_load(&failures);
    if (failed == 0)
        printf("PASS: mkdir/rmdir 并发压力场景未发现异常\n");

    // 清理临时目录和测试文件。
    cleanup();
    return failed == 0 ? 0 : 1;
}
