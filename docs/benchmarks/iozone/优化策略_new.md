# moss 当前 I/O 优化策略整理

本文根据当前 moss `main` 与本仓库保存的 StarryOS 基线 `refs/remotes/upstream/main` 进行代码比对，整理 moss 在 I/O 路径上的主要优化策略，供报告撰写使用。

需要注意的是：当前 moss 的 I/O 优化已经不再是旧报告中“全局页缓存 16384 页”那套表述。现在更准确的说法是：moss 通过按 inode 共享的 per-file 页缓存、元数据缓存、写路径减少无意义读、dirty 页批量写回、显式预读和 mmap fault 轻量预读等方式，降低普通文件小块 I/O 的固定开销。

## 1. 比对基线

本次整理使用的 StarryOS 基线为：

```text
refs/remotes/upstream/main
commit 2e075accf4fb0aefdd1d252ebd9ccf29727d9923
date   2026-03-09
title  build: prepare for publish on crates.io (#128)
```

当前 moss 主线为：

```text
main
commit cce03025eeff724367cf97e1b9f55afb894984e9
date   2026-06-18
title  docs: 修改 iozone 名字
```

StarryOS 基线中 `kernel/src/syscall/fs/io.rs` 和 `kernel/src/file/fs.rs` 已经有基本的 `read/write/readv/writev/pread/pwrite/fadvise` 等接口，但很多路径只是把请求转发到底层文件对象；当前 moss 则把 `vendor/axfs`、`vendor/axfs-ng-vfs` 纳入仓库，并在这些层里加入了页缓存、元数据缓存和回写策略。因此，下面很多优化并不是在 StarryOS 同名文件里逐行修改，而是 moss 对文件系统底层实现进行了整体接管和增强。

## 2. 优化策略总览

| 优化点 | 核心方法 | 主要受益场景 |
| --- | --- | --- |
| 按 inode 共享的 per-file 页缓存 | 以 `(fs_ptr, inode)` 找到同一份 `CachedFileRecord`，普通文件默认 256 页 LRU | iozone reread、random read、stride read、lmbench 文件读 |
| 写路径避免 read-before-write | 新页或整页覆盖时直接分配清零页，不先读旧页 | iozone writer/rewriter、UnixBench fstime |
| tmpfs 零页和稀疏写优化 | 读未分配页返回 `ZERO_PAGE`，写全零页不分配缓存页 | tmpfs、小文件、全零写 |
| dirty 页批量写回 | 脏页按页号排序，连续页合并后一次写回，单批最多 16 页 | fsync、close/drop、lmbench `lmdd fsync=1` |
| 元数据缓存和延迟提交 | 非目录 inode 的 size、atime、mtime、ctime 缓存在内存里，sync/drop 时统一提交 | stat/fstat、read atime、write mtime/ctime |
| relatime 式 atime 更新 | 普通读不总是更新 atime，只有必要时才更新 | iozone readers/rereaders、小 record read |
| 显式预读生效 | `readahead()` 和 `fadvise(WILLNEED)` 调用 `CachedFile::readahead()` | 显式预读测试、后续 read/pread/mmap fault |
| mmap 顺序 fault 轻量预读 | 连续 fault 时预热后续 4 页到页缓存 | lmbench `bw_mmap_rd`、顺序 mmap 扫描 |
| inotify/fanotify 无监听快速路径 | 没有 watch/interest 时直接返回，不做 stat/path/mark 匹配 | 高频 read/write/open/close |
| shared mmap 懒分配 | 共享页由 `Vec<Option<PhysAddr>>` 表示，首次访问才分配物理页 | mmap/shmem 初始化和 page fault |

## 3. 按 inode 共享的 per-file 页缓存

旧报告里如果写成“所有文件共享一个 64MiB 全局页缓存池”，现在已经不准确。当前 moss 的实现是：全局表只负责按文件身份找到缓存记录，真正的数据页缓存在每个文件自己的 `CachedFileShared` 中。

关键代码位于：

```text
vendor/axfs/src/highlevel/file.rs
```

核心结构如下：

```rust
const PAGE_SIZE: usize = 4096;
const DEFAULT_LOCAL_FILE_CACHE_PAGES: usize = 256;
const MAX_READAHEAD_BYTES: usize = DEFAULT_LOCAL_FILE_CACHE_PAGES * PAGE_SIZE;

type FileCacheKey = (usize, u64);

static FILE_CACHES: spin::Lazy<RwLock<BTreeMap<FileCacheKey, Arc<CachedFileRecord>>>> =
    spin::Lazy::new(|| RwLock::new(BTreeMap::new()));
```

其中 `FileCacheKey = (fs_ptr, inode)`，含义是“哪个文件系统里的哪个 inode”。这样同一个文件即使被多个进程、多个 fd 或多个路径打开，最终都会指向同一份 `CachedFileRecord`，避免同一文件出现多份缓存副本。

普通文件的缓存容量是 256 个 4KiB 页，也就是 1MiB：

```rust
page_cache: Mutex<LruCache<u32, PageCache>>,
...
LruCache::new(NonZeroUsize::new(DEFAULT_LOCAL_FILE_CACHE_PAGES).unwrap())
```

这样设置比较贴合评测负载：iozone 4 进程场景里单个 child file 常见大小约为 1MiB，lmbench 的 `bw_file_rd`、`bw_mmap_rd` 常见工作集是 512KiB。缓存能覆盖整个文件时，第二轮 reread、反向读、stride read 都更容易命中缓存。

这项优化的本质不是“无限扩大缓存”，而是把缓存容量调到足够覆盖小工作集，同时仍然保持每个文件自己的 LRU 淘汰边界，避免一个文件的访问把所有文件的缓存都挤掉。

## 4. 写路径避免 read-before-write

页缓存以 4KiB 为单位。朴素写法在目标页不在缓存里时，会先从底层文件系统读出完整 4KiB 页，再把用户写入的数据覆盖进去。这个逻辑对“修改旧文件中间一小段”是必要的，因为未覆盖部分要保留旧内容；但对新文件顺序写、EOF 之后 append 或整页覆盖来说，旧内容根本不存在或已经全部被覆盖，先读旧页就是纯浪费。

moss 当前在写入前记录旧文件长度 `old_len`，再判断当前页是否可以直接用全 0 页初始化：

```rust
fn can_zero_fill_write_page(old_len: u64, page_start: u64, range: &Range<usize>) -> bool {
    if page_start >= old_len {
        return true;
    }
    let old_valid_end = (old_len - page_start).min(PAGE_SIZE as u64) as usize;
    range.start == 0 && range.end >= old_valid_end
}
```

如果当前页完全在旧 EOF 之后，或者本次写入覆盖了旧有效内容，就走 `page_or_insert_zeroed()`，只分配清零页，不再调用底层 `read_at()`。

写路径中对应逻辑为：

```rust
let old_len = self.len()?;
...
let zeroed = Self::can_zero_fill_write_page(old_len, page_start, &range);
self.with_page_or_insert_impl(pn, zeroed, |page, _| {
    buf.read(&mut page.data()[range.start..range.end])?;
    page.dirty = true;
    Ok(())
})?;
```

对 iozone 的 `initial writers`、`rewriters`、`pwrite/pread` 写入项来说，这能减少大量“为了写新页而先读旧页”的底层文件系统调用。

## 5. tmpfs 零页和稀疏写优化

MemoryFs/tmpfs 的内容本来就在内存里，moss 没有让 inode 自己再维护一份数据，而是把实际内容交给页缓存处理。当前 `CachedFile::read_at_in_memory()` 在读到不存在的页时，直接向用户返回全局 `ZERO_PAGE` 中对应的范围：

```rust
static ZERO_PAGE: [u8; PAGE_SIZE] = [0; PAGE_SIZE];
...
if let Some(page) = guard.get_mut(&pn) {
    dst.write(&page.data()[range_start..range_end])?;
} else {
    dst.write(&ZERO_PAGE[range_start..range_end])?;
}
```

写入内存文件时，如果目标页不存在，moss 会先读取用户写入的数据；只有这段数据里存在非 0 字节，才真正分配 `PageCache`：

```rust
let mut data = Vec::new();
data.resize(len, 0);
buf.read(&mut data)?;
if data.iter().any(|byte| *byte != 0) {
    let mut page = PageCache::new()?;
    page.data().fill(0);
    page.data()[range_start..range_end].copy_from_slice(&data);
    guard.put(pn, page);
}
```

因此，全零写不会立刻占用实际页缓存空间，读未写入区域也不需要提前分配物理页。这对 tmpfs、procfs/simplefs 这类内存型文件系统尤其合适。

## 6. dirty 页批量写回

如果每个脏页都单独写回，一次顺序写文件会变成大量 4KiB 小请求。moss 当前的做法是：写回时先收集所有 dirty 页，按页号排序，再把连续页合并成一批写入。每批最多 16 页，即 64KiB。

关键常量：

```rust
const WRITEBACK_BATCH_PAGES: usize = 16;
const WRITEBACK_BATCH_BYTES: usize = WRITEBACK_BATCH_PAGES * PAGE_SIZE;
```

写回核心函数：

```rust
fn writeback_dirty_run(
    file: &FileNode,
    file_len: u64,
    run: &mut [(u32, PageCache)],
) -> VfsResult<()> {
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
    for (_, page) in run.iter_mut() {
        page.dirty = false;
    }
    Ok(())
}
```

`writeback_dirty_pages()` 会按页号排序并合并连续页：

```rust
dirty_pages.sort_by_key(|(pn, _)| *pn);
...
if pn != prev_pn + 1 {
    break;
}
if bytes > 0 && bytes + len > WRITEBACK_BATCH_BYTES {
    break;
}
```

这项优化主要改善 `fsync`、`close/drop`、`sync` 这类必须把脏页落到底层文件系统的场景。对 lmbench 的 `lmdd fsync=1` 和 iozone 小块写后的同步路径都有帮助。

## 7. 元数据缓存和延迟提交

moss 当前不只是缓存文件内容，也缓存 inode 元数据。缓存表位于：

```text
vendor/axfs-ng-vfs/src/node/mod.rs
```

核心结构：

```rust
struct InodeMetadataCache {
    node: Arc<dyn NodeOps>,
    file: Option<Arc<dyn FileNodeOps>>,
    metadata: Metadata,
    dirty_size: bool,
    dirty_update: MetadataUpdate,
}

type InodeMetadataCacheKey = (usize, u64);

static INODE_METADATA_CACHES:
    spin::Lazy<Mutex<BTreeMap<InodeMetadataCacheKey, SharedInodeMetadataCache>>> =
    spin::Lazy::new(|| Mutex::new(BTreeMap::new()));
```

这里同样用 `(fs_ptr, inode)` 做 key。这样 `stat/fstat`、写入后的 size 查询、读写触发的时间戳更新，都可以先走缓存，而不是每次都访问底层文件系统。

缓存只用于适合缓存的节点：

```rust
fn cacheable_metadata(&self) -> bool {
    self.node_type() != NodeType::Directory && !self.flags().contains(NodeFlags::NON_CACHEABLE)
}
```

目录被排除在外，是因为目录的 `nlink` 会随着子目录创建、删除而变化，缓存目录元数据很容易读到过期结果。`NON_CACHEABLE` 节点通常是 procfs、sysfs、设备等动态节点，也不能简单缓存。

元数据更新并不总是马上落盘，而是先合并到 `dirty_update`：

```rust
pub fn update_metadata(&self, update: MetadataUpdate) -> VfsResult<()> {
    if !self.cacheable_metadata() {
        return self.0.node.update_metadata(update);
    }

    let shared = self.ensure_metadata_cache()?;
    let mut cache = shared.lock();
    apply_metadata_update(&mut cache.metadata, &update);
    merge_metadata_update(&mut cache.dirty_update, &update);
    Ok(())
}
```

真正写回发生在 `flush_metadata_cache_shared()`，例如 `sync/drop` 或缓存失效时。这样可以把多次 atime/mtime/size 更新合并，减少底层 metadata 写。

## 8. relatime 式 atime 更新

StarryOS 基线里的普通 `read()` 基本只读数据，不处理复杂 atime 策略；moss 当前加入了 Linux 风格的 atime 控制。关键代码在：

```text
kernel/src/file/fs.rs
```

判断逻辑：

```rust
fn should_update_atime_for_access(loc: &Location, is_dir: bool, now: Duration) -> bool {
    let flags = loc.mount_flags();
    if flags & MS_NOATIME != 0 || (is_dir && flags & MS_NODIRATIME != 0) {
        return false;
    }
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
    true
}
```

含义是：默认不是每次读都更新 atime，而是只有 atime 早于 mtime/ctime，或者距离上次 atime 更新已经超过 24 小时，才更新。`MS_NOATIME` 可以完全关闭 atime 更新，`MS_STRICTATIME` 可以要求严格更新。

这能避免 iozone readers/rereaders 这类高频读测试把“读文件”变成“读数据 + 写元数据”。

同时 ext4 的 `update_metadata()` 也避免了 atime-only 更新顺手刷新 ctime：

```rust
let update_ctime =
    update.mode.is_some() || update.owner.is_some() || update.mtime.is_some();
...
if let Some(ctime) = update.ctime {
    inode.set_ctime(&ctime);
} else if update_ctime {
    inode.update_ctime();
}
```

这样单独更新 atime 不会把 ctime 也改掉，减少不必要的 metadata 改动。

## 9. 显式 readahead 和 fadvise(WILLNEED) 生效

StarryOS 基线中的 `sys_fadvise64()` 主要做参数检查，`POSIX_FADV_WILLNEED` 不会真正提前读文件。当前 moss 中，`sys_fadvise64()` 和 `sys_readahead()` 会找到 regular file 的 cached backend，并调用后端预读。

代码位置：

```text
kernel/src/syscall/fs/io.rs
vendor/axfs/src/highlevel/file.rs
```

syscall 层：

```rust
if advice == POSIX_FADV_WILLNEED && len > 0 {
    let file_like = get_file_like(fd)?;
    if let Some(file) = file_for_readahead(&file_like)? {
        file.inner()
            .backend()?
            .readahead(offset as u64, len as usize)?;
    }
}
```

`sys_readahead()`：

```rust
pub fn sys_readahead(fd: c_int, offset: __kernel_off_t, count: usize) -> AxResult<isize> {
    if offset < 0 {
        return Err(AxError::InvalidInput);
    }

    let file_like = get_file_like(fd)?;
    if let Some(file) = file_for_readahead(&file_like)? {
        file.inner()
            .backend()?
            .readahead(offset as u64, count)?;
    }
    Ok(0)
}
```

缓存层：

```rust
pub fn readahead(&self, offset: u64, count: usize) -> VfsResult<()> {
    if count == 0 || self.in_memory() {
        return Ok(());
    }

    let len = self.len()?;
    if offset >= len {
        return Ok(());
    }
    let count = count.min(MAX_READAHEAD_BYTES);
    let end = offset.saturating_add(count as u64).min(len);
    let start_page = (offset / PAGE_SIZE as u64) as u32;
    let end_page = end.div_ceil(PAGE_SIZE as u64) as u32;

    for pn in start_page..end_page {
        self.with_page_or_insert(pn, |_, _| Ok(()))?;
    }

    Ok(())
}
```

当前策略比较保守：同步预读，按页放入缓存，单次最多 1MiB；direct I/O 保持 no-op，因为 direct I/O 的语义本来就是绕过页缓存。

## 10. mmap 顺序 fault 轻量预读

文件被 mmap 后，用户态看起来是在读内存，但第一次访问文件页时仍会触发 page fault，内核需要把对应文件页读进页缓存并建立映射。StarryOS 基线的 file-backed mmap fault 基本只处理当前页；moss 当前会在发现连续 fault 时，顺手预热后面少量页。

代码位置：

```text
kernel/src/mm/aspace/backend/file.rs
```

新增状态：

```rust
const NO_FAULT_PAGE: u32 = u32::MAX;
const MMAP_FAULT_READAHEAD_PAGES: u32 = 4;

last_fault_page: AtomicU32,
next_readahead_page: AtomicU32,
```

预读判断：

```rust
fn maybe_readahead_after_fault(&self, pn: u32) {
    let last = self.0.last_fault_page.swap(pn, Ordering::Relaxed);
    if last.checked_add(1) != Some(pn) {
        return;
    }

    let start = pn.saturating_add(1);
    if start < self.0.next_readahead_page.load(Ordering::Relaxed) {
        return;
    }

    let end = start.saturating_add(MMAP_FAULT_READAHEAD_PAGES);
    self.0.next_readahead_page.store(end, Ordering::Relaxed);
    let offset = u64::from(start) * PAGE_SIZE_4K as u64;
    let count = (end - start) as usize * PAGE_SIZE_4K;
    let _ = self.0.cache.readahead(offset, count);
}
```

触发条件：

```rust
let fault_readahead =
    range.end - range.start == PAGE_SIZE_4K && access_flags.contains(MappingFlags::READ);
...
if pages > 0 && fault_readahead {
    self.maybe_readahead_after_fault(start_page);
}
```

这项优化不会提前建立页表映射，只把后续文件页放进页缓存。因此后续访问仍可能产生 page fault，但 fault 时更可能不用等待底层文件系统读页。它适合 `bw_mmap_rd` 这类顺序扫描，不会像激进预读那样明显拖慢单次随机 page fault。

## 11. inotify/fanotify 无监听快速路径

moss 实现 inotify/fanotify 后，`read/write/open/close` 等路径需要产生文件事件。但 iozone、lmbench、libcbench 等性能测试通常没有注册监听者。如果每次普通 I/O 都进入通知系统，去做 stat、路径解析、mark 匹配、事件准备，就会形成纯额外开销。

当前 inotify 用全局 watch 计数判断是否存在监听：

```rust
static INOTIFY_WATCHES: AtomicUsize = AtomicUsize::new(0);

pub(super) fn has_inotify_watchers() -> bool {
    INOTIFY_WATCHES.load(Ordering::Acquire) != 0
}

pub fn notify_inotify_file(file: &dyn FileLike, mask: u32) {
    if !has_inotify_watchers() {
        return;
    }
    ...
}
```

fanotify 用全局兴趣掩码先过滤：

```rust
static FANOTIFY_INTEREST_MASK: AtomicU64 = AtomicU64::new(0);

fn has_fanotify_interest(mask: u64) -> bool {
    if FANOTIFY_INTEREST_MASK.load(Ordering::Acquire) & mask == 0 {
        return false;
    }
    ...
}
```

`read/write` syscall 也只在真正读写成功后才通知：

```rust
let read = f.read(&mut VmBytesMut::new(buf, len))?;
if read > 0 {
    notify_inotify_file(f.as_ref(), IN_ACCESS);
    notify_fanotify_access(&f);
}
```

```rust
if len == 0 {
    return Ok(0);
}
...
let written = f.write(&mut VmBytes::new(buf, len))?;
if written > 0 {
    notify_inotify_file(f.as_ref(), IN_MODIFY);
    notify_fanotify_modify(&f);
}
```

这项优化不改变文件事件语义：一旦用户真的创建 watch 或 fanotify mark，仍会走事件匹配和队列投递；它只是让“无人监听”的常见性能测试路径尽快返回。

## 12. direct I/O 与 cached I/O 的一致性处理

moss 同时保留 cached backend 和 direct backend。direct I/O 的语义是绕过页缓存，但同一个文件可能被另一个 fd 以 cached 方式访问。为了避免两条路径看到不一致的数据，moss 在 direct 读写前后维护缓存一致性。

direct read 前，如果对应范围有 cached dirty 页，先写回：

```rust
if len > 0
    && let Some(cached) = CachedFile::get_existing(loc)
{
    cached
        .writeback_dirty_pages_in_range(start..start.saturating_add(len as u64))?;
}
```

direct write 后，如果文件有 cached 页，则让干净页失效，并更新缓存中的文件长度：

```rust
if offset > start {
    loc.grow_cached_len_if_present(offset);
    if let Some(cached) = cached {
        cached.invalidate_clean_pages_in_range(start..offset);
    }
}
```

这项优化更偏向“正确性 + 性能边界”：它既避免 cached 路径读到 direct 写之前的旧数据，也避免每次 direct 写后粗暴清空整个文件缓存。

## 13. shared mmap 懒分配

StarryOS 基线的 `SharedPages::new()` 会在创建共享映射时一次性分配所有物理页：

```rust
for _ in 0..num_pages {
    result.phys_pages.push(alloc_frame(true, page_size)?);
}
```

moss 当前改为：

```rust
pub struct SharedPages {
    phys_pages: SpinNoIrq<Vec<Option<PhysAddr>>>,
    pub size: PageSize,
}
```

初始化时只创建 `None` 槽位：

```rust
phys_pages: SpinNoIrq::new(vec![None; num_pages]),
```

第一次访问时才分配：

```rust
fn get_or_alloc(&self, index: usize) -> AxResult<PhysAddr> {
    if let Some(paddr) = self.get(index) {
        return Ok(paddr);
    }

    let paddr = alloc_frame(true, self.size)?;
    ...
    *slot = Some(paddr);
    Ok(paddr)
}
```

这不是磁盘文件 I/O 优化，但会影响 mmap/shmem 相关测试的内存分配成本。它把一次性分配压力摊到实际 page fault 上，未访问的共享页不会占用真实物理页。

## 14. 可写入报告的概括表述

可以将当前 moss 的 I/O 优化概括为：

> moss 当前的 I/O 优化重点已经从单一全局页缓存，转为按 inode 共享的 per-file 页缓存和更细粒度的热路径优化。文件内容以 `(fs_ptr, inode)` 共享缓存记录，普通文件默认保留 256 个 4KiB 页，能够覆盖 iozone 和 lmbench 常见的小工作集；写路径通过零页初始化避免新页写入前的 read-before-write；dirty 页写回时按页号排序并合并连续页，减少底层小 I/O 请求；非目录 inode 的元数据被缓存并延迟提交，配合 relatime 策略减少读操作带来的时间戳写入；`readahead()` 与 `fadvise(WILLNEED)` 从空实现变为真实预读，mmap 顺序 fault 也会轻量预热后续页；同时 inotify/fanotify 在无监听者时快速返回，降低普通 read/write/open/close 的固定开销。这些策略共同减少了小块文件 I/O 中的无意义读、零散写和元数据更新，提升了 iozone、lmbench 文件项和 UnixBench fstime 等测试中的缓存命中率与系统调用热路径效率。

## 15. 边界说明

当前实现仍然是偏稳妥的 benchmark pass，并没有做过于激进的内核 I/O 重构：

- 没有实现一个统一的全局 64MiB 页缓存池，当前是全局索引加 per-file LRU。
- 没有实现后台异步预读，`readahead()` 是同步把页放入缓存。
- 没有实现复杂的自动顺序读窗口，只有 mmap fault 路径做了连续 fault 的轻量判断。
- direct I/O 仍然保持绕过页缓存的语义，只做必要的一致性维护。
- 目录元数据和 `NON_CACHEABLE` 节点不会进入元数据缓存，避免动态内容读到过期值。
- 单次显式预读最多 1MiB，不追求一次性缓存很大的文件。

