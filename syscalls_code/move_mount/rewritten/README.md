# move_mount 系统调用测试 - 简化版

这是 LTP move_mount 测试用例的简化改写版本，移除了 LTP 框架依赖，使用标准 C 库实现。

## 文件说明

- `move_mount01_simple.c` - move_mount() 基本功能测试
- `move_mount02_simple.c` - move_mount() 错误处理测试
- `Makefile` - 编译脚本

## move_mount 系统调用说明

### 函数原型

```c
int move_mount(int from_dirfd, const char *from_pathname,
               int to_dirfd, const char *to_pathname,
               unsigned int flags);
```

### 参数说明

- `from_dirfd`: 源文件描述符（通常是 fsmount 返回的 fd）
- `from_pathname`: 源路径（配合 MOVE_MOUNT_F_EMPTY_PATH 时应为空字符串）
- `to_dirfd`: 目标目录文件描述符（可以是 AT_FDCWD）
- `to_pathname`: 目标路径
- `flags`: 标志位组合

### 标志说明

**源路径标志（from）：**
- `MOVE_MOUNT_F_SYMLINKS` (0x01) - 跟随源路径中的符号链接
- `MOVE_MOUNT_F_AUTOMOUNTS` (0x02) - 跟随源路径中的自动挂载点
- `MOVE_MOUNT_F_EMPTY_PATH` (0x04) - from_pathname 必须为空，使用 from_dirfd

**目标路径标志（to）：**
- `MOVE_MOUNT_T_SYMLINKS` (0x10) - 跟随目标路径中的符号链接
- `MOVE_MOUNT_T_AUTOMOUNTS` (0x20) - 跟随目标路径中的自动挂载点
- `MOVE_MOUNT_T_EMPTY_PATH` (0x40) - to_pathname 必须为空，使用 to_dirfd

### 相关系统调用

move_mount() 通常配合以下新挂载 API 使用：

1. **fsopen()** - 创建文件系统上下文
   ```c
   int fsopen(const char *fsname, unsigned int flags);
   ```

2. **fsconfig()** - 配置文件系统参数
   ```c
   int fsconfig(int fd, unsigned int cmd, const char *key,
                const void *value, int aux);
   ```

3. **fsmount()** - 创建挂载文件描述符
   ```c
   int fsmount(int fd, unsigned int flags, unsigned int ms_flags);
   ```

### 典型使用流程

```c
// 1. 创建文件系统上下文
int fd = fsopen("tmpfs", 0);

// 2. 配置文件系统（可选）
fsconfig(fd, FSCONFIG_SET_STRING, "source", "tmpfs", 0);
fsconfig(fd, FSCONFIG_CMD_CREATE, NULL, NULL, 0);

// 3. 创建挂载文件描述符
int mfd = fsmount(fd, 0, 0);
close(fd);

// 4. 移动挂载到目标位置
move_mount(mfd, "", AT_FDCWD, "/mnt/point",
           MOVE_MOUNT_F_EMPTY_PATH);
close(mfd);
```

## 测试说明

### move_mount01_simple.c

测试 move_mount() 的基本功能，验证各种标志组合：

1. MOVE_MOUNT_F_SYMLINKS - 源路径符号链接处理
2. MOVE_MOUNT_F_AUTOMOUNTS - 源路径自动挂载处理
3. MOVE_MOUNT_F_EMPTY_PATH - 使用空路径 + fd
4. MOVE_MOUNT_T_SYMLINKS - 目标路径符号链接处理
5. MOVE_MOUNT_T_AUTOMOUNTS - 目标路径自动挂载处理
6. MOVE_MOUNT_T_EMPTY_PATH - 目标使用空路径 + fd

### move_mount02_simple.c

测试 move_mount() 的错误处理，验证各种非法参数：

1. **EBADF** - 无效的 from_dirfd
2. **EINVAL** - from_pathname 非空但设置了 MOVE_MOUNT_F_EMPTY_PATH
3. **EBADF** - 无效的 to_dirfd
4. **ENOENT** - 目标路径不存在
5. **EINVAL** - 无效的标志组合

## 编译和运行

### 编译

```bash
cd syscalls_code/move_mount/rewritten
make
```

### 运行测试（需要 root 权限）

```bash
# 运行所有测试
sudo make test

# 或单独运行
sudo ./move_mount01_simple
sudo ./move_mount02_simple
```

### 清理

```bash
make clean
```

## 与原始 LTP 测试的区别

### 简化内容

1. **移除 LTP 框架**
   - 不再依赖 `tst_test.h`、`tst_safe_macros.h` 等
   - 使用标准 C 库和直接系统调用

2. **简化设备处理**
   - 原始测试使用 `tst_device->fs_type` 和 `.format_device`
   - 简化版直接使用 tmpfs，无需格式化设备

3. **简化挂载点检查**
   - 原始使用 `tst_is_mounted_at_tmpdir()`
   - 简化版直接读取 `/proc/mounts`

### 保留内容

1. **核心测试逻辑完全保留**
   - 所有标志组合测试
   - 所有错误码验证

2. **测试覆盖率相同**
   - move_mount01: 6 个标志测试
   - move_mount02: 5 个错误情况

## 系统调用号

这些测试使用的系统调用号（x86_64）：

```c
#define __NR_move_mount  429
#define __NR_fsopen      430
#define __NR_fsconfig    431
#define __NR_fsmount     432
```

不同架构的系统调用号可能不同，请参考：
- `/usr/include/asm/unistd_64.h` (x86_64)
- `/usr/include/asm-generic/unistd.h` (通用)

## 预期输出

### move_mount01_simple 成功输出

```
Testing move_mount() with various flags...

PASS: MOVE_MOUNT_F_SYMLINKS - move_mount() succeeded
PASS: MOVE_MOUNT_F_AUTOMOUNTS - move_mount() succeeded
PASS: MOVE_MOUNT_F_EMPTY_PATH - move_mount() succeeded
PASS: MOVE_MOUNT_T_SYMLINKS - move_mount() succeeded
PASS: MOVE_MOUNT_T_AUTOMOUNTS - move_mount() succeeded
PASS: MOVE_MOUNT_T_EMPTY_PATH - move_mount() succeeded
```

### move_mount02_simple 成功输出

```
Testing move_mount() error handling...

PASS: invalid-from-fd - move_mount() failed with Bad file descriptor as expected
PASS: invalid-from-path - move_mount() failed with Invalid argument as expected
PASS: invalid-to-fd - move_mount() failed with Bad file descriptor as expected
PASS: invalid-to-path - move_mount() failed with No such file or directory as expected
PASS: invalid-flags - move_mount() failed with Invalid argument as expected
```

## 故障排查

### 内核版本要求

move_mount() 等新挂载 API 是在 Linux 5.2 引入的。如果测试跳过并显示：

```
SKIP: fsopen() not supported: Function not implemented
```

说明内核版本太低，需要升级到 5.2 或更高版本。

### 权限问题

如果显示权限错误：

```
FAIL: mkdir mntpoint: Operation not permitted
```

请使用 sudo 运行测试。

## 参考资料

- [move_mount(2) - Linux manual page](https://man7.org/linux/man-pages/man2/move_mount.2.html)
- [fsopen(2) - Linux manual page](https://man7.org/linux/man-pages/man2/fsopen.2.html)
- [LTP - Linux Test Project](https://github.com/linux-test-project/ltp)
- [Linux mount API documentation](https://www.kernel.org/doc/html/latest/filesystems/mount_api.html)
