# Linux ext4 buffered read 与 Moss 文件缺页阅读指南

本文用于对照 Moss 和 Linux 的文件缺页、页缓存、ext4 块映射与块设备 I/O 路径，并重点解析 Linux 6.12 中的 `filemap_get_pages()`。

本文假设两个仓库位于：

- Moss：`/home/wuxun/Projects/moss`
- Linux：`/home/wuxun/Projects/linux-grader-baseline`

## 1. 先掌握四个对象

### 1.1 inode

inode 代表一个具体文件的文件系统对象，保存文件大小、权限、extent/块映射等信息。多个进程打开同一个文件时，会共享同一个 inode。

### 1.2 address_space

这里的 `address_space` 不是进程虚拟地址空间，而是 Linux 中一个文件的页缓存容器，通常由 `inode->i_mapping` 指向。

它负责维护：

- 文件页号到 folio 的映射；
- `read_folio`、`readahead`、`writepages` 等回调；
- 页缓存与 truncate、hole-punch 之间的同步。

不同 inode 通常拥有不同的 `address_space`，因此不同文件的页缓存和主要同步对象相互独立。

### 1.3 folio

folio 是 Linux 页缓存和内存管理使用的数据对象。它可以表示一个 4 KiB 基础页，也可以表示多个连续基础页。

一个 folio 记录：

- 属于哪个 `address_space`；
- 对应文件中的页索引；
- 数据是否有效，即 `uptodate`；
- 是否正在读取或修改，即 folio lock；
- `dirty`、`writeback` 和引用计数等状态。

同一 folio 只允许一个加载者执行磁盘读取。访问同一 folio 的其他任务等待它完成，而访问其他 folio 的任务可以继续运行。

### 1.4 BIO

BIO 是文件系统提交给 Linux 块层的 I/O 描述对象。一个读 BIO 通常包含：

- 目标块设备；
- 起始磁盘扇区；
- 若干内存段或 folio；
- READ、RAHEAD 等操作标志；
- I/O 完成回调。

ext4 会尽量把磁盘上连续且属性兼容的 folio 加入同一个 BIO，块层还可能继续合并请求。

## 2. Moss 推荐阅读路径

### 2.1 文件缺页入口

先区分普通共享文件映射和 ELF/COW 文件映射：

- `kernel/src/mm/aspace/backend/file.rs`
  - `maybe_readahead_after_fault()`
  - `map()`及缺页处理相关实现
- `kernel/src/mm/aspace/backend/cow.rs`
  - 文件预读调用
  - 从文件填充私有/COW 页面

阅读时重点记录：

1. 进入缺页处理时持有哪些地址空间锁；
2. 哪些操作在分配物理页之前完成；
3. `read_at()`和 `readahead()`是否在锁内同步等待 I/O；
4. 邻接页何时安装 PTE。

### 2.2 axfs 页缓存

文件：`vendor/axfs/src/highlevel/file.rs`

重点阅读：

- `CachedFileShared`
- `with_page_or_insert_impl()`
- `read_at()`
- `readahead()`
- `readahead_pages()`
- `reclaim_page_cache_if_needed()`

需要回答：

- 缺失页是在读取前还是读取后插入页缓存；
- 正在读取的页是否具有可等待的 Loading/in-flight 状态；
- 同一页如何避免重复读取；
- 不同页是否因为哈希分片碰撞而互相等待；
- `page_cache`、`cache_op`、`page_load_locks`各自覆盖什么范围；
- 哪些锁覆盖了块设备等待。

### 2.3 axfs ext4 挂载级锁

文件：

- `vendor/axfs/src/fs/ext4/fs.rs`
- `vendor/axfs/src/fs/ext4/inode.rs`

关键调用：

```text
Inode::read_at()
  -> self.fs.lock()
  -> LwExt4Filesystem::read_at()
```

`Ext4Filesystem::inner`是一个 `Mutex<LwExt4Filesystem>`。它属于整个 ext4 挂载实例，而不是单个 inode。只要同步块读取仍在这把锁内等待，不同文件的读取就仍会串行。

### 2.4 lwext4 映射和同步块读取

文件：

- `third_party/lwext4_rust/src/fs.rs`
  - `Ext4Filesystem::read_at()`
- `third_party/lwext4_rust/src/inode/file.rs`
  - `InodeRef::read_at()`
  - `get_inode_fblock()`
  - `ext4_blocks_get_direct()`
- `third_party/lwext4_rust/src/blockdev.rs`
  - `dev_bread()`
  - `BlockDevice::read_blocks()`

当前调用把逻辑块映射和同步设备读取绑在一个可变 `Ext4Filesystem`调用中：

```text
获取 ext4 挂载级 Mutex
  -> 获取 inode 引用
  -> 逻辑块映射
  -> 合并连续物理块
  -> 同步 read_blocks
  -> 等待设备完成
释放 ext4 挂载级 Mutex
```

## 3. Linux 推荐阅读路径

### 3.1 buffered read 和页缓存

文件：`mm/filemap.c`

建议依次阅读：

1. `filemap_read()`：buffered read 外层循环和向用户缓冲区复制；
2. `filemap_get_pages()`：取得本轮可读 folio 批次；
3. `filemap_get_read_batch()`：从页缓存 XArray 获取连续 folio；
4. `filemap_update_page()`：等待或加载批次末尾尚未就绪的 folio；
5. `filemap_create_folio()`：缓存完全缺页时创建单个 folio；
6. `filemap_read_folio()`：调用文件系统并等待目标 folio 完成；
7. `filemap_add_folio()`和 `__filemap_add_folio()`：短暂持有 XArray 锁发布 folio。

另外需要阅读 mmap 缺页使用的 `filemap_fault()`，比较 `read(2)`与文件映射缺页最终如何汇合到同一个 page cache 和 `read_folio`/`readahead`接口。

### 3.2 通用预读

文件：`mm/readahead.c`

建议依次阅读：

1. `page_cache_sync_ra()`：首次缺页或同步读取触发预读；
2. `page_cache_async_ra()`：消费到预读标记后扩大并推进窗口；
3. `page_cache_ra_unbounded()`：预分配并发布一批已锁定 folio；
4. `read_pages()`：调用文件系统 `readahead`回调；
5. `blk_start_plug()`和 `blk_finish_plug()`：批量收集块请求，但不等待磁盘完成。

预读窗口不是固定 64 KiB。Linux 会根据顺序访问历史、请求大小、设备能力和拥塞状态调整窗口。

### 3.3 ext4 页缓存接口

文件：`fs/ext4/inode.c`

重点阅读：

- `ext4_read_folio()`
- `ext4_readahead()`
- `ext4_aops`
- `ext4_map_blocks()`

`ext4_aops`把通用 page cache 接口连接到 ext4。`ext4_map_blocks()`先查询 inode 的 extent status cache；缓存未命中时使用该 inode 的 `i_data_sem`读锁查询映射。

这里的 `i_data_sem`属于单个 inode，而且普通映射查询使用共享读锁。它不同于 Moss 的挂载级 `Mutex<LwExt4Filesystem>`。

### 3.4 ext4 批量读取和 BIO

文件：`fs/ext4/readpage.c`

重点阅读 `ext4_mpage_readpages()`：

1. 从 readahead control 取得已经锁定的 folio；
2. 尽量复用上一次 `ext4_map_blocks()`返回的长 extent；
3. 将磁盘连续的 folio 加入当前 BIO；
4. 遇到不连续块、extent 边界、BIO 容量或加密边界时提交当前 BIO；
5. `submit_bio()`提交后立即返回；
6. BIO 完成回调更新 folio 状态并解锁，唤醒只等待这些 folio 的任务。

### 3.5 块层和 virtio-blk

在理解文件系统路径后，再继续阅读：

- `include/linux/bio.h`
- `block/blk-core.c`
- `block/blk-mq.c`
- `drivers/block/virtio_blk.c`

此时重点是：BIO 如何变为 block request、请求如何排队、virtqueue 如何提交、完成中断如何回到 BIO end_io。

## 4. `filemap_get_pages()`的任务

该函数不是把整个读取请求一次复制给用户。它只负责为 `filemap_read()`准备“本轮能够安全读取的一批连续 folio”。

函数的输入和输出是：

- `iocb`：当前文件、读取位置和 NOWAIT/NOIO 等标志；
- `count`：本轮最多希望覆盖的字节数；
- `fbatch`：返回的 folio 引用批次；
- `need_uptodate`：调用者是否要求整个 folio 必须完全有效。

成功返回 0 时，调用者遍历 `fbatch`，把 folio 数据复制给用户。返回负数时，表示当前没有能够先交给调用者消费的 folio。

## 5. 变量含义

```c
struct file *filp = iocb->ki_filp;
struct address_space *mapping = filp->f_mapping;
struct file_ra_state *ra = &filp->f_ra;
pgoff_t index = iocb->ki_pos >> PAGE_SHIFT;
```

- `filp`：当前打开文件对象；
- `mapping`：该 inode 的文件页缓存；
- `ra`：这个打开文件流的预读历史和窗口；
- `index`：当前读取偏移对应的基础页号。

```c
last_index = DIV_ROUND_UP(iocb->ki_pos + count, PAGE_SIZE);
```

`last_index`是请求结束位置之后的第一个页号，所以查询页缓存时使用 `last_index - 1`作为包含式上界。

## 6. 第一次查询页缓存

```c
filemap_get_read_batch(mapping, index, last_index - 1, fbatch);
```

`filemap_get_read_batch()`从该文件的 XArray 页缓存取得连续 folio。它有一个重要约束：

- 前面的 folio 都是 `uptodate`且没有 readahead 标志；
- 遇到第一个未 `uptodate`或带 readahead 标志的 folio 后，将它作为批次最后一个元素并停止。

因此，`filemap_get_pages()`只需要特殊检查批次最后一个 folio。前面的 folio 已经可以直接复制。

## 7. 缓存完全未命中时触发同步预读

```c
if (!folio_batch_count(fbatch)) {
    ...
    page_cache_sync_readahead(...);
    ...
    filemap_get_read_batch(...);
}
```

第一次查询没有找到任何 folio 时：

1. `IOCB_NOIO`表示本次调用禁止发起 I/O，因此直接返回 `-EAGAIN`；
2. 否则根据当前 `file_ra_state`触发同步预读；
3. 预读会先把一批处于 locked/loading 状态的 folio 插入页缓存，再调用 ext4 提交读取；
4. 随后重新查询页缓存，取得刚刚发布的 folio。

这里的“同步预读”表示它由当前同步读取路径触发，不表示所有预读 I/O 都在函数内同步等完。

`IOCB_NOWAIT`路径使用 `memalloc_noio_save()`，避免预读过程中的内存分配因为回收而递归触发可能阻塞的 I/O。

## 8. 预读仍没有产生 folio 时的兜底

```c
if (!folio_batch_count(fbatch)) {
    ...
    err = filemap_create_folio(...);
    if (err == AOP_TRUNCATED_PAGE)
        goto retry;
    return err;
}
```

预读可能被禁用、分配失败、没有覆盖目标页，或者与其他线程发生竞争。此时创建并读取当前真正需要的单个 folio。

如果返回 `AOP_TRUNCATED_PAGE`，说明刚才观察到的页缓存状态已经失效，需要重新查询，而不是把它作为用户可见错误返回。

## 9. 处理批次最后一个 folio

```c
folio = fbatch->folios[folio_batch_count(fbatch) - 1];
```

由于 `filemap_get_read_batch()`的停止规则，只有最后一个 folio 可能：

- 带有 readahead 触发标志；
- 尚未 `uptodate`；
- 正在由另一个线程读取。

### 9.1 推进异步预读窗口

```c
if (folio_test_readahead(folio))
    filemap_readahead(...);
```

当读取流消费到预读窗口中的触发 folio 时，Linux提前提交下一段异步预读，避免当前窗口完全耗尽后才开始下一次磁盘读取。

### 9.2 确保数据有效

```c
if (!folio_test_uptodate(folio))
    filemap_update_page(...);
```

`filemap_update_page()`会：

- 获取单文件 `invalidate_lock`共享锁；
- 尝试锁住目标 folio；
- 如果另一个任务已经在加载它，只等待该 folio；
- 必要时调用文件系统 `read_folio()`；
- 读取完成后确认 folio 仍属于原来的 mapping。

它不会让读取文件 A 的任务因为文件 B 的 I/O 而等待。

## 10. 为什么存在 `retry`

`retry`处理的是页缓存并发竞争，不是普通磁盘错误。

在以下过程之间，页缓存可能变化：

```text
查询 XArray
  -> 等待 folio lock
  -> truncate/hole-punch/invalidate
  -> folio 被移除或替换
```

也可能是两个任务同时发现缓存缺页：

```text
任务 A：分配 folio
任务 B：分配 folio
任务 A：先插入 XArray
任务 B：插入得到 -EEXIST
```

这些情况会转换为内部状态 `AOP_TRUNCATED_PAGE`。这个名字有历史原因，它不只表示真的执行了 truncate，更准确的含义是：

> 先前取得的 folio 或缺页判断已经不再可靠，请从页缓存重新查找。

因此代码跳到 `retry`，重新执行：

- 信号检查；
- XArray 查询；
- 必要的预读或单页创建。

不能继续使用旧 folio 指针，因为它可能已经不属于该文件，或者另一个任务已经发布了更新的 folio。

## 11. 为什么存在 `err`

`err`处理“批次最后一个 folio 失败”的情况，同时尽量保留前面已经可读的 folio。

```c
err:
    if (err < 0)
        folio_put(folio);
    if (likely(--fbatch->nr))
        return 0;
    if (err == AOP_TRUNCATED_PAGE)
        goto retry;
    return err;
```

逐句解释如下。

### 11.1 `if (err < 0) folio_put(folio)`

普通负 errno 表示最后一个 folio 不能返回给调用者，需要释放本函数为它持有的引用。

`AOP_TRUNCATED_PAGE`是内部重试状态，不按普通负 errno 处理；相关 helper 按该返回约定处理目标 folio 的引用，外层不能重复 `folio_put()`。

### 11.2 `if (likely(--fbatch->nr)) return 0`

`--fbatch->nr`把失败的最后一个 folio 从批次有效范围中删除。

如果批次原来是：

```text
[folio 10: Ready]
[folio 11: Ready]
[folio 12: 失败]
```

删除最后一个后仍有：

```text
[folio 10: Ready]
[folio 11: Ready]
```

函数此时返回 0，让 `filemap_read()`先把前两个 folio 复制给用户。这样可以：

- 避免因为后面的页失败而丢弃前面已经准备好的数据；
- 符合 read 的短读语义；
- 缩短已经可用数据的等待时间；
- 让下一轮读取再处理失败位置。

这也是为什么 `err`标签不等于“立即返回错误”。

### 11.3 批次已经为空时

如果失败 folio 是批次中唯一的元素：

- `AOP_TRUNCATED_PAGE`跳回 `retry`重新查询；
- 普通错误直接返回给 `filemap_read()`；
- `filemap_read()`只有在尚未复制任何字节时才向用户返回负错误，否则返回已经读取的字节数。

## 12. 函数控制流总结

```text
根据文件偏移计算页范围
        |
从 inode address_space 查询连续 folio 批次
        |
        +-- 完全未命中 --> 同步触发 readahead --> 再查一次
        |                                      |
        |                                      +-- 仍为空 --> 创建目标 folio
        |
批次非空
        |
只检查最后一个 folio
        |
        +-- 有 readahead 标志 --> 提交下一窗口异步预读
        |
        +-- 非 uptodate --> 等待已有 I/O 或发起 read_folio
                              |
                              +-- 状态已失效 --> retry
                              |
                              +-- 普通错误 --> 去掉最后一个 folio
                                                  |
                                                  +-- 前面还有 Ready folio --> 返回成功
                                                  +-- 批次为空 --> 返回错误
```

## 13. 为什么合并 BIO 后异步提交

### 13.1 合并 BIO

假设读取 16 个连续的 4 KiB folio，不合并可能产生 16 次：

- virtqueue 描述符提交；
- guest 到 QEMU 的通知；
- 块层排队和锁操作；
- 完成中断；
- 完成回调与任务唤醒。

合并为一个或少量 64 KiB BIO 后，可以显著减少固定开销，并复用一次长 extent 映射。

### 13.2 异步提交

异步 `submit_bio()`只把请求交给块层，不等待设备完成。folio 继续保持 locked/loading 状态，完成回调负责把它标记为 `uptodate`并解锁。

这样能够形成多个在途请求：

```text
CPU 0：映射文件 A，提交 BIO A
CPU 1：映射文件 B，提交 BIO B
CPU 2：映射文件 C，提交 BIO C

设备/virtio 队列同时拥有 A、B、C 三组请求
```

收益包括：

- 文件系统映射锁不覆盖设备等待；
- 不同 inode 和不同 folio 可以继续工作；
- 块设备获得足够队列深度进行合并和调度；
- CPU 计算可以与磁盘 I/O 重叠；
- 只有实际消费缺失 folio 的任务等待该 folio。

不能无限构造一个巨大 BIO。Linux 会受到物理块连续性、BIO 段数、设备限制、extent 边界、加密上下文和预读窗口约束。

## 14. Moss 与 Linux 当前结构差异

Moss 当前大致是：

```text
发现页缓存缺失
  -> 获取若干 page-load 分片锁
  -> 获取 ext4 挂载级锁
  -> 映射并同步读取 64 KiB 到临时 Vec
  -> 释放 ext4 锁
  -> 创建 PageCache 并发布
```

Linux 大致是：

```text
发现页缓存缺失
  -> 创建 locked/loading folio 并先发布到 inode address_space
  -> 在 inode 级共享锁下映射 extent
  -> 把多个 folio 加入 BIO
  -> 异步提交 BIO
  -> 释放映射锁并返回
  -> I/O 完成回调更新并解锁 folio
```

因此 Moss 后续优化的重点不是继续增大固定读取批次，而是补全：

1. 页缓存中的单页 Loading/Ready/Error 状态；
2. 同一 folio 的等待队列；
3. 块映射与块设备读取分离；
4. inode 级映射失效读写锁；
5. 不持有 ext4 挂载级锁的块设备读取；
6. BIO 或等价的批量异步块请求及完成回调。

## 15. `filemap_get_pages()`逐句精读

下面按照源代码顺序解释每个声明、函数和标志。源码位于 Linux 仓库的 `mm/filemap.c`。

### 15.1 函数名和参数

```c
static int filemap_get_pages(struct kiocb *iocb, size_t count,
        struct folio_batch *fbatch, bool need_uptodate)
```

#### `static`

这里表示函数只在当前 C 文件 `mm/filemap.c`内部可见，不导出为其他编译单元可直接调用的全局符号。

#### `int`

返回的是状态值，不是读取字节数：

- `0`：`fbatch`中至少有一批可以交给调用者处理的 folio；
- 负 errno：没有可以先消费的数据，并且发生错误或当前模式不允许等待；
- `AOP_TRUNCATED_PAGE`只在内部 helper 之间传播，本函数会尽量将它转换成重试，不应直接返回给用户态。

#### `filemap_get_pages`

- `filemap`：Linux 通用文件页缓存层，即 file mapping/address_space；
- `get`：取得 folio 引用，而不是复制文件数据；
- `pages`：历史命名。当前实现实际取得的是 folio，函数名保留了 page cache 的传统叫法。

该函数可以理解为“从文件页缓存取得本轮读取所需的一批页面对象”。

#### `struct kiocb *iocb`

`kiocb`是 kernel I/O control block，即内核 I/O 控制块：

- `k`：kernel，用来和用户接口中的 I/O 控制块区分；
- `iocb`：I/O control block；结构体字段统一使用 `ki_`前缀，表示 kernel I/O 字段；
- `ki_filp`：本次操作的 `struct file`；
- `ki_pos`：当前文件偏移；
- `ki_flags`：本次 I/O 的行为标志；
- `ki_waitq`：异步 buffered read 使用的页等待队列。

一个 `kiocb`描述一次正在进行的读写操作，而不是一个磁盘请求。磁盘请求要到后面的 BIO/块层才形成。

#### `size_t count`

从 `ki_pos`开始，本轮最多希望覆盖的字节数。`size_t`是用于表示内存大小和字节计数的无符号类型。

#### `struct folio_batch *fbatch`

- `f`：folio；
- `batch`：批次。

它内部包含一个 `folios[]`指针数组和当前元素数量 `nr`。使用批次可以摊薄逐页查询、引用计数和函数调用开销。Linux 6.12 中默认最多保存 `PAGEVEC_SIZE`，即 31 个 folio。

调用者在进入该函数前已经初始化 `fbatch`。本函数把持有引用的 folio 放进去，调用者使用完后负责 `folio_put()`。

#### `bool need_uptodate`

表示调用者是否要求整个 folio 必须是完整 `uptodate`状态：

- `true`：整个 folio 必须有效，例如 `splice`无法安全处理只部分有效的 folio；
- `false`：如果文件系统支持 `is_partially_uptodate`，只要当前请求覆盖的那段有效即可，普通 `read`使用该模式。

`uptodate`不是“时间最新”，而是页缓存术语，表示内存里的数据已经成功从文件/设备填充，可以使用。

### 15.2 局部变量

```c
struct file *filp = iocb->ki_filp;
```

- `file`：一次打开文件的内核对象；
- `filp`：file pointer 的传统缩写，用来避免变量名 `file`与类型名、其他局部概念混淆；
- `ki_filp`：`kiocb`中保存的目标打开文件。

`struct file`代表一次 open 实例，因此其中的当前预读状态可以与另一次独立 open 不同。

```c
struct address_space *mapping = filp->f_mapping;
```

- `f_mapping`：该打开文件对应的文件内容映射；
- `mapping`：从文件页号到 folio 的映射，也包含文件系统 page-cache 回调；
- 它通常就是 `inode->i_mapping`。

它不是进程页表。不同进程读取同一 inode 时会共享这个文件页缓存 mapping。

```c
struct file_ra_state *ra = &filp->f_ra;
```

- `ra`：readahead；
- `f_ra`：这个打开文件流的预读状态；
- 它记录前一次读取位置、当前预读窗口起点、大小和异步部分大小。

Linux把预读历史放在 `struct file`中，而页缓存放在 inode mapping 中。因此两个进程可以共享缓存，但仍拥有各自的顺序读取历史。

```c
pgoff_t index = iocb->ki_pos >> PAGE_SHIFT;
```

- `pgoff_t`：page offset type，用于表示以基础页为单位的文件偏移；
- `index`：当前读取位置对应的页索引；
- `ki_pos`：以字节为单位的文件偏移；
- `PAGE_SHIFT`：基础页大小的二进制位数，4 KiB 页通常为 12；
- 右移 12 位等价于除以 4096，并向下取整。

例如 `ki_pos = 9000`时，`index = 2`，因为偏移位于文件的第三个 4 KiB 页。

```c
pgoff_t last_index;
```

声明读取范围结束后的第一个页索引，即 exclusive end。使用 exclusive end 便于用差值计算页数。

```c
struct folio *folio;
```

后面指向批次最后一个 folio。根据 `filemap_get_read_batch()`的约定，只有最后一个 folio 可能需要触发预读或等待加载。

```c
unsigned int flags;
```

这是一个局部保存变量，只用于保存 `memalloc_noio_save()`返回的旧任务标志，之后传给 `memalloc_noio_restore()`。

它不是 `iocb->ki_flags`。更直观的名字可以是 `saved_flags`，但 Linux 在 save/restore 代码中经常简写为 `flags`。

```c
int err = 0;
```

- `err`：error/status；
- 初始为成功；
- 后面同时承载普通负 errno、0 和正的 `AOP_TRUNCATED_PAGE`内部状态。

### 15.3 计算读取结束页

```c
/* "last_index" is the index of the page beyond the end of the read */
last_index = DIV_ROUND_UP(iocb->ki_pos + count, PAGE_SIZE);
```

- `iocb->ki_pos + count`：本轮请求的字节结束位置；
- `DIV_ROUND_UP(n, d)`：除法向上取整，即 `(n + d - 1) / d`；
- `PAGE_SIZE`：基础页字节数；
- `last_index`：结束位置之后的第一页。

必须向上取整，因为哪怕请求只进入下一页 1 字节，也需要覆盖那个页。

例如从偏移 4090 读取 20 字节：

```text
起始页 index      = 0
结束字节位置      = 4110
last_index        = ceil(4110 / 4096) = 2
需要页范围        = [0, 2)，即页 0 和页 1
```

### 15.4 `retry`与致命信号

```c
retry:
```

C 标签，供 `goto retry`跳回。它不是循环语法，但形成了一个只在页缓存状态过期时重新执行的重试循环。

```c
if (fatal_signal_pending(current))
    return -EINTR;
```

- `current`：当前正在执行本函数的任务；
- `fatal_signal_pending()`：检查当前任务是否有致命信号待处理；该版本实现最终检查待处理 `SIGKILL`；
- `pending`：信号已经送达但尚未完成处理；
- `-EINTR`：interrupted system call，表示操作被信号中断。

之所以在 `retry`后检查，是为了避免 truncate、竞争或异常介质导致的多次重试让一个已经要退出的任务继续消耗 CPU。

### 15.5 查询页缓存批次

```c
filemap_get_read_batch(mapping, index, last_index - 1, fbatch);
```

函数名含义：

- `filemap`：文件页缓存层；
- `get`：取得并增加 folio 引用；
- `read_batch`：为读取取得一个连续批次。

参数含义：

- `mapping`：查询哪个文件的页缓存；
- `index`：包含式起点；
- `last_index - 1`：包含式终点；
- `fbatch`：保存结果。

这里减 1 是因为 `last_index`是 exclusive end，而 `filemap_get_read_batch()`的 `max`参数是 inclusive end。

该 helper 在 RCU 读侧临界区扫描 mapping 的 XArray，并对每个返回 folio 执行安全的引用获取。它保证：

1. 返回的是文件中连续的 folio；
2. 前面的 folio 都是 `uptodate`且没有 readahead 标志；
3. 第一个非 `uptodate`或带 readahead 标志的 folio可以成为最后一个元素；
4. 遇到这个特殊 folio后停止扫描。

### 15.6 第一次完全未命中

```c
if (!folio_batch_count(fbatch)) {
```

- `folio_batch_count()`返回 `fbatch->nr`；
- `!`是逻辑非；
- 数量为 0 时，`!0`为真。

因此这句表示页缓存中没有找到任何可返回 folio。

```c
if (iocb->ki_flags & IOCB_NOIO)
    return -EAGAIN;
```

- `ki_flags`是位掩码，一个整数中的不同 bit 表示不同 I/O 行为；
- `&`是按位与，用于检测目标 bit 是否设置；
- `IOCB_NOIO`：本次读取不允许产生任何新的 I/O，包括预读；
- `-EAGAIN`：当前无法在约束下完成，调用者以后可以重试。

如果目标页已经在缓存，`NOIO`读取仍然可以成功；只有缓存未命中、必须发 I/O 时才返回 `-EAGAIN`。

```c
if (iocb->ki_flags & IOCB_NOWAIT)
    flags = memalloc_noio_save();
```

- `IOCB_NOWAIT`：不能等待已有 I/O 完成，但不禁止启动 readahead；
- `memalloc_noio_save()`：给当前任务临时设置 `PF_MEMALLOC_NOIO`，使后续内存分配隐式去掉 `__GFP_IO`；
- 返回旧状态保存在局部 `flags`。

这里防止预读分配 folio 时触发内存回收，而回收又同步发起写回 I/O，从而违反 NOWAIT 路径不应被隐式 I/O 卡住的要求。它只约束内存分配的回收行为，并不等价于 `IOCB_NOIO`，也不禁止显式提交预读。

```c
page_cache_sync_readahead(mapping, ra, filp, index,
        last_index - index);
```

函数名含义：

- `page_cache`：通用文件页缓存；
- `sync`：由当前同步缺页/读取直接触发，不表示等待所有设备 I/O 完成；
- `readahead`：在实际需求之前读取后续数据。

参数：

- `mapping`：目标文件页缓存；
- `ra`：本打开文件的预读历史；
- `filp`：目标打开文件；
- `index`：预读起点；
- `last_index - index`：当前请求至少涉及的基础页数。

它根据访问历史计算预读窗口，预分配并发布 locked folio，然后调用文件系统的 `readahead`回调提交 I/O。

```c
if (iocb->ki_flags & IOCB_NOWAIT)
    memalloc_noio_restore(flags);
```

恢复进入预读前的任务内存分配状态。save/restore 必须严格配对，不能让当前任务后续所有内存分配永久处于 NOIO 模式。

```c
filemap_get_read_batch(mapping, index, last_index - 1, fbatch);
```

预读后重新查询页缓存。即使设备 I/O还没有完成，预读也应该已经把 locked/loading folio发布到 mapping，因此通常可以取得这些 folio并在后面决定等待、异步排队或返回 `-EAGAIN`。

### 15.7 预读后仍然没有 folio

```c
if (!folio_batch_count(fbatch)) {
```

第二次为空表示预读没有发布目标 folio，可能原因包括预读被限制、内存分配失败、窗口没有覆盖目标位置或并发失效。

```c
if (iocb->ki_flags & (IOCB_NOWAIT | IOCB_WAITQ))
    return -EAGAIN;
```

- `|`是按位或，构造“NOWAIT 或 WAITQ 任意一个已设置”的掩码；
- 再与 `ki_flags`按位与，结果非零表示至少命中一个标志；
- `IOCB_WAITQ`表示 `iocb->ki_waitq`有效，本次异步 buffered read允许把等待挂入指定页等待队列；
- 这里不调用可能同步阻塞的 `filemap_create_folio()`，而是返回 `-EAGAIN`交给异步/非阻塞上层处理。

```c
err = filemap_create_folio(filp, mapping, iocb->ki_pos, fbatch);
```

这是同步读取的兜底路径。`filemap_create_folio()`会：

1. 按 mapping 支持的最小 folio order 分配内存；
2. 获取该 mapping 的 `invalidate_lock`共享锁；
3. 把 locked folio 插入页缓存；
4. 调用 `mapping->a_ops->read_folio`；
5. 等待 folio 读取完成；
6. 成功后加入 `fbatch`。

```c
if (err == AOP_TRUNCATED_PAGE)
    goto retry;
```

- `AOP`：address_space operation；
- `TRUNCATED_PAGE`：历史名称，表示交给 address-space 回调的页已被解锁并可能遭遇失效；
- 值为正数 `0x80001`，不是 errno；
- `==`检查状态严格相等；
- `goto retry`重新查询页缓存。

这里不仅可能是 truncate。两个线程同时插入同一页时，后插入者得到 `-EEXIST`，`filemap_create_folio()`也把它转换成 `AOP_TRUNCATED_PAGE`，含义是“页缓存事实已变化，请重新查”。

```c
return err;
```

如果成功，`err == 0`且新 folio 已经在 `fbatch`中；如果是普通错误则返回负 errno。由于前面确认批次为空，这里没有可先交付的数据。

### 15.8 选择批次最后一个 folio

```c
folio = fbatch->folios[folio_batch_count(fbatch) - 1];
```

- C 数组从 0 开始；
- 元素数量为 `nr`时，最后一个下标是 `nr - 1`；
- `filemap_get_read_batch()`保证只有最后一个元素可能需要特殊处理。

### 15.9 消费预读标记时推进窗口

```c
if (folio_test_readahead(folio)) {
```

- `folio_test_*`是读取 folio 状态位的命名模式；
- `readahead`标志把这个 folio标记为当前预读窗口的推进触发点；
- 读到它说明旧窗口快被消费完，应提前提交下一窗口。

```c
err = filemap_readahead(iocb, filp, mapping, folio, last_index);
```

这个 helper 创建 `readahead_control`，以当前 folio 为起点调用 `page_cache_async_ra()`。这里的 `async`表示预取下一窗口后立即返回，不等待这些页全部完成。

如果设置了 `IOCB_NOIO`，该 helper 返回 `-EAGAIN`，因为推进窗口会产生新 I/O。

```c
if (err)
    goto err;
```

C 中 0 表示成功，非 0 表示某种错误或特殊状态。这里不能把最后一个 folio继续交给调用者，进入统一的批次尾部处理。

### 15.10 确保最后一个 folio 数据有效

```c
if (!folio_test_uptodate(folio)) {
```

如果 `uptodate`未设置，说明这个 folio尚未完成成功读取，不能直接复制其中的数据。

```c
if ((iocb->ki_flags & IOCB_WAITQ) &&
    folio_batch_count(fbatch) > 1)
    iocb->ki_flags |= IOCB_NOWAIT;
```

逐部分解释：

- 第一对括号：检查本次操作支持异步页等待队列；
- `&&`：逻辑与，左右条件必须都成立；
- `folio_batch_count(fbatch) > 1`：最后一个 folio之前已经有至少一个 Ready folio；
- `|=`：把 `IOCB_NOWAIT`位设置到现有 `ki_flags`中，不清除其他位。

为什么这样做：如果前面已经有可返回数据，就不应为了最后一个 Loading folio把整个异步请求挂起。设置 NOWAIT 后，下面的 helper对最后一页返回 `-EAGAIN`，`err`分支将最后一页移出批次，然后调用者立即消费前面的 Ready 前缀。

这也避免异步 buffered read在已经可以取得部分数据时返回“整个请求已排队等待”的状态。

```c
err = filemap_update_page(iocb, mapping, count, folio,
        need_uptodate);
```

函数名含义：

- `filemap`：文件页缓存层；
- `update`：让已有但尚未有效的 folio变为满足读取要求的状态；
- `page`：历史命名，参数实际是 folio。

它执行：

1. 获取单 mapping 的 `invalidate_lock`共享锁；NOWAIT 模式只 trylock；
2. 尝试获得 folio lock；
3. 如果别的任务正在加载：同步模式等待该 folio，WAITQ 模式注册异步等待，NOWAIT/NOIO 返回 `-EAGAIN`；
4. 重新确认 folio仍属于 mapping；
5. 检查整个 folio或请求范围是否已经有效；
6. 必要且允许时调用文件系统 `read_folio()`；
7. 处理 truncate/invalidate 竞争并释放锁。

`count`和 `need_uptodate`用于判断请求涉及的范围是否已经足够有效，不一定要求加载 folio中与本次读取无关的部分。

```c
if (err)
    goto err;
```

任何非零结果都不能把最后一个 folio作为成功元素返回：

- 负数：普通错误、NOWAIT 下的 `-EAGAIN`等；
- `AOP_TRUNCATED_PAGE`：缓存状态变化，需要重查。

### 15.11 成功路径

```c
trace_mm_filemap_get_pages(mapping, index, last_index - 1);
```

这是 tracepoint：

- `trace`：记录内核跟踪事件；
- `mm`：memory management 子系统；
- `filemap_get_pages`：事件名称。

它用于 perf/ftrace/BPF 等观测工具，不改变正常读取语义。记录 mapping 和页范围可以分析页缓存命中、批次和读取行为。

```c
return 0;
```

表示 `fbatch`中保留的 folio可以交给 `filemap_read()`或 `filemap_splice_read()`继续处理。它不表示读取了 0 字节；实际复制字节数由外层函数统计。

### 15.12 `err`标签与尾部回退

```c
err:
```

统一处理“批次最后一个 folio不能使用”的路径。设计目标是保留批次前面已经 Ready 的 folio。

```c
if (err < 0)
    folio_put(folio);
```

- `err < 0`：普通 errno；
- `folio_put()`：减少本函数持有的 folio引用计数；引用归零后该对象才可能释放；
- 失败的最后一个 folio不会返回给调用者，因此必须释放它的引用。

`AOP_TRUNCATED_PAGE`是正值，不进入这里。在 `filemap_update_page()`返回该状态的路径中，helper已经按照契约执行了对应的 `folio_put()`；外层再次 put 会导致引用计数错误。

```c
if (likely(--fbatch->nr))
    return 0;
```

逐部分解释：

- `--fbatch->nr`：前置自减，先将元素数量减 1，再判断结果；这在逻辑上移除了失败的最后一个 folio；
- 不需要清空数组槽，因为之后的遍历只读取下标 `[0, nr)`；
- `likely(x)`：分支预测提示，告诉编译器这个条件通常为真；不改变 C 语义；
- 结果非零：删除最后一个后，批次前面仍有 Ready folio；
- `return 0`：让调用者先消费这个 Ready 前缀。

例如：

```text
原批次 nr=3：Ready A、Ready B、Error C
--nr 后 nr=2：调用者只会看到 A、B
```

```c
if (err == AOP_TRUNCATED_PAGE)
    goto retry;
```

只有当失败 folio是批次唯一元素时，移除后批次才为空。此时如果属于页缓存状态变化，就重新查询；如果前面还有 Ready folio，则上一句已经返回，外层复制后下一轮自然会重新查询失败位置。

```c
return err;
```

批次已经为空且不是可重试的内部状态，只能把普通错误返回给上层。外层 `filemap_read()`遵循短读规则：如果此前已经复制过字节，优先返回字节数；只有一个字节都没复制时才把负错误返回用户态。

## 16. 三种 I/O flags 的区别

`iocb->ki_flags`是位掩码。每个 bit 独立表达一个行为约束，可以同时设置多个标志。

| 标志 | 名称含义 | 可以使用缓存数据 | 可以提交新预读 | 可以等待 I/O | 主要用途 |
| --- | --- | --- | --- | --- | --- |
| `IOCB_NOWAIT` | no wait | 是 | 是 | 否 | 当前不能阻塞；缺失数据时返回 `-EAGAIN`，但允许为未来启动预读 |
| `IOCB_NOIO` | no I/O | 是 | 否 | 否 | 本次不能产生任何新 I/O；只消费已经在缓存且可用的数据 |
| `IOCB_WAITQ` | wait queue valid | 是 | 是 | 通过 `ki_waitq`异步等待 | buffered async read，允许把完成等待登记到页等待队列 |

常见位操作：

```c
flags & IOCB_NOIO
```

测试 NOIO 位是否存在。

```c
flags & (IOCB_NOWAIT | IOCB_WAITQ)
```

测试 NOWAIT、WAITQ 两位中是否至少有一位存在。

```c
flags |= IOCB_NOWAIT
```

在保留原有所有标志的同时设置 NOWAIT 位。

局部变量 `unsigned int flags`与上述位掩码不同。它只是 `memalloc_noio_save()`和 `memalloc_noio_restore()`之间传递的旧任务状态，作用域只有一次预读调用。
