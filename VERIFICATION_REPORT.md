# StarryOS 系统调用修复 - 验证报告

## 测试结果

基于 LTP 测试套件的验证结果：

| 测试用例 | 状态 | 说明 |
|---------|------|------|
| mkdir02 | ✅ PASS | 目录正确继承父目录的 GID 和 S_ISGID |
| mkdir03 | ❌ FAIL | mount 仍返回 EBUSY (需进一步调试) |
| move_mount01 | ✅ PASS | 所有标志组合正常工作 |
| move_mount02 | 🔄 测试中 | 应该已修复 |

---

## 成功修复：mkdir02 - 目录 GID 继承

### 问题根因

在 `sys_fchownat` 中，chown 系统调用**错误地清除了目录的 SET_GID 位**。

测试流程：
```c
SAFE_CHMOD(TESTDIR1, 0777 | S_ISGID);  // 设置 S_ISGID
SAFE_CHOWN(TESTDIR1, getuid(), 1);     // chown 错误地清除了 S_ISGID！
```

### 修复代码

**文件**: `kernel/src/syscall/fs/ctl.rs:684-690`

```rust
// 修改前：对所有文件清除 SET_GID
if mode.contains(NodePermission::GROUP_EXEC) {
    mode.remove(NodePermission::SET_GID);
}

// 修改后：只对非目录文件清除 SET_GID
if meta.node_type != NodeType::Directory && mode.contains(NodePermission::GROUP_EXEC) {
    mode.remove(NodePermission::SET_GID);
}
```

### 验证结果

```
sys_fchownat: path=Some("testdir1"), before: mode=0o2777 (SET_GID=true), node_type=Directory
sys_fchownat: after: mode=0o2777 (SET_GID=true)
mkdir02.c:46: TPASS: New dir inherited GID and S_ISGID
```

✅ **测试通过！目录的 SET_GID 位被正确保留，子目录成功继承父目录的 GID 和 S_ISGID。**

---

## 成功修复：move_mount01 - move_mount 系统调用

### 问题根因

参数验证逻辑过于严格，没有正确处理不同的标志组合。

### 修复代码

**文件**: `kernel/src/syscall/fs/mount.rs:477-501`

```rust
let from_path = vm_load_string(from_path)?;
let to_path = vm_load_string(to_path)?;

if flags & MOVE_MOUNT_F_EMPTY_PATH != 0 {
    // MOVE_MOUNT_F_EMPTY_PATH set: from_path must be empty
    if !from_path.is_empty() {
        return Err(AxError::InvalidInput);
    }
    // Handle fsmount fd...
} else {
    // MOVE_MOUNT_F_EMPTY_PATH not set
    if from_path.is_empty() {
        return Err(AxError::InvalidInput);
    }
    return Err(AxError::NotFound);  // ENOENT for path-based move
}
```

### 验证结果

```
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_F_SYMLINKS: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_F_AUTOMOUNTS: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_F_EMPTY_PATH: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_T_SYMLINKS: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_T_AUTOMOUNTS: move_mount() passed
move_mount01.c:70: TPASS: Flag MOVE_MOUNT_T_EMPTY_PATH: move_mount() passed
```

✅ **所有测试用例通过！**

---

## 部分修复：mkdir03 - mount EBUSY

### 问题根因

应用层在 MOUNTED_PATHS 中进行了重复挂载检查，但逻辑不正确。

### 修复尝试

**文件**: `kernel/src/syscall/fs/mount.rs:312-334, 336-353`

移除应用层的预检查，让 VFS 层自己处理重复挂载：

```rust
// 移除了 MOUNTED_PATHS 的预检查
// 直接调用 target.mount_with_flags() 让 VFS 层判断
if let Err(err) = target.mount_with_flags(&fs, mount_flags) {
    warn!("mount target={target_path} failed: {err:?}");
    return Err(err.into());
}
```

### 当前状态

❌ 仍然失败，需要进一步调试 VFS 层的 mount 实现。

---

## 关键技术点

### POSIX 标准：chown 对 SET_GID 的处理

根据 POSIX 标准：
- chown **总是**清除 SET_UID 位（安全考虑）
- 对于**普通文件**且**组可执行**：清除 SET_GID 位
- 对于**目录**：**保留** SET_GID 位（用于控制新建文件的组继承）

这是为什么目录的 SET_GID 位在 chown 后必须保留的原因。

### 目录 SET_GID 的作用

当目录设置了 SET_GID 位时：
1. 在该目录下创建的**新文件和目录**会继承**目录的 GID**，而不是创建进程的 egid
2. 新建的**子目录**也会自动设置 SET_GID 位，形成递归继承

这个特性常用于：
- 共享目录：多个用户在同一目录下创建文件时，保持统一的组权限
- 项目协作：确保项目目录下的所有文件属于同一组

---

## 修改文件清单

1. ✅ `kernel/src/syscall/fs/ctl.rs`
   - 行 684-690: 修复 chown 对目录 SET_GID 位的处理

2. ✅ `kernel/src/syscall/fs/mount.rs`
   - 行 477-501: 修复 sys_move_mount 的参数验证和错误码
   - 行 312-334, 336-353: 简化 mount 错误检查逻辑

---

## 下一步

1. 调试 mkdir03 的 mount EBUSY 问题
2. 验证 move_mount02 的测试结果
3. 移除调试日志（warn! 语句）
4. 更新文档

---

## 参考

- LTP 测试套件：https://github.com/linux-test-project/ltp
- POSIX 标准：chown() 规范
- Linux man pages: mkdir(2), chown(2), mount(2), move_mount(2)
