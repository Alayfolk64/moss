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
- *FsContext* + 路径解析：记录任务的 `root_dir` 和 `current_dir`，创建进程时默认复制，在 `CLONE_FS` 下共享；四个解析变体覆盖不同 syscall 语义，符号链接循环检测上限为 40 层；
- *文件系统后端*：lwext4_rust 接入 ext4，starry-fatfs 接入 FAT32，MemoryFs 提供内存文件系统，SimpleFs 框架支撑 procfs/devfs。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-2.svg", width: 70%)
    ]
  ],
  caption: [Mountpoint 树],
) <fig:mount-tree>

MossOS 在此基础上完善了传统与新式挂载接口，引入 inode 元数据缓存和 Folio 文件页缓存，并补充扩展属性、inotify、fanotify 等功能。下面按这些方向展开。

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
    [`MS_MOVE`], [迁移挂载点：先在目标路径建立 bind mount，再卸载源挂载],
    [`MS_REMOUNT`], [修改现有挂载的标志（如读写→只读），修改前检查是否有打开的写文件],
  ),
  caption: [sys_mount 新增的操作模式],
) <tab:mount-flags>

此外，MossOS 增加了权限检查（要求 euid == 0），支持 10 种挂载标志（`MS_RDONLY`、`MS_NODEV`、`MS_NOEXEC`、`MS_NOSUID`、`MS_NOATIME`、`MS_NODIRATIME`、`MS_RELATIME`、`MS_STRICTATIME`、`MS_REMOUNT`、`MS_NOSYMFOLLOW`）。普通磁盘和 Loop 设备上的 ext2、ext3、ext4 统一由 ext4 后端解析；`xfs`、`btrfs`、`vfat`、`exfat`、`ntfs` 等类型目前用于兼容测试中的挂载流程，挂载后提供隔离的内存文件系统，不解析对应的磁盘格式。

`sys_umount2` 同样做了大量补全：

#figure(
  table(
    columns: (1.1fr, 2.5fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([标志 / 行为], [说明]),
    [`MNT_DETACH`], [延迟卸载：跳过打开写文件的 busy 检查，并从挂载树中移除挂载],
    [`MNT_EXPIRE`], [两阶段过期：首次调用标记并返回 EAGAIN；其间未再次访问时，第二次调用执行卸载],
    [`UMOUNT_NOFOLLOW`], [不跟随符号链接，直接操作路径本身],
    [卸载前清理], [移除页缓存（`drop_file_caches_by_device`）、刷新文件系统脏数据（`flush`）、发送 inotify `IN_UNMOUNT` 事件],
    [忙碌检查], [`MNT_DETACH` 未设置时检查是否有打开的写文件，有则返回 EBUSY],
  ),
  caption: [sys_umount2 新增功能],
) <tab:umount2>

=== 新式挂载 API

Linux 新式挂载 API 将文件系统配置、实例化和接入目录树拆开。StarryOS 没有实现这套接口，MossOS 补充了其主要调用链。

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
  caption: [新式挂载 API 的主要调用流程],
) <lst:new-mount-api>

当前实现覆盖以下职责：

- `fsopen`：按文件系统类型名创建上下文，返回 `FsContextFd`；
- `fsconfig`：设置 `source` 路径并处理 `FSCONFIG_CMD_CREATE`；其余部分命令仅完成参数兼容，尚未保存为文件系统配置；
- `fsmount`：将配置完成的上下文实例化为 `DetachedMountFd`，其中只有只读属性会转换为实际挂载标志；
- `open_tree`：打开已挂载的文件系统树作为 detached mount fd（`OPEN_TREE_CLONE` 模式），为 bind mount 提供基于 fd 的接口；
- `move_mount`：在 `MOVE_MOUNT_F_EMPTY_PATH` 模式下将 detached mount fd 接入目标路径，尚未支持路径到路径的移动。

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
        fn read_block(&self, block_id: u64, buf: &mut [u8]) -> DevResult {
            let offset = self.checked_range(block_id, buf.len())?;
            // 循环 read_at 直到读完，处理短读
        }
        fn write_block(&self, block_id: u64, buf: &[u8]) -> DevResult {
            // 块号 → 文件偏移的换算：offset = block_id * block_size
        }
        fn flush(&self) -> DevResult {
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

缓存以 `(fs_ptr, inode)` 为键，同一文件被多个进程、多个 fd 或多条路径访问时共享同一份元数据。全局索引划分为 64 个分片，每个分片使用独立的 `RwLock`；冷加载和写回分别使用可睡眠锁，避免慢速文件系统访问占用自旋锁。目录同样进入缓存，目录项增删后主动使相关缓存失效；`NON_CACHEABLE` 节点则始终直接读取底层信息。

元数据更新先合并到缓存，再在 `fsync`、全局同步、缓存失效或卸载时提交。atime 默认遵循 relatime：只有 atime 不晚于 mtime 或 ctime，或者距上次更新超过 24 小时时才记录新值；`MS_NOATIME` 和 `MS_STRICTATIME` 可覆盖该策略。这样既能合并重复更新，也避免普通文件关闭时产生不必要的同步写入。

== Folio 文件页缓存

MossOS 使用 Folio 作为普通文件的数据缓存单元。一个 Folio 由 1 到 16 个连续的 4 KiB 页面组成，并统一记录加载、脏页、写回和截断状态。相邻页面可以成组加载和写回，随机访问仍可退化为单页处理。

同一 inode 的不同 fd 和进程共享一份文件地址空间。文件缓存和 Folio 索引均采用 64 个分片，缓存容量不再按单个文件固定为 256 页，而是根据可用内存水位回收较冷的 Folio。Direct I/O 读前会写回重叠范围内的脏缓存，写后使对应的干净缓存失效，从而保持两条 I/O 路径的一致性。

`readahead` 和 `POSIX_FADV_WILLNEED` 会把预读任务交给后台工作队列，单次最多处理 1 MiB；文件映射缺页也会按访问模式预取相邻页面。Folio 在 BuildStorm 场景中的锁粒度、并发缺页和性能优化已在专项报告中展开，本节只说明其在文件系统中的基础作用。

== 扩展属性

扩展属性（xattr）是附加在文件上的键值对，可用于 ACL、安全标签和文件校验信息等场景。StarryOS 尚未包含相关接口，MossOS 补充了系统调用和 VFS 接口，并实现了当前所需的内存存储路径。

#figure(
  table(
    columns: (1.2fr, 2.8fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([层次], [MossOS 实现]),
    [syscall 层], [12 个系统调用：setxattr/getxattr/listxattr/removexattr × 三个变体（路径版、l-符号链接版、fd 版）],
    [VFS trait 层], [NodeOps 新增 4 个方法（set_xattr/get_xattr/list_xattr/remove_xattr），默认返回 OperationNotSupported；Location 通过 inherit_methods 透出对应方法],
    [存储层], [`XattrStore`（`Mutex<BTreeMap<Vec<u8>, Vec<u8>>>`）挂在目录项关联的 TypeMap 上并按需创建；当前系统调用使用该内存存储],
  ),
  caption: [xattr 三层实现],
) <tab:xattr-layers>

实现中处理了几个规范语义：

- *命名空间*：支持 `user.`、`security.`、`trusted.`、`system.` 四个前缀，`user.` 仅允许普通文件和目录使用；
- *标志语义*：`XATTR_CREATE` 要求属性不存在，`XATTR_REPLACE` 要求属性已经存在；
- *大小限制*：属性名最长 255 字节，值最长 65536 字节；
- *inode flags 集成*：设置属性时，immutable 或 append-only 文件会被拒绝。

ext4 后端已经提供直接读写磁盘 xattr 的 VFS 方法，但尚未接入上述系统调用路径，因此当前扩展属性不会持久化到 ext4 磁盘。

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

inotify 和 fanotify 需要在 read、write、open、close 等热路径上插入事件投递逻辑。MossOS 用全局计数和掩码提前过滤：inotify 的全局 watch 计数为零时，通知函数通过一次原子读取直接返回；fanotify 先检查全局兴趣掩码，当前事件无命中时跳过后续处理，命中后再检查仍然有效的 fd 和 mark。这样可以降低未监听事件在普通 I/O 路径上的额外开销。

== 文件 I/O 测试

初赛阶段曾使用 iozone 在 QEMU RISC-V 环境中对文件 I/O 吞吐量进行测试。下表保留当时记录的优化前基线、MossOS 测量值和 Ubuntu 参考值，用于说明初赛阶段文件缓存改造的效果；决赛阶段针对 BuildStorm 继续完成的文件系统性能优化及测试数据见专项报告。

#figure(
  table(
    columns: (1.5fr, 1fr, 1fr, 1fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([测试场景], [优化前], [初赛 MossOS], [Ubuntu 参考]),
    [顺序写], [3524], [33221], [51922],
    [顺序读], [13136], [81470], [104754],
    [顺序重写], [7471], [81639], [72760],
    [随机读], [11082], [57668], [57175],
    [随机写], [7141], [59978], [59817],
    [反向读], [10743], [58824], [67361],
    [pread 并发读], [14184], [108348], [125270],
    [pwrite 并发写], [3724], [28263], [39335],
  ),
  caption: [初赛阶段 iozone 性能记录（glibc，单位 kB/sec）],
) <tab:iozone-glibc>

#figure(
  [
    #align(center)[
      #image("./pictrues/code/iozone_glibc_before_after.svg", width: 96%)
    ]
  ],
  caption: [初赛阶段 iozone 吞吐量对比（glibc）],
) <fig:iozone-glibc-opt>

#figure(
  table(
    columns: (1.5fr, 1fr, 1fr, 1fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([测试场景], [优化前], [初赛 MossOS], [Ubuntu 参考]),
    [顺序写], [3524], [26407], [44818],
    [顺序读], [13136], [62733], [98518],
    [顺序重写], [7471], [62254], [69316],
    [随机读], [11082], [43813], [91905],
    [随机写], [7141], [51344], [57648],
    [反向读], [10743], [54217], [78861],
    [pread 并发读], [14184], [88315], [141988],
    [pwrite 并发写], [3724], [28192], [49303],
  ),
  caption: [初赛阶段 iozone 性能记录（musl，单位 kB/sec）],
) <tab:iozone-musl>

#figure(
  [
    #align(center)[
      #image("./pictrues/code/iozone_musl_before_after.svg", width: 96%)
    ]
  ],
  caption: [初赛阶段 iozone 吞吐量对比（musl）],
) <fig:iozone-musl-opt>

两组 libc 的测试项均高于优化前基线，其中 glibc 环境下的顺序重写、随机读写已达到同期 Ubuntu 参考值附近。由于这些数据采集于初赛阶段，不作为当前决赛版本的最终性能结论。

== 本章小结

本章介绍了 MossOS 对文件系统功能和缓存结构的完善。挂载子系统补充了 bind、move、remount、卸载检查、新式挂载 API 和 Loop 设备；inode 元数据缓存通过分片索引、relatime 和延迟提交减少底层访问；Folio 为普通文件 I/O、预读和文件映射提供统一的数据缓存；扩展属性、inotify 和 fanotify 则补齐了相关 Linux 接口。文件系统支持范围和扩展属性持久化仍有明确边界，后续工作将在现有 VFS 接口上继续完善。
