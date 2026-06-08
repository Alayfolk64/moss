# move_mount 测试用例改写总结

## 改写完成情况

✅ 已完成 move_mount 系统调用测试用例的简化改写

### 创建的文件

1. **syscalls_code/move_mount/rewritten/move_mount01_simple.c**
   - 基本功能测试
   - 测试 6 种标志组合
   - 201 行代码

2. **syscalls_code/move_mount/rewritten/move_mount02_simple.c**
   - 错误处理测试
   - 测试 5 种错误情况
   - 226 行代码

3. **syscalls_code/move_mount/rewritten/Makefile**
   - 编译脚本
   - 包含 clean 和 test 目标

4. **syscalls_code/move_mount/rewritten/README.md**
   - 完整的使用文档
   - 包含 move_mount API 说明
   - 包含预期输出和故障排查

## 改写原则

参考 `syscalls_code/open/rewritten/` 下的改写模式：

### 1. 移除 LTP 框架依赖

**原始代码：**
```c
#include "tst_test.h"
#include "lapi/fsmount.h"

static void run(unsigned int n) {
    // LTP 框架代码
}

static struct tst_test test = {
    .tcnt = ARRAY_SIZE(tcases),
    .test = run,
    // ...
};
```

**改写后：**
```c
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
// 标准 C 库

int main(void) {
    // 独立的测试逻辑
    return failed ? EXIT_FAILURE : EXIT_SUCCESS;
}
```

### 2. 使用直接系统调用

**原始代码依赖 LTP 封装：**
- `TEST(move_mount(...))`
- `TST_RET`、`TST_ERR`
- `SAFE_CLOSE()`、`SAFE_UMOUNT()`

**改写后直接调用：**
```c
static int move_mount(int from_dirfd, const char *from_pathname,
                      int to_dirfd, const char *to_pathname,
                      unsigned int flags)
{
    return syscall(__NR_move_mount, from_dirfd, from_pathname,
                   to_dirfd, to_pathname, flags);
}
```

### 3. 简化设备和挂载点处理

**原始代码：**
```c
.format_device = 1,
.mntpoint = MNTPOINT,
.all_filesystems = 1,

TEST(fd = fsopen(tst_device->fs_type, 0));
TEST(fsconfig(fd, FSCONFIG_SET_STRING, "source", tst_device->dev, 0));
```

**改写后：**
```c
// 直接使用 tmpfs，无需设备
fd = fsopen("tmpfs", 0);
fsconfig(fd, FSCONFIG_CMD_CREATE, NULL, NULL, 0);
```

### 4. 简化结果报告

**原始代码：**
```c
tst_res(TPASS | TTERRNO, "move_mount() passed");
tst_res(TFAIL | TTERRNO, "move_mount() failed");
```

**改写后：**
```c
printf("PASS: move_mount() succeeded\n");
printf("FAIL: move_mount() failed: %s\n", strerror(errno));
```

## 测试覆盖对比

### move_mount01 - 基本功能测试

| 测试项 | 原始 LTP | 改写版 | 状态 |
|--------|---------|--------|------|
| MOVE_MOUNT_F_SYMLINKS | ✅ | ✅ | 保留 |
| MOVE_MOUNT_F_AUTOMOUNTS | ✅ | ✅ | 保留 |
| MOVE_MOUNT_F_EMPTY_PATH | ✅ | ✅ | 保留 |
| MOVE_MOUNT_T_SYMLINKS | ✅ | ✅ | 保留 |
| MOVE_MOUNT_T_AUTOMOUNTS | ✅ | ✅ | 保留 |
| MOVE_MOUNT_T_EMPTY_PATH | ✅ | ✅ | 保留 |

### move_mount02 - 错误处理测试

| 测试项 | 预期错误 | 原始 LTP | 改写版 | 状态 |
|--------|----------|---------|--------|------|
| invalid-from-fd | EBADF | ✅ | ✅ | 保留 |
| invalid-from-path | ENOENT/EINVAL | ✅ | ✅ | 保留 |
| invalid-to-fd | EBADF | ✅ | ✅ | 保留 |
| invalid-to-path | ENOENT | ✅ | ✅ | 保留 |
| invalid-flags | EINVAL | ✅ | ✅ | 保留 |

## 关键改进

### 1. 自包含性

- **无外部依赖**：只需标准 C 库和 Linux 系统调用
- **独立编译**：可以单独编译运行，不需要 LTP 环境
- **便于调试**：可以直接用 gdb 调试

### 2. 可读性

- **清晰的注释**：每个测试都有中文说明
- **简单的结构**：main() 函数直接包含测试逻辑
- **明确的输出**：PASS/FAIL 输出清晰

### 3. 可移植性

- **系统调用号定义**：包含 fallback 定义
- **错误处理**：优雅处理不支持的内核
- **跨平台**：适配不同的 Linux 发行版

## move_mount 系统调用说明

### 新挂载 API（Linux 5.2+）

move_mount() 是 Linux 新挂载 API 的一部分，配合使用：

```
fsopen()  → 创建文件系统上下文
    ↓
fsconfig() → 配置参数
    ↓
fsmount() → 创建挂载 fd
    ↓
move_mount() → 移动到目标位置
```

### 与传统 mount() 的区别

**传统方式：**
```c
mount("tmpfs", "/mnt", "tmpfs", 0, "size=1G");
```

**新 API 方式：**
```c
int fd = fsopen("tmpfs", 0);
fsconfig(fd, FSCONFIG_SET_STRING, "size", "1G", 0);
fsconfig(fd, FSCONFIG_CMD_CREATE, NULL, NULL, 0);
int mfd = fsmount(fd, 0, 0);
move_mount(mfd, "", AT_FDCWD, "/mnt", MOVE_MOUNT_F_EMPTY_PATH);
```

**优势：**
- 基于文件描述符，更安全
- 支持异步配置
- 更好的错误处理
- 支持复杂的挂载拓扑

## 使用说明

### 编译

```bash
cd syscalls_code/move_mount/rewritten
make
```

### 运行（需要 root）

```bash
# 运行所有测试
sudo make test

# 单独运行
sudo ./move_mount01_simple
sudo ./move_mount02_simple
```

### 预期输出

成功时所有测试都应该 PASS：

```
Testing move_mount() with various flags...

PASS: MOVE_MOUNT_F_SYMLINKS - move_mount() succeeded
PASS: MOVE_MOUNT_F_AUTOMOUNTS - move_mount() succeeded
PASS: MOVE_MOUNT_F_EMPTY_PATH - move_mount() succeeded
PASS: MOVE_MOUNT_T_SYMLINKS - move_mount() succeeded
PASS: MOVE_MOUNT_T_AUTOMOUNTS - move_mount() succeeded
PASS: MOVE_MOUNT_T_EMPTY_PATH - move_mount() succeeded
```

## 与 StarryOS 集成

这些改写后的测试可以用于：

1. **验证 StarryOS 的 move_mount 实现**
   - 直接在 StarryOS 环境中编译运行
   - 对比 Linux 的行为

2. **作为集成测试**
   - 添加到 CI/CD 流程
   - 自动化测试新的实现

3. **调试参考**
   - 理解标准 Linux 行为
   - 对比实现差异

## 参考资料

- [move_mount(2) man page](https://man7.org/linux/man-pages/man2/move_mount.2.html)
- [Linux mount API](https://www.kernel.org/doc/html/latest/filesystems/mount_api.html)
- [LTP 原始测试](https://github.com/linux-test-project/ltp/tree/master/testcases/kernel/syscalls/move_mount)
- [open 改写示例](../open/rewritten/)
