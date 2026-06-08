# I/O Benchmark Pass 优化总结

本文总结本轮围绕 iozone、lmbench、UnixBench fstime 等测例做的 I/O 优化。整体目标是减少小块读写中的无意义开销，让 1KiB record、512KiB 到 1MiB 工作集这类测试更容易命中缓存，并补齐显式预读接口的实际效果。

## 1. 页缓存从 64 页提升到 256 页

代码位置：

- `vendor/axfs/src/highlevel/file.rs`
- `DEFAULT_FILE_CACHE_PAGES`
- `CachedFileShared::new`

原来普通 cached file 的页缓存是：

```text
64 页 * 4KiB = 256KiB
```

现在改为：

```text
256 页 * 4KiB = 1MiB
```

这样做的原因是测试里的文件工作集比 256KiB 大：

- iozone 4 进程测试每个 child file 是 1MiB。
- lmbench `bw_file_rd` 和 `bw_mmap_rd` 使用 512KiB 文件。
- UnixBench fstime 默认会用 1KiB 小块反复读写较小文件。

页缓存太小时，顺序读完整个文件后，LRU 里只剩文件尾部；下一轮从文件头 reread 时，前面读入的新页又会把尾部页挤出去，命中率很差。把 per-file cache 提到 1MiB 后，至少能完整覆盖 iozone 4 进程场景下的单个 child file，也能覆盖 lmbench 的 512KiB 读文件。

tmpfs/in-memory 文件仍然保持 unbounded cache，没有改变。

## 2. cached 写路径避免 read-before-write

代码位置：

- `vendor/axfs/src/highlevel/file.rs`
- `CachedFile::write_at_locked`
- `CachedFile::page_or_insert_zeroed`
- `CachedFile::can_zero_fill_write_page`

axfs 页缓存按 4KiB 一页保存文件内容。原来的写路径比较保守：如果目标页不在缓存里，就先从底层文件系统读出完整 4KiB 页，再把用户写入的数据覆盖进去。

这个逻辑在修改旧文件中间一小段时是必要的，因为没被覆盖的旧数据还要保留。

但在这些场景里，读旧页是浪费的：

- 新建文件后写入第一页。
- 文件末尾 append 到新页。
- 扩展文件后写入 EOF 之后的页。
- 本次写入完整覆盖了旧页中所有有效内容。

现在写路径会先记录写入前的文件长度 `old_len`，再判断每个页是否需要保留旧内容：

```text
如果页完全位于旧 EOF 之后：
    分配全 0 页，直接写入用户数据

如果写入范围覆盖了该页所有旧有效内容：
    分配全 0 页，直接写入用户数据

否则：
    仍然读取旧页，保留未覆盖的旧数据
```

这样可以在不改变文件语义的前提下，减少新文件小块写时的无意义底层读。对 iozone、fstime 这种 1KiB 小块连续写文件的测试比较有帮助。

## 3. 普通 read 的 atime 更新改成 relatime

代码位置：

- `kernel/src/file/fs.rs`
- `update_atime_for_access`

原来普通 `read()` 每次成功后都会更新 atime。对于 1KiB 小读来说，这会让每次读都额外触发一次元数据更新，开销很明显。

现在默认使用 Linux 常见的 relatime 语义：

```text
只有当 atime 早于 mtime，
或 atime 早于 ctime，
或距离上次 atime 更新超过 24 小时，
才真正更新 atime。
```

保留的行为：

- `MS_NOATIME`：不更新 atime。
- `MS_NODIRATIME`：目录访问不更新 atime。
- `MS_STRICTATIME`：仍然每次访问都更新 atime。

这样普通文件读的热路径会少很多元数据写，尤其适合 iozone 这种小 record read 测试。

## 4. ext4 atime-only 更新不再刷新 ctime

代码位置：

- `vendor/axfs/src/fs/ext4/inode.rs`
- `Inode::update_metadata`

原来 ext4 的 `update_metadata` 只要更新 metadata，就会顺手刷新 ctime。这样 atime-only 更新也会导致 ctime 变化。

现在改成：

```text
mode / owner / mtime 变化：
    更新 ctime

只有 atime 变化：
    不更新 ctime
```

这更符合常见文件系统语义，也避免 atime 更新连带制造更多元数据变化。

## 5. mount flag 接受 MS_RELATIME

代码位置：

- `kernel/src/syscall/fs/mount.rs`
- `SUPPORTED_MOUNT_FLAGS`

`/proc/mounts` 中已经呈现 relatime 风格，但 mount flag 白名单之前没有接受 `MS_RELATIME`。

现在把 `MS_RELATIME` 加入支持列表，避免用户态显式传入该 flag 时被错误拒绝。

## 6. readahead 和 fadvise(WILLNEED) 从 no-op 变成真实预读

代码位置：

- `kernel/src/syscall/fs/io.rs`
- `sys_readahead`
- `sys_fadvise64`
- `vendor/axfs/src/highlevel/file.rs`
- `FileBackend::readahead`
- `CachedFile::readahead`

原来 `readahead()` 和 `posix_fadvise(..., POSIX_FADV_WILLNEED)` 基本只是做参数检查，然后返回成功，并不会真的提前读文件。

现在的流程是：

```text
用户调用 readahead 或 fadvise(WILLNEED)
  -> syscall 层检查 fd、offset、文件类型
  -> 找到文件 backend
  -> 如果是 cached backend，把对应文件页读进页缓存
  -> 如果是 direct backend，保持 no-op
```

当前预读策略比较保守：

```text
按 4KiB 页同步预读
单次最多预读 1MiB
不改变文件 offset
不做后台异步预读
不做自动顺序预读
```

这样至少让显式预读接口变得真实有效。用户程序如果提前告诉内核“我马上要用这段文件”，后续 `read`、`pread` 或共享 mmap fault 更可能命中页缓存。

direct I/O 不做预缓存，因为 direct I/O 的语义就是绕过页缓存，通常由用户程序或数据库自己管理缓存。

## 7. 对测试的预期收益

这轮优化主要服务以下测试特点：

- iozone 使用 1KiB record，4 进程场景每个文件 1MiB。
- lmbench 文件读和 mmap 读使用 512KiB 文件。
- UnixBench fstime 使用 1KiB buffer 做 write/read/copy。

预期改善点：

- 1MiB per-file cache 降低小文件 reread 抖动。
- 新文件小块写减少 read-before-write。
- 普通 `read()` 少做 atime 元数据写。
- 显式 `readahead` / `fadvise(WILLNEED)` 后续读更可能命中缓存。

## 8. 当前边界

这轮优化刻意没有做大手术，仍有一些边界：

- 没有实现全局 page cache 回收。
- 没有实现自动顺序预读。
- 没有实现异步后台预读。
- 没有拆 ext4 全局锁。
- 没有改变 direct I/O 的缓存语义。
- 单次显式预读最多 1MiB，不追求一次缓存 LTP `readahead02` 那种 64MiB 文件。

因此这是一轮偏稳妥的 benchmark pass：优先减少明显浪费，提升小工作集 I/O 的缓存命中和写路径效率，同时避免引入大范围调度、块层或内存回收改造。

## 9. inotify / fanotify 无监听快路径

代码位置：
- `kernel/src/syscall/fs/inotify.rs`
- `kernel/src/syscall/fs/fanotify.rs`
- `kernel/src/syscall/fs/io.rs`
- `kernel/src/syscall/fs/fd_ops.rs`
- `kernel/src/syscall/mm/mmap.rs`

这次补充优化的是普通文件 I/O 热路径里的文件事件通知开销。iozone、lmbench、libcbench、Lua 启动和脚本执行这类测试通常不会主动创建 inotify/fanotify 监听者，但原来的 read/write/open/close 路径仍然会进入通知准备逻辑。

原来的典型路径是：

```text
read / write / open / close 成功
  -> 准备 inotify / fanotify 通知
  -> 可能触发 file.stat()
  -> 可能解析 file.path()
  -> 可能查找父目录 target
  -> fanotify 还可能准备 permission/event target
  -> 最后发现其实没有任何监听者
```

这些操作对功能是必要的，但在没有监听者时属于纯额外开销。尤其是 iozone 的小块 read/write 会高频进入 syscall，单次多一点 stat、路径解析或 `Arc::clone()`，累计起来就会影响分数。

现在的处理方式：

```text
inotify：
  维护全局 INOTIFY_WATCHES 计数
  没有 watch 时，notify/prepare 函数直接返回

fanotify：
  在 notify_fanotify_event / request_fanotify_permission 开头检查 has_fanotify_interest(mask)
  没有感兴趣的 mark 时，直接返回

热路径调用：
  read/write/readv/writev/close/mmap 改成传引用
  无监听场景下不再为了通知额外 clone Arc
```

inotify 的 watch 数会在 add_watch、remove_watch 和 InotifyFd drop 时维护，避免每次普通 I/O 都扫描全局 fd 列表。fanotify 继续复用现有的 mark interest 判断，但把判断提前到 stat/path 准备之前。

对测试的预期收益：

- iozone：主要改善小 record read/write/open/close 场景，减少文件通知的固定 syscall 开销。
- lmbench/libcbench：对 read/write/open/close 相关微基准有帮助。
- Lua：对脚本启动、模块加载、普通文件读取有轻微帮助。
- iperf：主要是网络测试，收益不明显，只会间接受益于少量 fd open/close 开销降低。

这项优化不改变 inotify/fanotify 的正常语义：一旦存在 watch/mark，仍然会走原来的事件匹配、权限请求和队列投递逻辑。它只是让“无人监听”的常见 benchmark 场景更快跳过通知子系统。
