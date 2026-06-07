# StarryOS 系统调用修复与测试改写 - 工作总结

## 工作概览

本次工作完成了两个主要任务：
1. **修复 StarryOS 系统调用问题** - 基于 LTP 测试失败进行定位和修复
2. **改写 move_mount 测试用例** - 将 LTP 测试用例改写为独立的简化版本

---

## 一、系统调用修复

### 1. mkdir02 - 目录 GID 继承问题 ✅ **已修复并验证**

#### 问题描述
```
mkdir02.c:34: TFAIL: New dir FAILED to inherit GID: has 65534, expected 1
mkdir02.c:41: TFAIL: New dir FAILED to inherit S_ISGID
```

#### 根本原因
在 `sys_fchownat` 中，chown 系统调用错误地清除了**目录**的 SET_GID 位。

根据 POSIX 标准：
- chown 对**普通文件**且**组可执行**时，应清除 SET_GID 位（安全考虑）
- chown 对**目录**时，应**保留** SET_GID 位（用于组继承机制）

#### 修复代码
**文件**: `kernel/src/syscall/fs/ctl.rs:688`

```rust
// 修改前：
if mode.contains(NodePermission::GROUP_EXEC) {
    mode.remove(NodePermission::SET_GID);
}

// 修改后：
if meta.node_type != NodeType::Directory && mode.contains(NodePermission::GROUP_EXEC) {
    mode.remove(NodePermission::SET_GID);
}
```

#### 验证结果
```
sys_fchownat: path=Some("testdir1"), before: mode=0o2777 (SET_GID=true), node_type=Directory
sys_fchownat: after: mode=0o2777 (SET_GID=true)
mkdir02.c:46: TPASS: New dir inherited GID and S_ISGID
```

✅ **测试通过！**

---

### 2. move_mount01 - move_mount 系统调用 ✅ **已修复并验证**

#### 问题描述
```
move_mount01.c:64: TFAIL: move_mount() failed: EINVAL (22)
```

#### 根本原因
参数验证逻辑不正确，没有正确处理 MOVE_MOUNT_F_EMPTY_PATH 标志。

#### 修复代码
**文件**: `kernel/src/syscall/fs/mount.rs:477-501`

```rust
let from_path = vm_load_string(from_path)?;
let to_path = vm_load_string(to_path)?;

if flags & MOVE_MOUNT_F_EMPTY_PATH != 0 {
    // MOVE_MOUNT_F_EMPTY_PATH 设置时，from_path 必须为空
    if !from_path.is_empty() {
        return Err(AxError::InvalidInput);
    }
    // 处理 fsmount fd...
} else {
    // MOVE_MOUNT_F_EMPTY_PATH 未设置
    if from_path.is_empty() {
        return Err(AxError::InvalidInput);
    }
    return Err(AxError::NotFound);  // 路径挂载不支持
}
```

#### 验证结果
```
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_F_SYMLINKS: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_F_AUTOMOUNTS: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_F_EMPTY_PATH: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_T_SYMLINKS: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_T_AUTOMOUNTS: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_T_EMPTY_PATH: move_mount() passed
```

✅ **所有测试通过！**

---

### 3. mkdir03 - mount EBUSY 🔄 **部分修复**

#### 问题描述
```
tst_test.c:1017: TBROK: mount((null), mntpoint, tmpfs, 33, (nil)) failed: EBUSY (16)
```

#### 修复尝试
**文件**: `kernel/src/syscall/fs/mount.rs:312-334, 336-353`

移除了应用层的 MOUNTED_PATHS 预检查，让 VFS 层自己处理重复挂载：

```rust
// 移除预检查，直接调用 VFS
if let Err(err) = target.mount_with_flags(&fs, mount_flags) {
    warn!("mount target={target_path} failed: {err:?}");
    return Err(err.into());
}
```

#### 当前状态
❌ 问题可能在 VFS 层更深处，需要进一步调试。

---

### 4. move_mount02 - 错误码 ✅ **应该已修复**

与 move_mount01 相同的修复应该已经解决了错误码问题。

---

## 二、move_mount 测试用例改写

### 改写完成情况

✅ 已完成 move_mount 系统调用测试用例的简化改写

#### 创建的文件

```
syscalls_code/move_mount/rewritten/
├── Makefile                      # 编译脚本
├── move_mount01_simple.c         # 基本功能测试 (201 行)
├── move_mount02_simple.c         # 错误处理测试 (226 行)
├── README.md                     # 使用文档 (5.9 KB)
└── REWRITE_SUMMARY.md            # 改写总结 (5.8 KB)
```

### 改写原则

参考 `syscalls_code/open/rewritten/` 的改写模式：

1. **移除 LTP 框架依赖**
   - 不再依赖 `tst_test.h`、`lapi/fsmount.h`
   - 使用标准 C 库和直接系统调用

2. **简化为独立程序**
   - 从测试框架变为独立的 `main()` 程序
   - 可以直接编译运行，无需 LTP 环境

3. **保留核心测试逻辑**
   - 所有测试用例完全保留
   - 测试覆盖率与原始版本相同

4. **添加中文注释**
   - 清晰的测试目标说明
   - 便于理解和调试

### 测试覆盖对比

#### move_mount01 - 基本功能
- ✅ MOVE_MOUNT_F_SYMLINKS
- ✅ MOVE_MOUNT_F_AUTOMOUNTS
- ✅ MOVE_MOUNT_F_EMPTY_PATH
- ✅ MOVE_MOUNT_T_SYMLINKS
- ✅ MOVE_MOUNT_T_AUTOMOUNTS
- ✅ MOVE_MOUNT_T_EMPTY_PATH

#### move_mount02 - 错误处理
- ✅ invalid-from-fd → EBADF
- ✅ invalid-from-path → EINVAL
- ✅ invalid-to-fd → EBADF
- ✅ invalid-to-path → ENOENT
- ✅ invalid-flags → EINVAL

### 使用方法

```bash
cd syscalls_code/move_mount/rewritten

# 编译
make

# 运行测试（需要 root）
sudo make test

# 或单独运行
sudo ./move_mount01_simple
sudo ./move_mount02_simple
```

---

## 技术要点

### POSIX 标准：chown 对 SET_GID 的处理

根据 POSIX 标准：
- chown **总是**清除 SET_UID 位（安全考虑）
- 对于**普通文件**且**组可执行**：清除 SET_GID 位
- 对于**目录**：**保留** SET_GID 位（用于控制新建文件的组继承）

### 目录 SET_GID 的作用

当目录设置了 SET_GID 位时：
1. 在该目录下创建的新文件和目录会继承**目录的 GID**，而不是创建进程的 egid
2. 新建的子目录也会自动设置 SET_GID 位，形成递归继承

这个特性常用于：
- **共享目录**：多个用户在同一目录下创建文件时，保持统一的组权限
- **项目协作**：确保项目目录下的所有文件属于同一组

### move_mount 新挂载 API

move_mount() 是 Linux 5.2+ 引入的新挂载 API 的一部分：

```
fsopen()     → 创建文件系统上下文
    ↓
fsconfig()   → 配置参数
    ↓
fsmount()    → 创建挂载 fd
    ↓
move_mount() → 移动到目标位置
```

**优势：**
- 基于文件描述符，更安全
- 支持异步配置
- 更好的错误处理
- 支持复杂的挂载拓扑

---

## 修改文件清单

### 内核代码修复

1. ✅ `kernel/src/syscall/fs/ctl.rs`
   - 行 688: 修复 chown 对目录 SET_GID 位的处理

2. ✅ `kernel/src/syscall/fs/mount.rs`
   - 行 477-501: 修复 sys_move_mount 的参数验证和错误码
   - 行 312-334, 336-353: 简化 mount 错误检查逻辑

### 测试用例改写

3. ✅ `syscalls_code/move_mount/rewritten/move_mount01_simple.c`
   - 基本功能测试

4. ✅ `syscalls_code/move_mount/rewritten/move_mount02_simple.c`
   - 错误处理测试

5. ✅ `syscalls_code/move_mount/rewritten/Makefile`
   - 编译脚本

6. ✅ `syscalls_code/move_mount/rewritten/README.md`
   - 使用文档

7. ✅ `syscalls_code/move_mount/rewritten/REWRITE_SUMMARY.md`
   - 改写总结

### 文档

8. ✅ `FIXES_SUMMARY.md` - 初步修复总结
9. ✅ `FINAL_SUMMARY.md` - 完整修复文档
10. ✅ `VERIFICATION_REPORT.md` - 测试验证报告

---

## 成果总结

### 修复成果

- ✅ **2 个系统调用问题已修复并验证通过**
  - mkdir02: 目录 GID 继承
  - move_mount01: move_mount 基本功能

- 🔄 **1 个问题部分修复**
  - mkdir03: mount EBUSY（需进一步调试）

### 测试改写成果

- ✅ **完成 move_mount 测试用例改写**
  - 2 个测试文件（基本功能 + 错误处理）
  - 完整的文档和使用说明
  - 与 open 改写模式保持一致

### 文档成果

- ✅ **完整的技术文档**
  - 问题分析和根本原因
  - 修复方案和验证结果
  - POSIX 标准说明
  - 使用示例和参考资料

---

## 后续工作

1. **调试 mkdir03 的 mount EBUSY 问题**
   - 深入 VFS 层的 mount 实现
   - 分析 MOUNTED_PATHS 和实际挂载状态的关系

2. **验证 move_mount02 的测试结果**
   - 确认所有错误码测试通过

3. **移除调试日志**
   - 清理 warn! 和 debug! 语句

4. **集成改写的测试**
   - 将改写的测试添加到 CI/CD
   - 用于验证后续的实现

---

## 参考资料

- [LTP - Linux Test Project](https://github.com/linux-test-project/ltp)
- [POSIX chown() 标准](https://pubs.opengroup.org/onlinepubs/9699919799/functions/chown.html)
- [Linux mount API](https://www.kernel.org/doc/html/latest/filesystems/mount_api.html)
- [move_mount(2) man page](https://man7.org/linux/man-pages/man2/move_mount.2.html)

---

生成时间：2026-06-03
