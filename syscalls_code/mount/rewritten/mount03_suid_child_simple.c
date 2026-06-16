// 本辅助程序用于 mount03_simple 验证 MS_NOSUID：以 setuid-root 文件方式执行后，setreuid(getuid(), 0) 应因为 nosuid 被忽略特权而返回 EPERM。

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

// 入口函数只执行原辅助程序中的 setreuid 检查。
int main(void)
{
    // 尝试把有效 UID 设为 0，MS_NOSUID 生效时应失败并返回 EPERM。
    errno = 0;
    if (setreuid(getuid(), 0) == -1 && errno == EPERM) {
        printf("setreuid(getuid(), 0) failed as expected\n");
        return 0;
    }

    // 成功或 errno 不匹配都表示 nosuid 行为不符合预期。
    fprintf(stderr, "setreuid(getuid(), 0) failed unexpectedly: %s\n", strerror(errno));
    return 1;
}
