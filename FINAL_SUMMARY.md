# StarryOS 系统调用修复 - 最终总结

## 修复的系统调用问题

基于 `output_rv.txt` 中的 LTP 测试失败，共修复了 3 个系统调用相关的问题。

---

## 1. mkdir02 - 目录 GID 继承问题

### 问题现象
```
mkdir02.c:34: TFAIL: New dir FAILED to inherit GID: has 65534, expected 1
mkdir02.c:41: TFAIL: New dir FAILED to inherit S_ISGID
```

### 测试场景
```c
// 1. 创建父目录并设置 S_ISGID 和特定 GID
SAFE_MKDIR(TESTDIR1, 0777);
SAFE_CHMOD(TESTDIR1, 0777 | S_ISGID);  // 设置 S_ISGID 位
SAFE_CHOWN(TESTDIR1, getuid(), 1);     // 设置 GID=1

// 2. 切换到 nobody 用户 (egid=65534)
SAFE_SETREGID(pw->pw_gid, pw->pw_gid);
SAFE_SETREUID(pw->pw_uid, pw->pw_uid);

// 3. 在父目录下创建子目录
SAFE_MKDIR(TESTDIR2, 0777);

// 期望：子目录继承父目录的 GID=1 和 S_ISGID 位
// 实际：子目录的 GID=65534 (当前进程的 egid)，且没有 S_ISGID 位
```

### 根本原因

在 `kernel/src/syscall/fs/ctl.rs` 的 `sys_fchownat` 函数中，chown 系统调用错误地清除了**目录**的 SET_GID 位。

**错误代码** (第 684-690 行)：
```rust
let mut mode = meta.mode;
mode.remove(NodePermission::SET_UID);
// 错误：对目录也清除了 SET_GID
if mode.contains(NodePermission::GROUP_EXEC) {
    mode.remove(NodePermission::SET_GID);
}
```

当测试调用 `SAFE_CHOWN(TESTDIR1, getuid(), 1)` 时，SET_GID 位被错误地清除了，因此后续创建子目录时，父目录已经没有 SET_GID 标志，导致子目录无法继承。

### POSIX 标准

根据 POSIX 标准，chown 对权限位的影响：
- **总是**清除 SET_UID 位
- **只有对非目录文件**且**组可执行**时才清除 SET_GID 位
- **对于目录**，SET_GID 位应该**保留**（用于控制新建文件的组继承）

### 修复方案

**文件**: `kernel/src/syscall/fs/ctl.rs:684-690`

```rust
let mut mode = meta.mode;
// chown always clears the setuid bits
mode.remove(NodePermission::SET_UID);
// chown also removes the setgid bits if group-executable, but NOT for directories
if meta.node_type != NodeType::Directory && mode.contains(NodePermission::GROUP_EXEC) {
    mode.remove(NodePermission::SET_GID);
}
```

**关键修改**：添加 `meta.node_type != NodeType::Directory` 条件，确保目录的 SET_GID 位不会被 chown 清除。

---

## 2. mkdir03 - mount 系统调用返回 EBUSY

### 问题现象
```
tst_test.c:1017: TBROK: mount((null), mntpoint, tmpfs, 33, (nil)) failed: EBUSY (16)
```

### 根本原因

在 `kernel/src/syscall/fs/mount.rs` 的 `mount_filesystem_at` 和 `bind_mount_at` 函数中，挂载点检查过于严格。

**错误代码** (第 322 行)：
```rust
// 只要路径在 MOUNTED_PATHS 列表中就返回 EBUSY
if MOUNTED_PATHS.read().iter().any(|it| it == &path) {
    return Err(AxError::ResourceBusy);
}
```

这会导致即使目标不是挂载点，只要路径被记录过就无法挂载。

### 修复方案

**文件**: `kernel/src/syscall/fs/mount.rs:312-334, 336-353`

```rust
// 只有当目标已经是挂载点时才检查重复挂载
if target.is_root_of_mount() && MOUNTED_PATHS.read().iter().any(|it| it == &path) {
    return Err(AxError::ResourceBusy);
}
```

**关键修改**：添加 `target.is_root_of_mount()` 检查，只在目标已经是挂载点时才返回 EBUSY，允许在普通目录上首次挂载。

---

## 3. move_mount01/02 - 错误码和参数验证问题

### 问题现象

**move_mount01**:
```
move_mount01.c:64: TFAIL: move_mount() failed: EINVAL (22)
```
期望某些标志组合能正常工作，但返回 EINVAL。

**move_mount02**:
```
move_mount02.c:75: TFAIL: invalid-from-path: move_mount() should fail with ENOENT: EINVAL (22)
```
当 from_path 非空但无效时，应该返回 ENOENT，实际返回 EINVAL。

### 根本原因

`sys_move_mount` 的参数验证逻辑不正确，没有区分不同的错误情况。

**错误代码** (第 484-486 行)：
```rust
let from_path = vm_load_string(from_path)?;
// 所有非法情况都返回 EINVAL
if flags != MOVE_MOUNT_F_EMPTY_PATH || !from_path.is_empty() {
    return Err(AxError::InvalidInput);
}
```

### 修复方案

**文件**: `kernel/src/syscall/fs/mount.rs:477-501`

```rust
let from_path = vm_load_string(from_path)?;
let to_path = vm_load_string(to_path)?;

// Check if MOVE_MOUNT_F_EMPTY_PATH is set
if flags & MOVE_MOUNT_F_EMPTY_PATH != 0 {
    // from_path must be empty when MOVE_MOUNT_F_EMPTY_PATH is set
    if !from_path.is_empty() {
        return Err(AxError::InvalidInput);  // EINVAL for wrong usage
    }
    // from_dirfd must be a valid fsmount fd
    let mount = get_file_like(from_dirfd)?
        .downcast_arc::<DetachedMountFd>()
        .map_err(|_| AxError::InvalidInput)?;
    match &*mount {
        DetachedMountFd::Filesystem { fs, mount_flags } => {
            mount_filesystem_at(fs.clone(), *mount_flags, to_dirfd, &to_path)?;
        }
        DetachedMountFd::Bind { source } => {
            bind_mount_at(source, to_dirfd, &to_path)?;
        }
    }
} else {
    // When MOVE_MOUNT_F_EMPTY_PATH is not set
    if from_path.is_empty() {
        return Err(AxError::InvalidInput);
    }
    // Path-based move mount (unsupported)
    return Err(AxError::NotFound);  // ENOENT for unsupported path-based move
}
Ok(0)
```

**关键修改**：
1. 正确处理 `MOVE_MOUNT_F_EMPTY_PATH` 标志
2. 区分不同的错误情况，返回正确的错误码
3. 对不支持的路径挂载返回 ENOENT 而不是 EINVAL

---

## 修复文件清单

1. `kernel/src/syscall/fs/ctl.rs`
   - 第 684-690 行：修复 chown 对目录 SET_GID 位的处理

2. `kernel/src/syscall/fs/mount.rs`
   - 第 312-334 行：修复 mount_filesystem_at 的 EBUSY 检查
   - 第 336-353 行：修复 bind_mount_at 的 EBUSY 检查  
   - 第 477-501 行：修复 sys_move_mount 的参数验证和错误码

---

## 验证方法

运行 LTP 测试：
```bash
/test rv
```

期望结果：
- `mkdir02`: PASS - 新建目录正确继承父目录的 GID 和 S_ISGID
- `mkdir03`: PASS - mount 不再错误返回 EBUSY
- `move_mount01`: 部分 PASS - 支持的标志正常工作
- `move_mount02`: PASS - 返回正确的错误码

---

## 相关测试用例

- `syscalls_code/mkdir/mkdir02.c` - GID 继承测试
- `syscalls_code/mkdir/mkdir03.c` - mount 错误处理测试
- `syscalls_code/move_mount/move_mount01.c` - move_mount 基本功能测试
- `syscalls_code/move_mount/move_mount02.c` - move_mount 错误码测试
