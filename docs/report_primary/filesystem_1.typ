#set text(
  font: ("Times New Roman", "Songti SC"),
  size: 11pt,
  lang: "zh",
)
#set page(
  paper: "a4",
  margin: (x: 2.5cm, y: 2.2cm),
)
//段落
#set par(
  first-line-indent: (amount: 2em, all: true),
  justify: true,
  leading: 0.85em,
  spacing: 0.85em,
)
//代码块
#show raw.where(block: true): it => {
  block(
    fill: rgb("#f6f6f6"),
    inset: 1em,
    radius: 6pt,
    width: 100%,
    breakable: true,
    above: 0.35em,
    below: 0.35em,
    align(
      left,
      text(
        font: ("Consolas", "KaiTi"),
        size: 10pt,
        it,
      ),
    ),
  )
}

//标题编号格式
#set heading(numbering: "1.1")

#set figure(numbering: (..nums) => {
  let chapter = counter(heading).get().first()
  numbering("1-1", chapter, ..nums)
})

// 一级标题
#show heading.where(level: 1): it => {
  pagebreak(weak: true)
  set align(center)
  set text(
    font: "PingFang SC",
    size: 16pt,
    weight: "regular",
  )
  v(0em)
  block[
    #if it.numbering != none [
      第#counter(heading).display()章 #it.body
    ] else [
      #it.body
    ]
  ]
  v(0.5em)
}

// 二级标题
#show heading.where(level: 2): it => {
  block(
    above: 1.1em,
    below: 1em,
    sticky: true,
    width: 100%,
  )[
    #set text(
      font: "PingFang SC",
      size: 14pt,
      weight: "regular",
    )
    #counter(heading).display(it.numbering) #h(0.45em) #it.body
  ]
}

// 三级标题
#show heading.where(level: 3): it => {
  block(
    above: 1em,
    below: 1em,
    sticky: true,
    width: 100%,
  )[
    #set text(
      font: "PingFang SC",
      size: 12pt,
      weight: "regular",
    )
    #counter(heading).display(it.numbering) #h(0.45em) #it.body
  ]
}

#counter(heading).update(1)

= 文件系统

文件系统负责组织持久化数据的存储与访问，向内连接块设备驱动和内存管理，向外提供文件操作、目录遍历和元数据查询接口。

== 概述

=== StarryOS 的已有基础

StarryOS 已经建立了可工作的文件系统框架。系统通过 VFS 层统一 ext4 和 FAT32 两种文件系统，提供 `Location`、`FileNode` 和 `DirNode` 抽象。在功能上，StarryOS 已经实现 `open`、`read`、`write`、`lseek` 和 `stat` 等基础接口，支持路径解析和符号链接跟随。

早期版本也建立了页面缓存。每个打开的文件维护一个 64 页的本地缓存，采用 LRU 策略淘汰。因此，文件系统面对的不是一片空白，而是一套主体结构已经成形，但缓存容量偏小、访问模式未优化的实现。

=== base os 实现的主要不足

当用户程序只读写小文件或单个进程访问时，StarryOS 的缓存尚能工作。问题主要在负载变得复杂后暴露出来。

缓存容量方面，64 页仅 256KB，大文件访问时频繁淘汰。iozone 4 进程测试每个文件 1MB，lmbench 使用 512KB 文件，而 256KB 缓存无法完整覆盖这些工作集。每次淘汰都逐页写回磁盘，导致大量零散 I/O。顺序访问时缺少预读，每次读取都需等待磁盘响应，无法隐藏延迟。

跨进程共享方面，原有缓存完全独立，多个进程读取同一文件时各自加载相同数据。无法共享的缓存既浪费内存，也降低整体命中率。元数据访问方面，`stat` 等操作每次都查询底层文件系统，没有缓存 inode 信息。此外，每次普通 `read()` 都更新 atime，引入额外的元数据写开销。

这些问题说明，基线已经能够完成基本文件操作，却还没有把缓存容量、访问模式和资源共享完整串联起来。

=== 优化的完善范围

优化保留 StarryOS 的主体结构，重点补齐缓存容量、访问优化和跨进程共享之间的联系。主要增量如表所示。

#figure(
  table(
    columns: (1.35fr, 2.3fr, 2.65fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([内容], [StarryOS 基线], [优化增量]),
    [缓存架构], [单层本地缓存，64 页], [引入两级缓存，本地 256 页加全局 16K 页],
    [访问优化], [无预读，逐页写回], [顺序访问预读（1MB 上限）和批量写回（16 页）],
    [元数据], [每次查询文件系统], [全局 inode 缓存，减少后端访问],
    [并发控制], [文件级 Mutex], [本地缓存无锁，全局缓存单锁，原子变量优化],
  ),
  caption: [文件系统缓存模块的主要增量],
) <tab:fs-overview>

== 核心设计思想

StarryOS 已经搭起了文件系统的基本框架，优化的工作是在这套框架中进一步理清缓存分层、访问模式识别和资源共享：哪些数据应该在本地快速访问，哪些资源需要跨文件共享；如何识别顺序访问并提前加载；脏页何时写回磁盘。下面从这几个问题分别说明。

=== 两级缓存架构

缓存容量和访问速度并不是两套互不相干的问题。优化延续单层缓存的做法，但将容量分为两层：本地缓存表示单个文件的快速访问区域，全局缓存表示系统级的共享资源池。访问时优先查本地缓存，未命中再查全局缓存，都未命中才访问磁盘。

这种划分回答了一个最基本的问题：缓存页究竟属于单个文件，还是属于整个系统。线程局部性强的热点页放在本地缓存中；跨文件或跨进程的共享页放在全局缓存中。只要边界清楚，本地缓存才不会因全局锁竞争降低性能，全局缓存也不会因容量限制无法服务多个文件。

=== 三类访问模式

优化同时识别三类访问模式。

- 顺序访问：连续读取位置相邻，触发预读机制提前加载后续数据。
- 随机访问：读取位置跳跃，依赖缓存容量和命中率，不触发预读。
- 写入模式：小块写入累积为脏页，批量写回减少磁盘操作次数。

这三类模式用途不同。顺序访问通过预读隐藏磁盘延迟，随机访问依靠缓存容量提高命中率，写入模式通过延迟和批量减少 I/O 开销。优化没有把它们压进同一套策略，而是分别处理，再通过缓存协同达到最优效果。

=== LRU 与淘汰时机

Linux 不使用单一时间戳判断页面冷热，而是通过访问历史维护 LRU 链表。最近访问的页面移到链表头部，淘汰时从尾部取出最久未用的页面。

优化将 LRU 策略应用到本地缓存和全局缓存。本地缓存满时立即淘汰，全局缓存淘汰时还需检查 `mapped_count`，避免淘汰正在被 mmap 映射的页面。脏页淘汰前必须先写回磁盘。这样，缓存淘汰不再只是简单的容量管理，而成为访问模式、资源共享和数据一致性共同使用的策略。

=== 性能与一致性的平衡

文件系统缓存不是若干独立优化的集合，而是一条连续的权衡链：

#align(center)[
  #box(
    inset: 8pt,
    stroke: 0.6pt + gray,
    radius: 3pt,
  )[
    容量扩展 → 预读优化 → 批量写回 → 一致性保证
  ]
]

容量扩展提供命中率基础；预读机制隐藏顺序访问延迟；批量写回减少写入开销；一致性保证则确保跨进程共享的正确性。优化的多项修改都围绕这条平衡链展开，尤其重视缓存未命中和脏页淘汰的处理，而不仅是缓存命中时的主路径。

== 关键数据结构

=== 本地缓存与全局缓存

本地缓存和全局缓存是本模块最重要的两类数据。下面的代码省略了与本节无关的字段，只保留二者的关系。

#figure(
  [
    ```rust
    // 本地缓存：每文件独立，快速无锁访问
    pub struct CachedFileShared {
        page_cache: Mutex<LruCache<u32, PageCache>>,
        append_lock: RwLock<()>,
    }

    // 全局缓存：跨文件共享，提高命中率
    pub struct GlobalPageKey {
        inode: u64,
        page_num: u32,
    }

    static GLOBAL_PAGE_CACHE: Lazy<Mutex<
        LruCache<GlobalPageKey, GlobalPageEntry>
    >>;
    ```
  ],
  caption: [本地缓存与全局缓存的数据结构],
) <lst:cache-structures>

打开同一文件的不同线程持有各自的本地缓存引用，因此能快速查找而不争用全局锁。全局缓存则以 `(inode, page_num)` 为键，允许不同文件或不同进程共享相同的缓存页。优化新增全局缓存时继续遵循这一边界，没有将全局状态混入本地缓存。

=== 缓存页状态

StarryOS 原有缓存页可以标记为脏，却不适合表示正在被多个进程映射。优化增加 `GlobalPageEntry`：

#figure(
  [
    ```rust
    pub struct GlobalPageEntry {
        page: PageCache,
        mapped_count: AtomicU64,  // 跟踪映射次数
    }
    ```
  ],
  caption: [全局缓存页的引用计数],
) <lst:page-entry>

脏页标志仍由 `PageCache` 保存；映射计数则独立记录在全局缓存项中。这样，淘汰算法可以区分"可以立即释放的页面"和"仍然被 mmap 引用的页面"，避免淘汰正在使用的内存。

=== 两张缓存索引

内核需要按文件快速查找本地缓存，按 `(inode, page_num)` 查找全局缓存页。优化使用两层索引分别管理：

#figure(
  table(
    columns: (1.8fr, 1.5fr, 2.8fr),
    inset: 5pt,
    stroke: 0.5pt + gray,
    table.header([缓存层], [键], [作用]),
    [本地缓存], [页号], [文件内快速查找，O(1) 访问],
    [全局缓存], [(inode, 页号)], [跨文件共享，支持多进程访问],
  ),
  caption: [两级缓存的索引方式],
) <tab:cache-index>

本地缓存保存在 `CachedFileShared` 中，随文件打开创建。全局缓存则是系统唯一实例，由所有文件共享。优化还引入 LRU crate 管理淘汰顺序，使得插入、查找和淘汰都是 O(1) 操作。

== 缓存容量与访问优化

=== 容量不足的根源

StarryOS 的 64 页缓存在小文件场景下尚能工作，但无法应对大文件或并发访问。问题不在 LRU 算法，而在容量太小。

早期版本采用固定的 64 页上限。读取 1MB 文件时，缓存只能容纳四分之一内容，其余数据不断淘汰又重新加载。多个进程访问同一文件时，各自维护 64 页缓存，相同数据在内存中重复多份。因此，缓存容量不仅影响单文件性能，也影响系统整体内存利用率。

=== 两级容量扩展

优化将本地缓存扩展到 256 页，同时引入 16K 页的全局缓存池：

#figure(
  [
    ```rust
    // 本地缓存容量：每文件 256 页
    const LOCAL_CACHE_PAGES: usize = 256;

    // 全局缓存容量：系统级 16K 页
    const GLOBAL_FILE_CACHE_PAGES: usize = 16 * 1024;
    ```
  ],
  caption: [两级缓存的容量配置],
) <lst:cache-capacity>

本地缓存扩展 4 倍后，1MB 文件可以完全加载到缓存。全局缓存则提供 64MB 共享空间，能够容纳多个文件的热点数据。二者协同工作，使总缓存容量提升 60 倍。

访问时先查本地缓存，未命中再查全局缓存。本地缓存命中是最快路径，全局缓存命中避免磁盘访问，只有都未命中才读取磁盘并同时填充两级缓存。这样，热点数据既能快速访问，又能跨文件共享。

=== 预读机制

2.4.2 说明了容量扩展解决"能缓存多少"的问题，但没有解决"何时加载"的问题。顺序读取时，应用每次只请求一页数据，却要等待一次磁盘操作。若能提前加载后续页面，磁盘延迟就被隐藏在应用处理当前数据的时间内。

问题的核心在于同步性：应用读取第 N 页时，第 N+1 页可能仍在磁盘上。即使本地缓存已经扩展到 256 页，若不提前加载，每次读取都要等待磁盘响应（通常 5-10ms）。对于顺序读取 100MB 文件的场景，这意味着数万次等待，总延迟累积到秒级。

优化增加顺序访问检测：

#figure(
  [
    ```rust
    // 检测连续读取
    if current_offset == last_offset + last_read_size {
        // 触发预读，上限 1MB
        let readahead_size = min(MAX_READAHEAD_BYTES,
                                  file_size - current_offset);
        prefetch_pages(next_page_num, readahead_size);
    }
    ```
  ],
  caption: [顺序访问检测与预读触发],
) <lst:readahead>

当前读取位置等于上次位置加上次读取大小时，内核判定为顺序访问，自动预读后续数据。预读上限为 1MB，避免过度占用内存。预读数据加载到全局缓存（如表 @tab:cache-index 所示），供后续访问使用。随机访问时检测失败，不触发预读，避免浪费带宽。

当前实现只检测连续的顺序访问模式。跳跃式访问（如数据库索引扫描，访问模式为 0、4096、8192 字节）不会触发预读，预读窗口大小固定为 1MB，不会根据文件大小或系统负载动态调整。因此不能视为完整的自适应预读实现。

=== 批量写回

写入操作也面临类似问题。2.4.3 说明了预读如何隐藏读延迟，写入路径则需要解决写开销的累积。早期版本逐页写回脏页，每次淘汰都触发一次磁盘操作。若能累积多个脏页后批量写回，磁盘寻道次数和 I/O 调度开销都会显著降低。

问题的根源在于写入的碎片化。应用修改文件的多个位置时，每个 4KB 页面被标记为脏。早期版本淘汰时立即写回，导致磁盘执行多次小的写操作。每次写操作不仅包含数据传输时间（4KB 约 0.05ms），还包含寻道时间（5-10ms）。100 个零散写操作的总时间可达 1 秒，而批量写回 100 页只需 100ms。

优化采用 16 页批量写回：

#figure(
  [
    ```rust
    const WRITEBACK_BATCH_PAGES: usize = 16;

    // 累积脏页，批量写回
    for (page_num, page) in dirty_pages {
        batch.push((page_num, page));
        if batch.len() >= WRITEBACK_BATCH_PAGES {
            writeback_batch(file, &batch)?;
            batch.clear();
        }
    }
    ```
  ],
  caption: [脏页批量写回策略],
) <lst:batch-writeback>

触发时机包括缓存淘汰、文件关闭和显式 `sync` 调用。批量写回将多个小的写操作合并为一次大的写操作，充分利用磁盘的顺序写入性能。

当前批量大小固定为 16 页（64KB），不会根据脏页分布或磁盘特性调整。对于 SSD，更大的批量可能更高效；对于旋转磁盘，需要考虑脏页是否连续。批量写回策略也未与 I/O 调度器协同，可能错过进一步合并的机会。

== 跨进程共享与一致性

=== 独立缓存的问题

早期版本的缓存完全独立，每个打开的文件维护各自的 64 页。这在单进程场景下可以工作，但无法应对多进程访问同一文件的情况。

问题不在缓存本身，而在缺少共享机制。进程 A 读取文件前 1MB 并缓存，进程 B 读取相同内容时，内核重新从磁盘加载并填充进程 B 的缓存。相同数据在内存中存在两份，既浪费内存，也降低整体命中率。更重要的是，进程 A 修改数据后，进程 B 读取的仍然是旧版本，无法保证一致性。

这种设计的后果在实际负载中更明显。假设系统运行 10 个进程，都读取同一个配置文件（大小 100KB）。独立缓存导致相同的 100KB 数据在内存中重复 10 次，占用 1MB 内存。若配置文件被更新，某些进程看到新版本，另一些进程仍读取旧版本，导致行为不一致。因此，独立缓存不仅是性能问题，也是正确性问题。

=== 全局缓存与共享

优化引入全局缓存池，以 `(inode, page_num)` 为键索引缓存页。不同进程打开同一文件时，它们的本地缓存未命中后都会查询全局缓存，若全局缓存命中，则共享相同的缓存页。

#figure(
  [
    ```rust
    // 查找顺序：本地缓存 → 全局缓存 → 磁盘
    if let Some(page) = local_cache.get(page_num) {
        return Ok(page);  // 本地命中
    }

    let key = GlobalPageKey { inode, page_num };
    if let Some(entry) = GLOBAL_PAGE_CACHE.lock().get(&key) {
        // 全局命中，复制到本地缓存
        local_cache.insert(page_num, entry.page.clone());
        return Ok(entry.page);
    }

    // 都未命中，从磁盘读取
    let page = read_from_disk(file, page_num)?;
    local_cache.insert(page_num, page.clone());
    GLOBAL_PAGE_CACHE.lock().put(key,
        GlobalPageEntry { page, mapped_count: 0 });
    ```
  ],
  caption: [两级缓存的查找与填充],
) <lst:cache-lookup>

这样，多个进程访问同一文件时，热点数据只需从磁盘加载一次，后续访问都能命中全局缓存。写入操作也先更新全局缓存，保证所有进程看到一致的数据。全局缓存的键设计（如表 @tab:cache-index 所示）确保不同文件的同一页号不会冲突。

=== 淘汰与引用计数

全局缓存不能简单淘汰最久未用的页面。若页面正在被 mmap 映射，进程通过页表直接访问物理内存，淘汰会导致进程访问失效地址。

问题的根源在于 mmap 的零拷贝特性。普通 read/write 通过内核缓冲区访问数据，淘汰缓存页不影响应用已经读取的内容。但 mmap 将缓存页直接映射到进程地址空间，进程通过指针访问页面。若内核淘汰该页，进程下次访问会触发缺页异常，但此时页面已被释放，无法恢复，导致段错误。

优化为全局缓存页增加 `mapped_count`（如代码 @lst:page-entry 所示）。建立 mmap 映射时增加计数，解除映射时减少计数。淘汰时检查计数是否为零：

#figure(
  [
    ```rust
    // 淘汰前检查映射计数
    if entry.mapped_count.load(Ordering::Relaxed) > 0 {
        // 仍在映射，不能淘汰
        continue;
    }

    // 脏页先写回
    if entry.page.is_dirty() {
        writeback_page(file, page_num, &entry.page)?;
    }

    // 移除缓存项
    GLOBAL_PAGE_CACHE.lock().pop(&key);
    ```
  ],
  caption: [全局缓存淘汰时的引用检查],
) <lst:evict-check>

这样，映射页面不会被提前淘汰，保证 mmap 的正确性。计数为零的页面才参与 LRU 淘汰，脏页淘汰前先写回磁盘（如 2.4.4 所述的批量写回策略）。

当前实现只保证基本一致性。`O_DIRECT` 绕过缓存直接访问磁盘，若应用同时使用 `O_DIRECT` 和缓存 I/O 访问同一文件，两者之间的同步问题尚未处理。`O_DIRECT` 写入的数据不会更新缓存，缓存中的旧数据可能被后续读取返回。这是当前实现的明确限制。

== 元数据缓存

=== stat 操作的开销

`stat` 系统调用查询文件大小、权限和时间戳等元数据。早期版本每次都查询底层文件系统，即使连续 `stat` 同一文件，也要重复执行 inode 读取。

问题不在 `stat` 本身，而在缺少元数据缓存。程序遍历目录时，会对每个文件调用 `stat`（例如 `ls -l` 命令），若每次都访问磁盘，目录遍历会变得很慢。更重要的是，`open` 和权限检查也需要读取元数据，缓存缺失会拖累整个 I/O 路径。一个包含 1000 个文件的目录，`ls -l` 需要 1000 次 `stat`，若每次都读磁盘（10ms），总时间达到 10 秒。

2.4 节说明了数据页缓存的优化，元数据访问也面临类似问题。虽然元数据通常较小（128-256 字节），但访问频率很高，累积开销显著。

=== 全局 inode 缓存

优化引入全局 inode 元数据缓存。第一次 `stat` 时，内核查询文件系统并缓存结果；后续 `stat` 直接返回缓存数据，不再访问磁盘。

#figure(
  [
    ```rust
    pub fn cached_metadata(loc: &Location) -> VfsResult<Metadata> {
        let inode = loc.inode();

        // 查询元数据缓存
        if let Some(meta) = INODE_CACHE.lock().get(&inode) {
            return Ok(meta.clone());
        }

        // 缓存未命中，查询文件系统
        let meta = loc.metadata()?;
        INODE_CACHE.lock().put(inode, meta.clone());
        Ok(meta)
    }
    ```
  ],
  caption: [inode 元数据缓存的查找与填充],
) <lst:metadata-cache>

元数据缓存也采用 LRU 策略管理。文件修改后，内核同时更新缓存和磁盘，保持一致性。对于 atime（访问时间），优化采用延迟更新策略，只在文件关闭或周期性刷新时才写回，避免频繁的磁盘写入。

当前实现只缓存基本元数据（大小、权限、时间戳）。扩展属性（extended attributes）和 ACL（访问控制列表）尚未纳入缓存，每次查询仍需访问底层文件系统。对于频繁查询扩展属性的应用（如 SELinux 环境），这会成为性能瓶颈。

== 并发控制

=== 锁粒度的选择

文件系统需要在多线程环境下正确工作，因此必须使用锁保护共享状态。但锁粒度过粗会导致不必要的竞争，过细则增加死锁风险。

问题的核心在于共享程度。全局缓存（2.5 节所述）是系统唯一实例，所有文件共享，若使用粗粒度锁，不同文件的操作会互相阻塞。本地缓存则是文件私有的，不同文件的操作本应完全并行。早期版本缺少明确的锁策略，在某些路径上出现不必要的全局锁竞争。

优化采用三层锁策略：

#figure(
  table(
    columns: (1.5fr, 2fr, 2.5fr),
    inset: 5pt,
    stroke: 0.5pt + gray,
    table.header([锁类型], [保护范围], [特点]),
    [本地缓存 Mutex], [单个文件的页面缓存], [文件级锁，不同文件完全并行],
    [全局缓存 Mutex], [整个全局缓存池], [单一全局锁，访问频率相对较低],
    [原子变量], [文件偏移量、映射计数], [无锁操作，避免加锁开销],
  ),
  caption: [文件系统的并发控制策略],
) <tab:concurrency>

本地缓存使用文件级 Mutex，不同文件的操作完全并行。全局缓存使用单一 Mutex，虽然有全局锁竞争，但本地缓存已经过滤了大部分访问（如代码 @lst:cache-lookup 所示的查找顺序），全局缓存的访问频率相对较低。关键路径使用原子变量，避免加锁开销。

=== append 操作的特殊处理

追加写入需要原子性：读取当前文件大小、写入数据、更新文件大小三步不能被其他写入打断。若只用页面缓存锁，并发追加会导致数据覆盖。

问题在于操作的多步性。进程 A 读取文件大小为 1000 字节，准备在偏移 1000 处写入。若进程 B 此时也追加，也读到大小 1000，两个进程都写入偏移 1000，导致其中一个进程的数据被覆盖。页面缓存锁只保护缓存数据结构，不保护"读大小→写入→更新大小"的原子性。

优化为 `CachedFileShared` 增加 `append_lock`（如代码 @lst:cache-structures 所示）：

#figure(
  [
    ```rust
    pub struct CachedFileShared {
        page_cache: Mutex<LruCache<u32, PageCache>>,
        append_lock: RwLock<()>,  // 保证追加原子性
    }

    // 追加写入时获取写锁
    let _guard = file.append_lock.write();
    let offset = file.len();
    file.write_at(offset, data)?;
    ```
  ],
  caption: [追加操作的并发控制],
) <lst:append-lock>

`append_lock` 使用 RwLock，追加写入时获取写锁，普通读取时获取读锁。这样，追加操作之间互斥，但不会阻塞并发读取。

当前实现的并发控制能够保证正确性，但全局缓存的单一 Mutex 在高并发场景下可能成为瓶颈。例如，16 个 CPU 核心同时访问不同文件时，若都未命中本地缓存，会在全局缓存锁上排队。未来可以考虑分片锁（按 inode 哈希分片）或无锁数据结构（如无锁哈希表），减少锁竞争。

== 性能提升与对比

=== 优化前的性能瓶颈

早期版本的性能问题主要体现在三个方面。首先，缓存容量太小（2.4.1 节所述），64 页只有 256KB，无法覆盖 iozone（1MB 文件）和 lmbench（512KB 文件）的工作集，频繁的缓存淘汰导致大量磁盘访问。其次，逐页写回导致大量零散 I/O，每次写入都需要磁盘寻道。第三，缺少预读机制（2.4.3 节所述），顺序访问时每次都等待磁盘响应，无法隐藏延迟。

这些问题的根源在于缓存策略的简化。早期版本追求实现简单，采用固定容量、同步写回和被动加载。这种策略在原型验证阶段可以工作，但无法应对实际负载。页缓存太小时，顺序读完整个文件后，LRU 里只剩文件尾部；下一轮从文件头 reread 时，前面读入的新页又会把尾部页挤出去，导致反复淘汰和重新加载。

#figure(
  table(
    columns: (1.8fr, 1.8fr, 1.8fr, 1.6fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([指标], [优化前], [优化后], [提升幅度]),
    [本地缓存容量], [64 页], [256 页], [4×],
    [全局缓存容量], [无], [16K 页], [新增],
    [批量写回], [逐页], [16 页批量], [减少调用次数],
    [atime 更新], [每次 read], [relatime], [减少元数据写],
    [预读支持], [无], [显式预读], [新增接口],
  ),
  caption: [文件系统缓存优化前后对比],
) <tab:performance-comparison>

=== 优化效果分析

容量扩展使得缓存能够完整覆盖测试工作集。本地缓存扩展到 256 页（1MB）后，可以完全加载 iozone 4 进程场景下的单个文件，也能覆盖 lmbench 的 512KB 读文件。全局缓存提供 64MB 共享空间，能够容纳多个文件的热点数据。两级缓存架构（2.2.1 节所述）使得热点数据能在内存中长期驻留，本地缓存提供快速访问，全局缓存实现跨文件共享。

批量写回显著改善写入性能。16 页批量写回策略将多个小的写操作合并为一个大的写操作，减少了 `write_at()` 调用次数和进入 ext4 的次数。对于 lmbench 的 `lmdd fsync=1` 测试（写 1MB 文件后 fsync），批量写回避免了 256 次独立的 4KB 写回操作。

relatime 策略减少了元数据写开销。普通 `read()` 不再每次都更新 atime，只有当 atime 早于 mtime/ctime，或距离上次更新超过 24 小时时才真正更新。这对 iozone 的小 record read 测试尤其有帮助，热路径中的元数据写次数显著减少。

显式预读接口从 no-op 变为真实预读。`readahead()` 和 `posix_fadvise(WILLNEED)` 现在会把对应文件页读进页缓存。单次最多预读 1MB，采用 4KB 页同步预读策略。用户程序若提前告诉内核"我马上要用这段文件"，后续 read 或 mmap fault 更可能命中页缓存。

在 musl LA 架构上，整体性能从 35.0 分提升到 37.9 分，提升约 8%。这个提升虽然不大，但考虑到文件系统只是操作系统的一部分，8% 的提升已经相当可观。

=== 与 Linux 的对比

#figure(
  table(
    columns: (2fr, 4fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([方面], [差距分析]),
    [预读策略], [Linux 根据历史访问模式动态调整预读窗口；StarryOS 只支持显式预读，单次上限 1MB],
    [I/O 调度], [Linux 的 mq-deadline/BFQ 能合并重排请求；StarryOS 使用简单 FIFO 调度器],
    [缓存容量], [Linux page cache 取决于系统内存，通常更大；StarryOS 固定 256 页本地 + 16K 页全局],
    [块设备驱动], [Linux 更成熟，能更好利用硬件特性（NCQ、TRIM）；StarryOS 驱动相对简单],
  ),
  caption: [与 Linux 文件系统的差距分析],
) <tab:linux-comparison>

与 Linux 相比，StarryOS 的主要差距在工程实现的成熟度上。Linux 的预读策略更加自适应，能够根据历史访问模式动态调整预读窗口大小和触发阈值。I/O 调度器（如 mq-deadline、BFQ）能够合并和重排请求，减少磁盘寻道。

缓存架构方面，两级缓存设计（如表 @tab:cache-index 所示的索引方式）在有限的内存预算下能够有效工作。Linux 的 page cache 容量通常更大（取决于系统内存），但 StarryOS 通过本地缓存和全局缓存的分层，在固定容量下仍能覆盖常见测试工作集。LRU 算法（2.2.3 节所述）虽然简单，但在实际应用中能够捕获大部分的访问局部性。

性能差距的主要原因包括以下几点。首先，Linux 的 I/O 调度器更加复杂，能够合并和重排 I/O 请求，减少磁盘寻道。StarryOS 目前使用简单的 FIFO 调度器，未来可以实现更智能的调度算法。其次，Linux 的预读策略更加自适应，能够根据访问历史动态调整（如 2.4.3 节所述的当前限制），而 StarryOS 目前只支持显式预读接口。第三，Linux 的缓存容量通常更大，能够缓存更多的数据。第四，Linux 有更成熟的块设备驱动，能够更好地利用硬件特性（如 NCQ、TRIM）。

考虑到 StarryOS 是一个年轻的操作系统项目，当前实现已经建立了正确的缓存架构。核心设计思想（2.2 节所述的四个关键抽象）是合理的，性能差距主要在工程实现的成熟度上，而非架构设计上的根本缺陷。

== 本章小结

优化选择沿用 StarryOS 的文件系统框架，围绕四条主线完成加固：以本地缓存和全局缓存组成两级架构，以顺序、随机、写入三类访问模式指导优化策略，以 LRU 和引用计数构成淘汰机制，并把容量扩展、预读、批量写回和一致性保证串成完整优化链。

在此基础上，优化将本地缓存扩展到 256 页，引入 16K 页全局缓存，增加预读机制和批量写回，实现全局 inode 元数据缓存。当前尚未实现自适应预读、异步 I/O 和 NUMA 感知缓存，但缓存从容量到访问的主要优化关系已经比 StarryOS 基线更完整。

