# I/O 优化过程记录

本文记录 iozone、lmbench 文件项、UnixBench fstime 等 I/O 类测试相关优化。每一项按收益优先级整理，固定说明四件事：原来的问题、修改了什么、为什么能提速、对应代码。

## 总体思路

这轮 I/O 优化没有改变文件系统语义，也没有按 benchmark 名称做特殊分支。主要做的是减少普通文件读写中的明显浪费：

| 优先级 | 优化点 | 主要受益测试 |
| --- | --- | --- |
| P0 | 页缓存容量从 256KiB 提到 1MiB | iozone reread、lmbench `bw_file_rd`、`bw_mmap_rd` |
| P1 | 新文件写入避免 read-before-write | iozone writer、UnixBench fstime |
| P2 | dirty page 连续批量写回 | lmbench `lmdd fsync=1`、小文件 fsync |
| P3 | 无 inotify/fanotify 监听时快速跳过通知 | lmbench read/write/open/close、iozone 小块 I/O、libcbench stdio |
| P4 | `read()` atime 改为 relatime | iozone 小 record read、普通文件热读 |
| P5 | `readahead()` / `fadvise(WILLNEED)` 真正预读 | 显式预读测试、mmap 后续 fault |
| P6 | mmap 顺序 fault 轻量预热 | lmbench `bw_mmap_rd` |

## P0: 页缓存容量从 64 页提升到 256 页

**原来的问题**

普通 cached file 以前只保留 64 个 4KiB 页，也就是 256KiB。iozone 4 进程测试里每个 child file 是 1MiB，lmbench 文件读和 mmap 读常见工作集是 512KiB。缓存容量小于工作集时，顺序读完一遍后，文件开头的页已经被 LRU 淘汰，下一轮 reread 又要重新从底层文件系统读。

**修改了什么**

把普通本地文件的 per-file page cache 提到 256 页，也就是 1MiB。

代码引用：

```rust
// vendor/axfs/src/highlevel/file.rs
const PAGE_SIZE: usize = 4096;
const DEFAULT_LOCAL_FILE_CACHE_PAGES: usize = 256;
const MAX_READAHEAD_BYTES: usize = DEFAULT_LOCAL_FILE_CACHE_PAGES * PAGE_SIZE;
```

缓存创建时使用这个容量：

```rust
// vendor/axfs/src/highlevel/file.rs
LruCache::new(NonZeroUsize::new(DEFAULT_LOCAL_FILE_CACHE_PAGES).unwrap())
```

**为什么能提速**

1MiB cache 可以完整覆盖 iozone 单个 child file，也能覆盖 lmbench 512KiB 文件。第二轮读、反向读、stride read、mmap fault 后续访问更容易直接命中页缓存，减少底层 ext4/lwext4 读和路径锁开销。

**边界**

tmpfs/in-memory 文件本来就不按这个 LRU 容量限制，不受影响。这里也没有实现全局 page cache 回收，所以容量不能继续无脑放大。

## P1: cached 写路径避免 read-before-write

**原来的问题**

页缓存以 4KiB 为单位。旧逻辑在缓存里没有目标页时，会先从底层文件系统读出完整 4KiB，再把用户写入的数据覆盖进去。这个逻辑对“修改旧文件中间一小段”是必要的，因为未覆盖部分要保留旧内容。

但 benchmark 常见场景是新建文件后连续写，或者 append 到 EOF 之后的新页。这些页没有旧内容需要保留，先读旧页就是纯浪费。1KiB record 写文件时，这个浪费会被放大。

**修改了什么**

写入前先记录旧文件长度 `old_len`，再判断当前页是否可以直接用全 0 页填充。如果页完全在旧 EOF 之后，或者本次写入覆盖了该页旧有效内容，就不再读底层旧页。

代码引用：

```rust
// vendor/axfs/src/highlevel/file.rs
fn can_zero_fill_write_page(old_len: u64, page_start: u64, range: &Range<usize>) -> bool {
    if page_start >= old_len {
        return true;
    }
    let old_valid_end = (old_len - page_start).min(PAGE_SIZE as u64) as usize;
    range.start == 0 && range.end >= old_valid_end
}
```

写路径使用这个判断：

```rust
// vendor/axfs/src/highlevel/file.rs
let old_len = self.len()?;
...
let zeroed = Self::can_zero_fill_write_page(old_len, page_start, &range);
self.with_page_or_insert_impl(pn, zeroed, |page, _| {
    buf.read(&mut page.data()[range.start..range.end])?;
    page.dirty = true;
    Ok(())
})?;
```

**为什么能提速**

对新建文件连续写来说，原来每个新页可能经历“读 4KiB 旧页 + 写用户数据”，现在变成“分配零页 + 写用户数据”。这减少了底层 `read_at()` 次数，也减少了进入 ext4/lwext4 的次数。iozone writer、rewriter 和 UnixBench fstime 这种小块连续写最容易受益。

**正确性**

如果写入没有覆盖旧页全部有效内容，仍然走原来的 `page_or_insert()`，会先读旧页再修改，保证未覆盖字节不丢。

## P2: dirty page 连续批量写回

**原来的问题**

lmbench 里有类似下面的测试：

```text
lmdd ... of=/var/tmp/XXX move=1m fsync=1
```

它会写 1MiB 文件再 `fsync`。旧的全量写回路径会按 LRU 一页页写，每页 4KiB，连续 1MiB dirty data 会变成很多次小 `file.write_at()`。

**修改了什么**

全量写回时先收集 dirty 页，按页号排序，把连续页合并成一批写回。当前批量上限是 16 页，也就是 64KiB。

代码引用：

```rust
// vendor/axfs/src/highlevel/file.rs
const WRITEBACK_BATCH_PAGES: usize = 16;
const WRITEBACK_BATCH_BYTES: usize = WRITEBACK_BATCH_PAGES * PAGE_SIZE;
```

连续 dirty 页合并：

```rust
// vendor/axfs/src/highlevel/file.rs
dirty_pages.sort_by_key(|(pn, _)| *pn);
...
if pn != prev_pn + 1 {
    break;
}
if bytes > 0 && bytes + len > WRITEBACK_BATCH_BYTES {
    break;
}
Self::writeback_dirty_run(file, file_len, &mut dirty_pages[start..end])?;
```

批量写回：

```rust
// vendor/axfs/src/highlevel/file.rs
let start = run[0].0 as u64 * PAGE_SIZE as u64;
let mut buf = Vec::new();
for (pn, page) in run.iter_mut() {
    let len = Self::page_write_len(file_len, *pn);
    if len > 0 {
        buf.extend_from_slice(&page.data()[..len]);
    }
}
if !buf.is_empty() {
    file.write_at(&buf, start)?;
}
```

**为什么能提速**

把多个 4KiB 写合并成最多 64KiB 写，可以减少 `write_at()` 调用次数、ext4 锁进入次数和底层块写路径调度次数。对 `fsync=1`、小文件同步写回这类测试比较直接。

**正确性**

只在写成功后清 dirty。文件尾页通过 `page_write_len()` 按 `file_len` 裁剪，不写超过 EOF。普通 LRU 单页 eviction 语义不变。

## P3: inotify / fanotify 无监听快路径

**原来的问题**

普通 `read/write/open/close` 成功后会准备 inotify/fanotify 通知。benchmark 通常没有创建 watcher 或 fanotify mark，但旧路径仍可能做 `stat()`、路径解析、父目录查找、事件对象准备，最后才发现没人监听。

对 1KiB 小块读写来说，这些固定开销会被调用次数放大。

**修改了什么**

inotify 维护全局 watcher 计数，没有 watcher 时直接返回。fanotify 维护 interest mask，没有感兴趣事件时不解析路径、不构造事件。

代码引用：

```rust
// kernel/src/syscall/fs/inotify.rs
pub(super) fn has_inotify_watchers() -> bool {
    INOTIFY_WATCHES.load(Ordering::Acquire) != 0
}

pub fn notify_inotify_file(file: &dyn FileLike, mask: u32) {
    if !has_inotify_watchers() {
        return;
    }
    let Ok(stat) = file.stat() else {
        return;
    };
    ...
}
```

fanotify 快速判断：

```rust
// kernel/src/syscall/fs/fanotify.rs
fn has_fanotify_interest(mask: u64) -> bool {
    if FANOTIFY_INTEREST_MASK.load(Ordering::Acquire) & mask == 0 {
        return false;
    }
    ...
}

fn notify_fanotify_event(file: &Arc<dyn FileLike>, mask: u64) {
    if !has_fanotify_interest(mask) {
        return;
    }
    ...
}
```

热路径调用仍保留通知语义：

```rust
// kernel/src/syscall/fs/io.rs
let read = f.read(&mut VmBytesMut::new(buf, len))?;
if read > 0 {
    notify_inotify_file(f.as_ref(), IN_ACCESS);
    notify_fanotify_access(&f);
}
```

**为什么能提速**

没有监听者时，普通文件 I/O 不再为通知系统做路径解析、stat、mark 匹配和事件准备。lmbench `Simple read/write/open/close`、iozone 小 record、libcbench stdio 都会减少固定 syscall 开销。

**正确性**

一旦存在 watcher/mark，仍然走原来的事件匹配和权限请求逻辑。这个优化只跳过“无人监听”的空路径。

## P4: `read()` atime 改为 relatime

**原来的问题**

如果每次 `read()` 都更新 atime，1KiB 小读会额外制造大量元数据写。iozone 的 readers/rereaders 会反复读同一批文件，这个开销很明显。

**修改了什么**

默认采用 Linux 常见 relatime 规则：只有 atime 早于 mtime/ctime，或者距离上次 atime 更新超过 24 小时，才更新 atime。`MS_STRICTATIME` 仍保留每次访问更新，`MS_NOATIME`、`MS_NODIRATIME` 仍按语义跳过。

代码引用：

```rust
// kernel/src/file/fs.rs
if flags & MS_STRICTATIME == 0 {
    let Ok(metadata) = cached_metadata(loc) else {
        return false;
    };
    let stale_by_time = now.checked_sub(metadata.atime).map_or(false, |elapsed| {
        elapsed >= Duration::from_secs(24 * 60 * 60)
    });
    if metadata.atime > metadata.mtime && metadata.atime > metadata.ctime && !stale_by_time {
        return false;
    }
}
```

mount flag 支持：

```rust
// kernel/src/syscall/fs/mount.rs
const SUPPORTED_MOUNT_FLAGS: u32 = MS_RDONLY
    | MS_NODEV
    | MS_NOEXEC
    | MS_NOSUID
    | MS_NOATIME
    | MS_NODIRATIME
    | MS_RELATIME
    | MS_REMOUNT
    | MS_NOSYMFOLLOW
    | MS_STRICTATIME;
```

**为什么能提速**

热读路径少做元数据更新，减少 inode metadata 写、ctime 相关连带变化和文件系统锁争用。

## P5: `readahead()` 和 `fadvise(WILLNEED)` 真正预读

**原来的问题**

`readahead()` 和 `posix_fadvise(..., POSIX_FADV_WILLNEED)` 原来基本是参数检查后返回成功，没有把数据读入页缓存。用户态显式告诉内核“我要用这段文件”时，后续读仍然要等实际 I/O。

**修改了什么**

syscall 层找到 regular file 的 cached backend 后，调用 `CachedFile::readahead()`，按页把文件内容放进页缓存。direct backend 保持 no-op。

代码引用：

```rust
// kernel/src/syscall/fs/io.rs
if advice == POSIX_FADV_WILLNEED && len > 0 {
    let file_like = get_file_like(fd)?;
    if let Some(file) = file_for_readahead(&file_like)? {
        file.inner().backend()?.readahead(offset as u64, len as usize)?;
    }
}
```

```rust
// kernel/src/syscall/fs/io.rs
pub fn sys_readahead(fd: c_int, offset: __kernel_off_t, count: usize) -> AxResult<isize> {
    ...
    if let Some(file) = file_for_readahead(&file_like)? {
        file.inner().backend()?.readahead(offset as u64, count)?;
    }
    Ok(0)
}
```

backend 分发：

```rust
// vendor/axfs/src/highlevel/file.rs
pub fn readahead(&self, offset: u64, count: usize) -> VfsResult<()> {
    match self {
        Self::Cached(cached) => cached.readahead(offset, count),
        Self::Direct(_) => Ok(()),
    }
}
```

**为什么能提速**

后续 `read/pread/mmap fault` 更容易命中页缓存，显式预读测试也不再只是空实现。

## P6: mmap 顺序 fault 轻量预热

**原来的问题**

`bw_mmap_rd` 会 mmap 文件后顺序扫描。旧逻辑是每访问一个未映射页，就 fault 一次、读一页、映射一页。顺序扫描被拆成很多碎片化读页。

**修改了什么**

文件映射后端记录上一次 fault 的页号。如果发现当前 fault 是上一页的下一页，就认为可能在顺序扫描，提前把后续 4 页读进页缓存。

代码引用：

```rust
// kernel/src/mm/aspace/backend/file.rs
const MMAP_FAULT_READAHEAD_PAGES: u32 = 4;

fn maybe_readahead_after_fault(&self, pn: u32) {
    let last = self.0.last_fault_page.swap(pn, Ordering::Relaxed);
    if last.checked_add(1) != Some(pn) {
        return;
    }
    ...
    let _ = self.0.cache.readahead(offset, count);
}
```

只在单页读 fault 后触发：

```rust
// kernel/src/mm/aspace/backend/file.rs
let fault_readahead =
    range.end - range.start == PAGE_SIZE_4K && access_flags.contains(MappingFlags::READ);
...
if pages > 0 && fault_readahead {
    self.maybe_readahead_after_fault(start_page);
}
```

**为什么能提速**

后续页仍然会 fault 建立映射，但 fault 时数据更可能已经在页缓存里，减少底层文件读等待。只对连续 fault 触发，可以避免伤害 `lat_pagefault` 这种随机 fault 测试。

## 未做的事

- 没有实现全局 page cache 回收。
- 没有实现异步后台预读。
- 没有拆 ext4 全局锁。
- 没有改变 direct I/O 绕过页缓存的语义。
- 没有按 benchmark 名称跳过真实 I/O。
