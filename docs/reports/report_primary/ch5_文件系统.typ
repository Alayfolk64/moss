#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(5)

= 文件系统

== 概述

从 Unix 的设计哲学 —— “Everything is a file.”出发，文件系统要解决的核心问题是：让用户程序用同一套接口去访问性质完全不同的对象——磁盘上的 ext4 文件、内存里的临时文件、/proc 下的内核信息、/dev 下的设备节点。

StarryOS 为这个问题提供了一套完整的基础框架。分四层：axfs-ng-vfs 定义文件系统、目录节点、文件节点的接口；axfs 实现路径解析、页缓存和 ext4/FAT32 后端；kernel/src/file/ 将 VFS 对象统一进文件描述符表；syscall/fs/ 把系统调用参数翻译为上层调用。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-1.svg", width: 80%)
    ]
  ],
  caption: [文件系统分层架构],
) <fig:fs-arch>

在这套框架中，StarryOS 已经提供了几个关键设计：

- *Location*：将目录项与所属挂载点绑定为统一句柄，路径解析穿越挂载点时自动切换文件系统；
- *Mountpoint 树*：子节点用 `Weak` 引用，卸载时自动释放，跨设备检测用 `Arc::ptr_eq` 对比挂载点指针；
- *NodeFlags*：四个标志位（`STREAM`、`NON_CACHEABLE`、`ALWAYS_CACHE`、`BLOCKING`）让节点声明自身 I/O 契约，上层无需判断节点类型；
- *FsContext* + 路径解析：每个任务独立的 `root_dir` 和 `current_dir`，四个解析变体覆盖不同 syscall 语义，符号链接循环检测上限 40 层；
- *文件系统后端*：lwext4_rust 接入 ext4，starry-fatfs 接入 FAT32，MemoryFs 提供内存文件系统，SimpleFs 框架支撑 procfs/devfs。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-2.svg", width: 70%)
    ]
  ],
  caption: [Mountpoint 树],
) <fig:mount-tree>

MossOS 在这套框架上的工作集中在六个方面：挂载系统调用的完善、新式挂载 API 的实现、inode 元数据缓存的引入、扩展属性的支持、文件事件通知（inotify 与 fanotify），以及 I/O 路径的性能优化。下面按这几个方向展开。

== 挂载子系统

挂载解决的核心问题是：把独立的文件系统拼到同一棵目录树下。`mount` 调用在 `Mountpoint` 树上添加节点，路径解析穿越目录时自动切换到对应文件系统的根——这个设计来自 StarryOS。StarryOS 的 `sys_mount` 当前支持 `tmpfs`，flags 参数尚未处理；`sys_umount2` 同样未包含权限和状态检查。MossOS 在此基础上扩展了挂载操作的类型和语义。

=== 传统挂载接口的完善

`sys_mount` 在 MossOS 中除了普通挂载外，支持了三种改变已有挂载关系的操作模式：

#figure(
  table(
    columns: (1fr, 2.6fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([标志], [含义]),
    [`MS_BIND`], [bind mount：让已有目录树在另一条路径下可见，直接复用源挂载点对象，不创建新文件系统实例],
    [`MS_MOVE`], [迁移挂载点：将挂载从一个路径移到另一个路径，底层做 bind + unmount 原子操作],
    [`MS_REMOUNT`], [修改现有挂载的标志（如读写→只读），修改前检查是否有打开的写文件],
  ),
  caption: [sys_mount 新增的操作模式],
) <tab:mount-flags>

此外，MossOS 增加了权限检查（要求 euid == 0），支持 10 种挂载标志（`MS_RDONLY`、`MS_NODEV`、`MS_NOEXEC`、`MS_NOSUID`、`MS_NOATIME`、`MS_NODIRATIME`、`MS_RELATIME`、`MS_STRICTATIME`、`MS_REMOUNT`、`MS_NOSYMFOLLOW`），并将 `fs_type` 从单一的 `"tmpfs"` 扩展到 ext4、FAT32 及 LTP 兼容的多种文件系统。

`sys_umount2` 同样做了大量补全：

#figure(
  table(
    columns: (1.1fr, 2.5fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([标志 / 行为], [说明]),
    [`MNT_DETACH`], [延迟卸载：即使文件被占用也移除挂载，busy 文件在最后一个 fd 关闭后自动释放],
    [`MNT_EXPIRE`], [两阶段过期：首次调用标记到期，再次调用时若无打开文件则卸载，否则返回 EAGAIN],
    [`UMOUNT_NOFOLLOW`], [不跟随符号链接，直接操作路径本身],
    [卸载前清理], [移除页缓存（`drop_file_caches_by_device`）、刷新文件系统脏数据（`flush`）、发送 inotify `IN_UNMOUNT` 事件],
    [忙碌检查], [`MNT_DETACH` 未设置时检查是否有打开的写文件，有则返回 EBUSY],
  ),
  caption: [sys_umount2 新增功能],
) <tab:umount2>

=== 新式挂载 API

传统 `sys_mount` 有一个历史包袱：调用者必须同时拥有源设备访问权限和目标路径挂载权限。容器场景里这很别扭——容器进程应该能在自己的命名空间里自由挂载，却因为没有宿主机路径权限而卡住。Linux 5.2 引入的新式挂载 API 将配置和挂载拆为两步，StarryOS 没有实现这套接口，MossOS 补上了主要路径。

#figure(
  [
    ```
    fd = fsopen("ext4")                        // 创建文件系统上下文
    fsconfig(fd, "source", "/dev/loop0")  // 配置设备路径
    fsconfig(fd, "cmd", "create")          // 完成配置，准备挂载
    mfd = fsmount(fd, 0, 0)                     // 实例化为挂载对象 fd
    move_mount(mfd, "", "/mnt", MOVE_MOUNT_F_EMPTY_PATH)
    ```
  ],
  caption: [新式挂载 API 的完整调用流程],
) <lst:new-mount-api>

五个新系统调用各有明确的职责：

- `fsopen`：按文件系统类型名创建上下文，返回 `FsContextFd`；
- `fsconfig`：对上下文进行配置（设置 source 路径、发出 create 命令），支持 `FSCONFIG_SET_STRING` 和 `FSCONFIG_CMD_CREATE` 两种操作；
- `fsmount`：将配置完成的上下文实例化为 `DetachedMountFd`，此时文件系统已创建但尚未附加到任何路径——"分离的挂载"；
- `open_tree`：打开已挂载的文件系统树作为 detached mount fd（`OPEN_TREE_CLONE` 模式），为 bind mount 提供基于 fd 的接口；
- `move_mount`：将 detached mount fd 挂载到目标路径，支持 `MOVE_MOUNT_F_EMPTY_PATH` 标志（空路径表示直接挂载 fd 本身）。

`DetachedMountFd` 是一个关键设计——它在挂载操作期间作为"尚未附加的挂载对象"存在，可以是全新文件系统（`Filesystem { fs, mount_flags }`）或已有挂载树的引用（`Bind { source: Location }`）。这个中间态让配置和挂载的分离成为可能。

=== Loop 设备

LTP 文件系统测试中大量用例需要在运行时把镜像文件（如 ext4.img）当块设备挂载：先 `mkfs.ext4` 格式化一个普通文件，再 `mount` 它。要支持这个场景，需要能把普通文件当成块设备用。

MossOS 为此实现了 `LoopFileBlockDevice`：

#figure(
  [
    ```rust
    pub struct LoopFileBlockDevice {
        file: FileBackend,   // 被当作块设备使用的普通文件
        blocks: u64,         // 文件总块数 = file.len() / block_size
    }

    impl BlockDriverOps for LoopFileBlockDevice {
        fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> DevResult {
            let offset = self.checked_range(block_id, buf.len())?;
            // 循环 read_at 直到读完，处理短读
        }
        fn write_block(&mut self, block_id: u64, buf: &[u8]) -> DevResult {
            // 块号 → 文件偏移的换算：offset = block_id * block_size
        }
        fn flush(&mut self) -> DevResult {
            self.file.sync(false)  // 将脏数据写回底层文件
        }
    }
    ```
  ],
  caption: [LoopFileBlockDevice 将普通文件适配为块设备接口],
) <lst:loop-device>

核心逻辑是块号到文件偏移的换算：块设备第 N 块对应普通文件从 `N * block_size` 开始的一段内容。ext4 后端仍按块读写，`LoopFileBlockDevice` 把这些块访问翻译成对镜像文件的 `read_at`/`write_at`。`sys_mount` 在识别到 `/dev/loop*` 路径时，将对应文件包装后传给 ext4 后端，镜像文件挂载走普通 ext4 路径，不需要单独处理。读写循环处理了短读/短写（底层文件系统可能一次未返回全部请求字节），只读 loop 设备以非 `MS_RDONLY` 挂载时显式拒绝。

== inode 元数据缓存

频繁的 `stat` 调用和读写伴随的时间戳更新如果每次都穿透到底层文件系统，会在 I/O 密集场景产生大量元数据写——进程只是想读文件，却要附带一次写的开销。StarryOS 的 `metadata()` 直接访问底层节点，未引入额外的缓存层。MossOS 在 VFS 层增加了一套 inode 元数据缓存，以减少频繁穿透底层文件系统带来的开销。

#figure(
  [
    ```rust
    type InodeMetadataCacheKey = (usize, u64);          // (fs_ptr, inode)，唯一定位
    static INODE_METADATA_CACHES:
        Lazy<Mutex<BTreeMap<InodeMetadataCacheKey, SharedInodeMetadataCache>>> = ...;

    struct InodeMetadataCache {
        metadata: Metadata,            // 缓存的 size/atime/mtime/ctime 等
        dirty_size: bool,              // 文件大小变更但未提交
        dirty_update: MetadataUpdate,  // 时间戳/权限等待提交的更新
    }
    ```
  ],
  caption: [inode 元数据缓存：以 (fs_ptr, inode) 为键的全局 BTreeMap],
) <lst:metadata-cache>

缓存以 `(fs_ptr, inode)` 为键存入全局 `BTreeMap`，同一文件被多个进程、多个 fd、多条路径访问时共享同一份缓存。目录的 nlink 随子节点增删变化，缓存容易读到过期值，被排除在外；`NON_CACHEABLE` 节点（procfs 等）内容动态生成，同样不进入缓存。

元数据更新采用延迟提交策略：各字段的修改先合并到 `dirty_update` 中，`sync` 或文件关闭时统一提交，将多次独立的元数据写合并为一次底层调用。atime 遵循 relatime 策略——只有当 atime 早于 mtime 或 ctime、或距上次更新超过 24 小时时，读操作才触发 atime 更新。`MS_NOATIME` 和 `MS_STRICTATIME` 挂载标志可以覆盖这个策略。这个设计让大多数只读访问不再产生元数据写开销。

缓存在以下时机被同步或失效：`fsync`/`close` 时调用 `flush_metadata_cache` 将脏数据写回；文件被删除时 `invalidate_inode_metadata_cache` 先 flush 再移除；设备卸载时 `drop_file_caches_by_device` 批量清理。

== 扩展属性

扩展属性（xattr）是文件系统上以键值对形式存储的元数据，用于 ACL、SELinux 安全标签、文件校验和等场景。与 inode 自带的 mode/uid/gid 不同，xattr 由用户态自由定义和扩展。StarryOS 尚未包含 xattr 相关接口，MossOS 从 syscall、VFS trait 和 inode 存储三个层面补充了这套机制。

#figure(
  table(
    columns: (1.2fr, 2.8fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([层次], [MossOS 实现]),
    [syscall 层], [12 个系统调用：setxattr/getxattr/listxattr/removexattr × 三个变体（路径版、l-符号链接版、fd 版），共 291 行],
    [VFS trait 层], [NodeOps 新增 4 个方法（set_xattr/get_xattr/list_xattr/remove_xattr），默认返回 OperationNotSupported；Location 通过 inherit_methods 透出对应方法],
    [存储层], [per-inode 的 XattrStore（`Mutex<BTreeMap<Vec<u8>, Vec<u8>>>`）挂在 Location 的 TypeMap 上，惰性创建；ext4 通过 lwext4 接口直接读写磁盘 xattr],
  ),
  caption: [xattr 三层实现],
) <tab:xattr-layers>

实现中处理了几个规范语义：

- *命名空间*：支持 `user.`、`security.`、`trusted.`、`system.` 四个前缀，`user.` 仅允许普通文件和目录使用；
- *标志语义*：`XATTR_CREATE` 要求属性不存在才写入，`XATTR_REPLACE` 要求存在才替换，与 `setxattr(2)` 完全一致；
- *大小限制*：属性名最长 255 字节，值最长 65536 字节；
- *inode flags 集成*：immutable 或 append-only 文件自动拒绝 xattr 写入。

== 文件事件通知

用户态程序需要知道文件系统上发生了什么——备份工具感知文件修改，安全软件在文件访问时介入。Linux 提供 inotify 和 fanotify 两套机制覆盖不同场景。StarryOS 尚未包含这两套接口，MossOS 补充了主要路径。

=== inotify

inotify 面向具体文件或目录的变化监视。用户态对目标路径添加 watch，之后 create、delete、modify、attrib、move 等操作产生的事件进入事件队列，通过 `read` 取出。

实现上，一个 inotify fd 维护两张表：

#figure(
  [
    ```rust
    struct Target { dev: u64, ino: u64, is_dir: bool }
    struct Watch  { wd: c_int, target: Target, mask: u32 }
    struct Event  { wd: c_int, mask: u32, cookie: u32, name: Option<String> }

    pub struct InotifyFd {
        watches: Mutex<Vec<Watch>>,          // 当前 fd 上注册的所有 watch
        events: Mutex<VecDeque<Event>>,      // 等待用户态 read 的事件队列
        poll: PollSet,                       // 新事件到达时唤醒 read/poll
    }
    ```
  ],
  caption: [inotify 的 watch 表与事件队列],
) <lst:inotify-structs>

事件投递发生在 VFS 层的创建、删除、重命名、属性更新路径上——这些路径在 StarryOS 中已经存在，MossOS 的工作是在这些路径上插入通知点。移动操作产生成对的 `IN_MOVED_FROM` 和 `IN_MOVED_TO` 事件，通过相同的 cookie 关联，让接收方能识别是同一次 rename。卸载时向所有相关 watch 发送 `IN_UNMOUNT`。

#figure(
  [
    #align(center)[
      #image("./pictrues/图5-7-1.svg", width: 90%)
    ]
  ],
  caption: [inotify 从注册 watch 到用户态取出事件的完整数据流],
) <fig:inotify-flow>

=== fanotify

fanotify 解决 inotify 覆盖不了的两个场景：监视范围可以是一整个挂载点甚至文件系统（而非单个路径），以及能在访问发生时主动介入（而非事后通知）。

mark 可以挂在单个文件、目录或整个挂载点上：

#figure(
  [
    ```rust
    enum FanotifyMarkKind { Inode, Mount, Filesystem }

    struct FanotifyMark {
        target: FanotifyTarget,
        kind: FanotifyMarkKind,    // mark 的作用范围
        mask: u64,                 // FAN_OPEN/FAN_ACCESS/FAN_CREATE 等
        ignored_mask: u64,         // 临时忽略的事件集合
    }

    struct FanotifyEvent {
        mask: u64,
        file: Option<Arc<dyn FileLike>>,  // 需要返回 fd 时保留文件对象
        fid: Option<FanotifyTarget>,      // FID 模式下报告的文件身份
        permission_id: Option<u64>,       // 权限事件等待用户态响应
    }

    pub struct FanotifyFd {
        marks: Mutex<Vec<FanotifyMark>>,
        events: Mutex<VecDeque<FanotifyEvent>>,
        permissions: Mutex<Vec<FanotifyPermission>>,  // 尚未 allow/deny 的权限事件
        poll: PollSet,
    }
    ```
  ],
  caption: [fanotify 的 mark、事件队列与权限等待表],
) <lst:fanotify-structs>

权限事件（`FAN_OPEN_PERM`、`FAN_ACCESS_PERM`）是 fanotify 最特殊的部分：内核在文件被打开或访问时先等待监听进程响应，再决定是否放行。触发进程在此期间阻塞，监听进程通过写入 allow 或 deny 响应。安全软件（防病毒、DLP）依赖这个机制在文件访问路径上做拦截。

#figure(
  [
    #align(center)[
      #image("./pictrues/图5-7-2.svg", width: 90%)
    ]
  ],
  caption: [fanotify 权限事件的三方交互时序],
) <fig:fanotify-perm>

=== 无监听快速路径

inotify 和 fanotify 需要在 read、write、open、close 等热路径上插入事件投递逻辑。没有注册监听者时，这些逻辑不应带来任何额外开销。MossOS 用全局计数和掩码做早期过滤：inotify 维护全局 watch 计数，为零时通知函数立即返回；fanotify 维护全局兴趣掩码，对当前事件无命中时跳过后续所有处理。两处检查都只需一次原子读，无监听者时不进入任何路径解析、mark 匹配或事件队列操作。

== I/O 性能优化

文件系统的性能差异在 StarryOS 和 MossOS 之间非常显著：iozone 测试中，顺序写提升约 7--13 倍，顺序读提升约 5 倍，随机读写提升约 5--9 倍。这些提升根源于页缓存和元数据缓存的几项关键优化。

=== 页缓存增强

StarryOS 已经有基础的页缓存机制，但 MossOS 做了三个关键改进。

*全局索引 + per-file LRU*。StarryOS 的页缓存文件查找路径较长。MossOS 以 `(fs_ptr, inode)` 为键建立全局 `BTreeMap`，同一文件被多个 fd 打开时找到同一份缓存记录。每个文件的缓存数据放在独立的 LRU 中，默认容量 256 个 4KiB 页（1MiB），单个文件的大规模顺序扫描不会挤出其他文件的缓存。

*避免 read-before-write*。页缓存以 4KiB 为单位操作，朴素的实现在目标页缺失时会先读出完整旧页再部分覆盖——这对修改已有文件中间段是必要的，但对新文件顺序写或 EOF 之后 append 而言是浪费。MossOS 判断写入是否完全覆盖了页内旧有效区域，如果是则直接分配清零页而不调用底层 `read_at()`，消除了新页写入前的无效读取。

*脏页合并写回*。脏页按页号排序，连续页合并成一批写入，每批最多 16 页（64KiB）。相比逐页提交，顺序写场景下的块设备请求数量明显减少。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-5-1.svg", width: 78%)
    ]
  ],
  caption: [I/O 读写三条路径：cached、direct 和 in-memory],
) <fig:io-paths>

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-5-2.svg", width: 82%)
    ]
  ],
  caption: [脏页按页号排序后合并为批次写回],
) <fig:dirty-writeback>

Direct I/O 与 Cached I/O 之间的一致性也得到了处理：direct 读前先将目标范围内的 dirty cached 页写回，direct 写后使对应干净页失效，两条路径在重叠区域不会读到不一致的内容。

=== 元数据缓存

元数据缓存的实现细节已在第三节中描述。这里补充其在性能上的意义：IOPS 密集场景下，大量 `stat`/`fstat` 调用不再穿透底层文件系统；atime 的 relatime 策略使绝大多数只读访问不产生任何元数据写；延迟提交将多次独立的元数据更新合并为一次底层调用。

=== 预读

预读在应用程序实际访问页面之前提前将文件内容放入页缓存，降低后续访问的等待时间。StarryOS 的 `fadvise(WILLNEED)` 仅做参数检查，未执行实际预读，MossOS 补全了这条路径：`sys_readahead()` 和 `fadvise(POSIX_FADV_WILLNEED)` 找到文件对应的 cached backend，按请求范围同步地将页面放入缓存，单次最多 1MiB。direct I/O 场景保持 no-op，符合绕过缓存的语义。

mmap 访问时每次 page fault 只处理当前缺失页。MossOS 在 fault 后检查 `last_fault_page`：若当前页号比上次大 1，说明是顺序扫描，额外将后续 4 页预热入缓存。预读只放入页缓存不建立页表映射，后续 fault 命中缓存时延迟更低，且不影响随机访问场景下单次 fault 的延迟。

=== 性能数据

为验证优化效果，使用 iozone 在 QEMU RISC-V 虚拟机上进行吞吐对比。对比对象为优化前的 StarryOS、优化后的 MossOS（均 1 核 1GB 内存，与竞赛评测环境配置一致），以及 Ubuntu 26.04 基准（4 核 4GB 内存）。

#figure(
  table(
    columns: (1.5fr, 1fr, 1fr, 1fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([测试场景], [优化前], [优化后], [Ubuntu]),
    [顺序写], [3524], [33221], [51922],
    [顺序读], [13136], [81470], [104754],
    [顺序重写], [7471], [81639], [72760],
    [随机读], [11082], [57668], [57175],
    [随机写], [7141], [59978], [59817],
    [反向读], [10743], [58824], [67361],
    [pread 并发读], [14184], [108348], [125270],
    [pwrite 并发写], [3724], [28263], [39335],
  ),
  caption: [iozone 性能对比（glibc，单位 kB/sec）],
) <tab:iozone-glibc>

#figure(
  [
    #align(center)[
      #image("./pictrues/code/iozone_glibc_before_after.svg", width: 96%)
    ]
  ],
  caption: [iozone 优化前后吞吐量对比（glibc）],
) <fig:iozone-glibc-opt>

#figure(
  table(
    columns: (1.5fr, 1fr, 1fr, 1fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([测试场景], [优化前], [优化后], [Ubuntu]),
    [顺序写], [3524], [26407], [44818],
    [顺序读], [13136], [62733], [98518],
    [顺序重写], [7471], [62254], [69316],
    [随机读], [11082], [43813], [91905],
    [随机写], [7141], [51344], [57648],
    [反向读], [10743], [54217], [78861],
    [pread 并发读], [14184], [88315], [141988],
    [pwrite 并发写], [3724], [28192], [49303],
  ),
  caption: [iozone 性能对比（musl，单位 kB/sec）],
) <tab:iozone-musl>

#figure(
  [
    #align(center)[
      #image("./pictrues/code/iozone_musl_before_after.svg", width: 96%)
    ]
  ],
  caption: [iozone 优化前后吞吐量对比（musl）],
) <fig:iozone-musl-opt>

综合两组 libc 结果，MossOS 在所有测试项上均显著优于优化前的 StarryOS。随机读写吞吐接近 Ubuntu 基准，顺序重写已在 glibc 环境下超出 Ubuntu。

== 本章小结

本章介绍了 MossOS 在 StarryOS 文件系统框架上的六项增量工作。挂载子系统从 tmpfs 扩展到多种文件系统，并补充了 bind/move/remount 及新式 API；inode 元数据缓存将频繁的 stat 和时间戳更新从"穿透底层"变为"内存命中"，并采用 relatime 和延迟提交策略减少元数据写；扩展属性从无到有三层实现，覆盖 12 个系统调用和 ext4 透传；inotify 和 fanotify 填补了文件事件通知的空白，无监听者时通过全局计数和兴趣掩码零开销过滤。I/O 性能优化——全局索引页缓存、避免 read-before-write、脏页合并写回、预读补全——将这些设计选择转化为 iozone 上 5--13 倍的吞吐提升。
