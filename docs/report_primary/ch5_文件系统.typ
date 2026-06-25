#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(5)

= 文件系统

== 整体架构

正如众计算机学子所周知的 Linux 设计哲学 —— “一切皆文件” 。文件系统要解决的核心问题是：让用户程序用同一套接口（open/read/write/stat）去访问性质完全不同的对象——磁盘上的 ext4 文件、内存里的临时文件、内核生成的 /proc 条目和/dev 下的设备节点。这些对象的存储方式、内容来源和访问语义差异很大，但用户程序不该感知这种差异。

moss 的文件系统沿用 StarryOS 的做法分四层来处理这个问题。最底层 axfs-ng-vfs 只定义接口：文件系统是什么、目录节点能做什么、文件节点能做什么，不涉及任何存储细节。axfs 在此之上实现路径解析、页缓存和具体的 ext4/FAT32 后端。再往上，kernel/src/file/ 提供 Linux 兼容的 `FileLike` 接口，把 VFS 的文件对象、管道、socket、设备文件统一进一张文件描述符表。最顶层 syscall/fs/ 负责把系统调用的参数翻译成对上层接口的调用。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-1.svg", width: 80%)
    ]
  ],
  caption: [moss 文件系统分层架构],
) <fig:fs-arch>

这种分层的好处是职责清晰：路径解析、挂载语义、权限检查在各自的层处理，不会混入具体文件系统的实现里。StarryOS 提供了这套框架的雏形，moss 的工作主要集中在挂载命名空间的完整化、页缓存的统一管理、I/O 多路复用和文件事件通知上。

== 挂载子系统

没有挂载，"一切皆文件"就只是句口号。磁盘分区、插入的 U 盘、内核生成的 /proc、内存里的临时目录——它们生来是独立的存储对象，各自有自己的根，根本不认识彼此。挂载解决的就是这个问题：把独立的文件系统动态拼到同一棵目录树下，让 `/home/user` 和 `/proc/meminfo` 可以用同一套接口访问，调用方完全不需要知道底下的存储是什么。

在内核里，每次 `mount` 调用都在一棵 `Mountpoint` 树上添加一个节点，记录"这个文件系统的根接在了哪个目录项上"。路径解析穿越目录时，一旦碰到挂载点就自动切换到对应文件系统的根——这个切换对调用方透明，`open("/mnt/foo")` 和 `open("/bin/bash")` 走同一套代码路径，差别只在于前者在路途中换了一次"文件系统"。

=== 传统挂载接口

`sys_mount` 除了普通挂载外，还支持几种改变已有挂载关系的操作模式：

#figure(
  table(
    columns: (1fr, 2.6fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([标志], [含义]),
    [`MS_BIND`], [bind mount：让已有目录树在另一条路径下可见，不复制任何文件],
    [`MS_MOVE`], [迁移挂载点：将挂载从一个路径移到另一个路径，不卸载重挂],
    [`MS_REMOUNT`], [修改现有挂载的标志，如将读写挂载改为只读],
    [`MS_PRIVATE`], [命名空间隔离：本挂载点的变动不向父/子命名空间传播],
  ),
  caption: [sys_mount 支持的操作模式],
) <tab:mount-flags>

bind mount 值得单独说一下。它不创建新的文件系统实例，只是让同一棵目录树从另一条路径也可以访问到。容器场景里常用这个：把宿主机的某个目录以只读方式 bind mount 进容器，容器内用户能读到内容，但任何写操作都会被挂载标志拦下来，底层数据不受影响。moss 的实现直接复用源挂载点对象，新的 `Location` 指向同一个 `Mountpoint`，device_id 天然相同，`stat()` 结果与 Linux 一致。

`sys_umount2` 卸载时会向所有关注该挂载点的 inotify watch 发送 IN_UNMOUNT 事件，监视 /mnt 的应用能在文件系统被卸载时收到通知。

=== 新式挂载 API

传统 `sys_mount` 有个历史包袱：调用者必须同时拥有源设备的访问权限和目标路径的挂载权限，两者缺一不可。在容器场景里这很别扭——容器进程应该能在自己的命名空间里自由挂载，却因为没有宿主机路径的权限而卡住。

Linux 5.2 针对这个问题引入了基于 fd 的新式挂载 API。StarryOS 没有实现这套接口，moss 参考 Linux 内核的语义补上了这部分，把挂载拆成几个独立步骤：

#figure(
  [
    ```
    fd  = fsopen("ext4")                        // 创建文件系统上下文
          fsconfig(fd, "source", "/dev/loop0")  // 配置设备路径
          fsconfig(fd, "cmd", "create")          // 完成配置，准备挂载
    mfd = fsmount(fd, 0, 0)                     // 实例化为挂载对象 fd
          move_mount(mfd, "/mnt")               // 将挂载接入目录树
    ```
  ],
  caption: [新式挂载 API 的调用流程],
) <lst:new-mount-api>

关键在于 `fsopen` 到 `fsmount` 这几步可以在无特权的用户命名空间里完成，只有最后的 `move_mount` 才需要对目标路径的权限。`open_tree` 反向工作，把现有的挂载子树克隆成一个 fd，可以跨命名空间传递再接入。这五个调用共同构成了容器运行时和命名空间工具所依赖的挂载语义基础。

== VFS 抽象层

=== 目录树与挂载上下文

路径解析看起来简单——逐级查目录——但挂载的存在让它变得复杂。`/mnt` 可能挂载了一个 ext4 镜像，`/mnt` 里的某个子目录又做了 bind mount。穿越挂载点时，"当前位置"不只是一个目录项，还包括它处于哪个挂载上下文里——因为挂载标志（只读、noatime 等）是挂载级别的属性，同一个目录项在不同挂载下行为可以不同。

moss 用 `Location` 把这两者绑在一起：

#figure(
  [
    ```rust
    pub struct Location {
        mountpoint: Arc<Mountpoint>,  // 当前所属挂载点
        entry: DirEntry,              // 目录树节点
    }

    pub struct Mountpoint {
        parent: Option<Location>,     // 挂附在哪里
        children: Mutex<HashMap<ReferenceKey, Weak<Mountpoint>>>,
        device_id: DeviceId,
        flags: MountFlags,
    }
    ```
  ],
  caption: [Location：将目录节点与挂载上下文绑定为统一句柄],
) <lst:location>

`Location` 作为路径解析过程中所有操作的句柄，有几个直接的好处：bind mount 不需要额外的数据结构，直接复用源挂载点对象，`stat()` 返回的 device_id 便自然正确；堆叠挂载（同一目录连续 mount 多次）通过 `effective_mountpoint()` 向上遍历到最顶层，这样调用方便看不到底层细节；cross-device 的 `link` 和 `rename` 用 `Arc::ptr_eq` 对比两个 `Location` 的挂载点指针即可，不需要额外的设备号比较逻辑。

`Mountpoint` 树的子节点用 `Weak` 引用而非 `Arc`，这个选择是为了让卸载操作自然生效——子挂载点没有外部引用时会自动释放，不需要显式通知父节点，也可以避免父子引用形成环，影响挂载点的释放。卸载前的 EBUSY 检查会反过来用 `Weak::strong_count` 判断子挂载点是否还活着，如果这个挂载点还在被使用的话则拒绝卸载返回错误码 EBUSY 。

@fig:mount-tree 展示了一个典型的挂载点树。rootfs 挂载在根，/proc、/tmp、/mnt 作为三个并列的子挂载点分别挂载 procfs、tmpfs 和 ext4 镜像，父到子的引用均为 Arc。/mnt 下还有一个 bind mount 到 /mnt/ro，以只读标志重新暴露同一份 ext4 数据，这个子挂载点用 Weak 引用持有，卸载 /mnt 时它会随之自动释放。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-2.svg", width: 70%)
    ]
  ],
  caption: [Mountpoint 树],
) <fig:mount-tree>

=== 节点能力标志

把 pipe、procfs 节点、普通文件、设备节点统一抽象为"文件节点"，上层就需要知道怎么对待它们的差异。一种做法是在 VFS 里做类型判断，但这样每增加一种节点类型都要修改上层，很不利于代码的实现。moss 的做法是让节点自己声明 I/O 契约，用 `NodeFlags` 表达。

#figure(
  table(
    columns: (1fr, 2.2fr, 1.6fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([标志], [含义], [使用方]),
    [`STREAM`], [没有文件偏移的概念，读写始终从头开始], [pipe、socket],
    [`NON_CACHEABLE`], [内容动态生成，不能缓存元数据], [procfs、sysfs],
    [`ALWAYS_CACHE`], [强制走页缓存，没有块设备后端], [tmpfs],
    [`BLOCKING`], [I/O 直接阻塞，不包 poll 等待循环], [普通文件],
  ),
  caption: [NodeFlags：节点向上层声明自身 I/O 契约],
) <tab:node-flags>

有了这些标志，VFS 就能做出正确的决策：`STREAM` 节点不维护文件偏移；`NON_CACHEABLE` 节点每次 stat 都走后端；`ALWAYS_CACHE` 节点的写入不需要回写到任何设备。上层不再需要 if-else 判断节点类型，节点的差异被限制在声明这一步。

元数据缓存以 `(fs_ptr, inode)` 为键，存入了全局的 `BTreeMap`，这样多个进程、多个 fd、多个路径访问同一个文件时，可以共享同一份缓存，提高访问效率。由于目录的 nlink 会随子节点增删而变化，缓存它很容易导致读到过期数据，所以将目录节点被刻意排除在元数据缓存外。

=== 路径解析与 FsContext

路径解析需要处理几个彼此独立的问题：每个进程有自己的工作目录和 chroot 根；符号链接可能形成循环；穿越挂载点要切换文件系统上下文。

moss 用 `scope_local!` 给每个任务绑定一个独立的 `FsContext`，里面存着 `root_dir` 和 `current_dir`。`scope_local!` 是 ArceOS 提供的机制，类似线程局部存储，但生命周期和任务绑定，fork/clone 时从父任务继承。`root_dir` 既是 chroot 的锚点，也是路径向上遍历的上界——遇到 `..` 到达根之后继续向上，会被静默截断，这正是 chroot 应有的行为。

符号链接的循环检测靠一个共享计数器 `follow_count`，它以可变引用的形式贯穿整个解析调用栈。A→B→C 这样的间接链，三次跳转都计入同一个计数器，不会因为"每次只跟一跳"而绕过上限。上限设为 40，与 Linux 一致，超过就返回错误，避免死循环。

解析本身有四个变体，对应不同的 syscall 语义：

#figure(
  table(
    columns: (1.7fr, 2.3fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([变体], [适用场景]),
    [`resolve`], [跟随末端符号链接，open/stat 等常规路径],
    [`resolve_no_follow`], [不跟随末端符号链接，lstat/lchown],
    [`resolve_nonexistent`], [父目录必须存在，末端必须不存在，creat/mkdir],
    [`resolve_parent`], [返回父目录和末端名称二元组，rename/link],
  ),
  caption: [路径解析的四个变体覆盖所有 syscall 场景],
) <tab:resolve-variants>

每次目录查找完成后，VFS 自动调用 `resolve_mountpoint()` 检查是否到达挂载点，如果是则切换到对应文件系统的根。这个操作对调用方透明，穿越挂载点和普通的目录查找在接口上没有区别，可以让所有系统调用都共享同一套正确路径语义。@fig:resolve-flow 展示了一次完整路径解析的处理流程。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-2-3.svg", width: 75%)
    ]
  ],
  caption: [路径解析的完整处理流程],
) <fig:resolve-flow>

== 文件系统实现

=== 磁盘文件系统

ext4 是承载根文件系统和用户程序的主力，moss 通过 `lwext4_rust`（lwext4 C 库的 Rust FFI 绑定）接入，底层块访问通过 `BlockDriverOps` trait 向下适配。这里有一个实际的测试问题：LTP 中很多用例会在运行时用 `mount` 把一个镜像文件挂载成 ext4，然后在里面做文件操作测试。要支持这个场景，需要能把普通文件当成块设备用。

moss 为此实现了 `LoopFileBlockDevice`：

#figure(
  [
    ```rust
    pub struct LoopFileBlockDevice {
        // 被当作块设备使用的普通镜像文件。
        file: Arc<Mutex<File>>,
        // ext4 后端看到的逻辑块大小。
        block_size: u32,
    }

    impl BlockDriverOps for LoopFileBlockDevice {
        fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> ... {
            let offset = block_id * self.block_size as u64;
            self.file.lock().read_at(offset, buf)
        }

        fn write_block(&mut self, block_id: u64, buf: &[u8]) -> ... {
            let offset = block_id * self.block_size as u64;
            self.file.lock().write_at(offset, buf)
        }
    }
    ```
  ],
  caption: [LoopFileBlockDevice 将普通文件适配为块设备接口],
) <lst:loop-device>

这层适配的核心就是块号到文件偏移的换算：块设备第 N 块，对应普通文件中从 `N * block_size` 开始的一段内容。ext4 后端仍然按块读写，`LoopFileBlockDevice` 则把这些块访问翻译成对镜像文件固定偏移处的 `read_at`/`write_at`，这样普通文件就具备了块设备接口。

`sys_mount` 识别 `/dev/loop*` 路径时，把对应文件包装成 `LoopFileBlockDevice`，再传给 ext4 后端。这样镜像文件挂载就走普通的 ext4 挂载路径，不需要单独处理。

FAT32 基于 `starry-fatfs`（StarryOS 基础层的纯 Rust 实现）接入，底层负责解析 FAT 表和目录项，上层仍然通过同一套 VFS 接口执行 open/read/write/stat 等操作。FAT 本身只保存文件名、大小、目录关系和只读、隐藏等简单属性，没有 Linux 的 uid/gid、rwx 权限位和完整 inode 模型，因此适配层在返回元数据时为这些字段补出默认值：普通文件和目录给出固定的 mode，uid/gid 使用挂载时约定的默认身份，inode/nlink 等字段也按 VFS 需要合成。这样上层程序即使访问 FAT32，也能像访问 ext4 一样拿到一份 Linux 风格的 stat 结果。

=== 内存文件系统

tmpfs 的核心需求是：快速创建销毁文件，不访问磁盘，支持 /dev/shm 和 /tmp 这类高频场景。moss 实现的 `MemoryFs` 在 inode 管理和文件内容存储上都做了针对性的设计。

inode 用 `slab::Slab<Arc<Inode>>` 分配。slab 分配的好处是 inode 号直接等于 slab 下标加一，这样分配和按号查找都是 O(1)，不需要维护额外的 id 映射表。

核心代码如下：

#figure(
  [
    ```rust
    pub struct MemoryFs {
        // Slab 像一张可复用空槽的 inode 表。
        inodes: Mutex<Slab<Arc<Inode>>>,
        // ...
    }

    impl MemoryFs {
        fn get(&self, ino: u64) -> Arc<Inode> {
            // inode 号从 1 开始，Slab 下标从 0 开始。
            self.inodes.lock()[ino as usize - 1].clone()
        }
    }
    ```
  ],
  caption: [MemoryFs 用 Slab 分配 inode 并合成 inode 号],
) <lst:memoryfs-slab>

因此，创建文件时只需要从 slab 取一个空槽，空槽下标加一就是对外暴露的 inode 号；之后按 inode 号查找时再减一，就能直接定位回 slab 中的对象。

文件内容则反向思考：`MemoryFs` 的 inode 里不存数据，`FileContent` 只记录一个 `length: Mutex<u64>`，实际内容全部委托给上层页缓存（靠 `ALWAYS_CACHE` 标志触发）。这样做有两个好处：inode 本身非常轻量，不需要在内存中维护两份缓冲区；全零写入由页缓存检测，不分配缓存页，天然支持稀疏文件。

释放 inode 时要同时满足 `nlink == 0` 和 `Arc::strong_count == 2`（一份在 slab，一份在当前调用者）才真正释放。只满足前者不够——进程可能还持有打开的文件描述符，此时 inode 虽然没有目录项指向它，但数据不能提前销毁。

=== 伪文件系统

procfs 和 devfs 解决的是一个本质问题：/proc 里的文件内容由内核按当前状态动态生成，/dev 下的设备文件背后是驱动而不是存储，但两者都需要出现在文件系统的路径空间里，能被普通的 open/read/stat 访问。

moss 用 `SimpleFs` 框架处理这类伪文件系统，它提供 slab inode 分配和基本的目录结构，具体的目录内容和文件读取逻辑通过 `SimpleDirOps` trait 注入。procfs 和 devfs 只需要实现"这个目录下有哪些条目"和"读这个文件返回什么"，其他的框架处理。两者的目录结构如下：

#figure(
  [
    ```
    /proc                              /dev
    ├── [pid]/                         ├── null    ← SPECIAL_NODES 注入 rdev
    │   ├── stat                       ├── zero    ← SPECIAL_NODES 注入 rdev
    │   ├── status                     ├── urandom
    │   ├── maps                       ├── tty
    │   ├── pagemap                    ├── ptmx    ← PTY 主设备
    │   ├── cmdline                    ├── pts/    ← PTY 从设备目录
    │   ├── exe  ──► 可执行文件路径     │   ├── 0
    │   ├── fd/                        │   └── 1
    │   └── ns/  (pid/uts/user)        ├── loop0   ← loop 块设备
    ├── self/ ──► 当前进程 [pid]/       ├── fb0     ← 帧缓冲
    └── sys/                           └── rtc0    ← 实时时钟
        ├── kernel/
        ├── fs/   (pipe-max-size 等)
        └── vm/   (drop_caches 等)
    ```
  ],
  caption: [procfs 与 devfs 的目录结构],
) <lst:pseudofs-tree>

procfs 中 `[pid]/` 目录是动态的，每个运行中的进程对应一个。其中有个容易出错的地方：进程可能在 procfs 条目被访问的同时退出。`ProcessTaskDir` 用 `Weak<Process>` 持有进程引用，升级 `Weak` 失败时直接返回空目录，不会悬空也不会 panic。

devfs 里 `/dev/null`、`/dev/zero` 等设备文件在真实文件系统上并不存在，但 stat 时要返回正确的 major/minor 号。moss 用全局 `SPECIAL_NODES` 表处理这个问题——stat 调用时检查路径是否在表里，如果是则用表项中的 mode 和 rdev 覆盖底层节点的返回值。这个覆盖发生在 VFS 层，不需要修改任何文件系统后端。

== I/O 子系统

文件读写请求经过 `CachedFile` 后端统一处理。每个文件在全局缓存表中有一份 `CachedFileRecord`，以 `(fs_ptr, inode)` 为键索引，同一文件被多个进程或多个 fd 打开时共享同一份缓存记录，不会出现多份副本。读写请求根据文件类型走三条路径：普通磁盘文件走 cached 路径，经页缓存层访问；`O_DIRECT` 打开的文件走 direct 路径，绕过缓存直接访问块设备；tmpfs 等内存文件系统走 in-memory 路径，不涉及块设备。三条路径的分流结构如@fig:io-paths 所示。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-5-1.svg", width: 78%)
    ]
  ],
  caption: [I/O 读写三条路径],
) <fig:io-paths>

=== 页缓存

moss 采用"全局索引 + per-file LRU"的结构管理页缓存：全局表以 `(fs_ptr, inode)` 为键记录各文件的缓存入口，真正的数据页缓存在每个文件自己的 LRU 里，默认容量 256 个 4KiB 页（1MiB）。这样设置有两个好处：单个文件的大规模顺序扫描不会把其他文件的缓存挤出；对于能完整装入缓存的小文件，一旦内容被读入，后续的重复读、非顺序访问都可以直接命中缓存，无需再访问底层存储。

这个结构的关键代码如@lst:file-cache-record 所示。全局表只负责"同一个文件找到同一份缓存记录"，页数据本身仍然放在每个文件自己的 `CachedFileShared` 里。

#figure(
  [
    ```rust
    // vendor/axfs/src/highlevel/file.rs
    // ...
    type FileCacheKey = (usize, u64); // (fs_ptr, inode)，唯一定位某个文件

    struct CachedFileRecord {
        key: FileCacheKey,             // 当前缓存记录对应哪个文件
        location: Location,            // 文件在 VFS 中的位置，用来回到底层读写
        shared: Arc<CachedFileShared>, // 真正共享的页缓存和同步状态
        in_memory: bool,               // 是否是 tmpfs 这类内存文件
    }

    static FILE_CACHES:
        // 全局索引表：同一文件的多个 fd 会找到同一个 CachedFileRecord
        Lazy<RwLock<BTreeMap<FileCacheKey, Arc<CachedFileRecord>>>> = ...;
    // ...
    ```
  ],
  caption: [文件页缓存的全局索引记录],
) <lst:file-cache-record>

tmpfs 走 in-memory 路径：它也通过全局索引找到自己的缓存记录，但记录中的 `in_memory` 为 true，内部使用无界缓存，且不需要写回块设备。读未分配页时直接返回全局 `ZERO_PAGE` 中对应的内容，写入全零数据时不分配真实缓存页，能够支持稀疏文件或者全零写入。

写路径做了一项针对性的优化，避免 read-before-write。页缓存以 4KiB 为单位操作，朴素的实现在目标页缺失时会先读出完整旧页再部分覆盖——这对修改已有文件的中间段是必要的，但对新文件顺序写或 EOF 之后 append 而言，旧内容根本不存在或即将全部被覆盖，先读旧页纯属浪费。moss 用 `can_zero_fill_write_page` 判断：如果当前页完全在旧 EOF 之后，或者本次写入从页内偏移 0 开始且覆盖了页内所有旧有效内容时，直接分配清零页而不调用底层 `read_at()`，消除了新页写入前的无效读取。

脏页写回时，所有脏页按页号排序，连续页合并成一批写入，每批最多 16 页（64KiB）。如@fig:dirty-writeback 所示，相比逐页提交，顺序写场景下的块设备请求数量可以明显减少，对 fsync、close/drop 等需要落盘的路径尤为有效。

#figure(
  [
    #align(center)[
      #image("./pictrues/图-5-5-2.svg", width: 82%)
    ]
  ],
  caption: [脏页按页号排序后合并为批次写回],
) <fig:dirty-writeback>

O_DIRECT 读写绕过页缓存，但同一文件可能同时有 cached fd 在访问，如果进程 A 用 cached I/O 写了数据，还在缓存里没落盘，此时进程 B 又要用 direct I/O 读同一段，如果不处理，B 可能读到旧数据。为维护一致性，moss 在 direct 读前先将目标范围内的 dirty cached 页写回，direct 写后使对应干净页失效，这样两条路径就不会读到不一致的内容。

=== 元数据缓存

频繁的 stat 调用和读写带来的时间戳更新如果每次都穿透底层文件系统，会在 I/O 密集场景产生大量元数据写。如果一个任务只是想读文件，却要附加一次写的任务，就会产生很多无用的开销。为解决这一问题，moss 在 VFS 层引入 inode 元数据缓存，以 `(fs_ptr, inode)` 为键将 size、atime、mtime、ctime 缓存在内存中。目录的 nlink 随子节点增删变化，缓存容易读到过期值，被排除在外；`NON_CACHEABLE` 节点（procfs、sysfs 等）内容动态生成，同样不进入缓存。

元数据缓存的核心字段如@lst:metadata-cache 所示。它只保留当前报告需要理解的字段：一份缓存后的 `metadata`，以及两个表示"还有东西没提交到底层"的 dirty 标记。

#figure(
  [
    ```rust
    // vendor/axfs-ng-vfs/src/node/mod.rs
    // ...
    type InodeMetadataCacheKey = (usize, u64); // (fs_ptr, inode)

    struct InodeMetadataCache {
        metadata: Metadata,            // 当前内存里缓存的 size/atime/mtime/ctime 等
        dirty_size: bool,              // 文件大小是否已经改了但还没提交
        dirty_update: MetadataUpdate,  // 时间戳、权限、所有者等待提交更新
    }

    static INODE_METADATA_CACHES:
        // 全局元数据缓存表：同一 inode 的 stat/fstat 共享这一份结果
        Lazy<Mutex<BTreeMap<InodeMetadataCacheKey, SharedInodeMetadataCache>>> = ...;
    // ...
    ```
  ],
  caption: [inode 元数据缓存记录],
) <lst:metadata-cache>

元数据更新采用延迟提交策略：各字段的修改先合并到 `dirty_update`，不立刻落盘；sync 或文件关闭时统一提交，把多次独立的元数据写合并为一次。atime 更新遵循 relatime 策略：只有 atime 早于 mtime 或 ctime、或距上次更新超过 24 小时，读操作才会触发 atime 更新，大多数只读访问因此不再产生元数据写。`MS_NOATIME` 和 `MS_STRICTATIME` 挂载标志可以覆盖这个策略。

=== 预读

预读在应用程序实际访问页面之前提前将文件内容放入页缓存，降低后续访问的等待时间。

StarryOS 基线的 `fadvise(WILLNEED)` 只做参数检查，不实际预读。moss 补全了这条路径：`sys_readahead()` 和 `fadvise(POSIX_FADV_WILLNEED)` 找到文件对应的 cached backend，按请求范围同步地将页面放入缓存，单次最多 1MiB。direct I/O 场景保持 no-op，符合绕过缓存的语义。

mmap 访问时每次 page fault 只处理当前缺失页。moss 在 fault 后检查 `last_fault_page`：若当前页号比上次大 1，说明是顺序扫描，额外将后续 4 页预热入缓存。预读只把文件页放入页缓存，不建立页表映射，后续 fault 命中缓存时等待时间更短，且不影响随机访问场景下单次 fault 的延迟。

共享匿名映射的物理页采用懒分配：初始化时只创建占位槽，首次 fault 时才申请物理页，未被访问的页不占用真实内存。

=== I/O 多路复用

服务器程序需要同时等待多个 fd 上的事件，轮询是一种做法但 CPU 的开销极大。正确的实现应该是：没有事件时挂起任务，有事件时精准唤醒。

moss 实现了 select、poll 和 epoll 三种接口。select 和 poll 的实现相对直接：通过 `block_on(timeout(poll_io(...)))` 把当前任务交给调度器，等到有 fd 就绪或超时时再返回，全程不占用 CPU。

epoll 需要更精细的控制，因为它要在 fd 就绪时立刻通知，而不是等调用者再次轮询。moss 的实现里，每个被监视的 fd 对应一个 `EpollInterest`，持有一个实现了 `std::task::Wake` 的 `InterestWaker`。当底层文件对象（pipe、socket、设备等）的状态变化时，它调用 `InterestWaker::wake()`，后者把这个 interest 推入 epoll 的 ready queue，并唤醒阻塞在 `epoll_wait` 上的任务。整条路径是事件驱动的，没有任何轮询。

epoll 的核心状态可以见@lst:epoll-inner 这一段：一张"关注表"记录用户关心哪些 fd，一个 ready queue 记录已经就绪的 fd，再加一个 `PollSet` 用来唤醒睡在 `epoll_wait` 上的任务。

#figure(
  [
    ```rust
    // kernel/src/file/epoll.rs
    // ...
    struct EpollInner {
        // 关注表：key 是被监听的 fd，value 是该 fd 关心的事件和触发模式
        interests: SpinNoPreempt<HashMap<EntryKey, Arc<EpollInterest>>>,
        // 就绪队列：底层 fd 有事件时，把对应 interest 放进这里
        ready_queue: SpinNoPreempt<VecDeque<Weak<EpollInterest>>>,
        // 等待队列：ready_queue 从空变为非空时唤醒 epoll_wait
        poll_ready: PollSet,
    }
    // ...
    ```
  ],
  caption: [epoll 的关注表与就绪队列],
) <lst:epoll-inner>

epoll 还支持三种触发模式：

#figure(
  table(
    columns: (1.2fr, 2.8fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([模式], [行为]),
    [LT（电平触发）], [fd 就绪时持续上报，直到数据被读尽，默认模式],
    [ET（边沿触发）], [仅在状态从未就绪变为就绪时通知一次，要求应用读尽数据],
    [EPOLLONESHOT], [触发一次后自动撤除监听，需显式重新 arm 才能继续接收事件],
  ),
  caption: [epoll 的三种触发模式],
) <tab:epoll-modes>

嵌套 epoll（把一个 epoll fd 加入另一个 epoll）需要防止形成环形等待。moss 在 `epoll_ctl` 时递归检查，深度上限 5 层，同时记录已访问的实例指针来发现环。

`ppoll` 和 `pselect6` 需要原子地切换信号掩码然后进入等待——如果先换掩码再等待，中间有个窗口期可能丢失信号。moss 在进入 `block_on` 前完成掩码切换，返回时恢复，保证原子语义。

== 文件元数据与权限

=== 元数据查询

stat 系列（stat/fstat/lstat）返回固定字段集合。moss 参考 Linux 语义实现了扩展版本 statx，在兼容 stat 全部字段的基础上增加了两类能力：文件系统无法提供的字段（如 `stx_btime` 创建时间）可以用 `STATX_ATTR_*` 标志标记为"不可用"而不是返回错误；`stx_mnt_id` 返回挂载 ID，让调用者知道这个文件属于哪个挂载点。`SPECIAL_NODES` 表中的设备文件在 stat 时用表项值覆盖 mode 和 rdev。

#figure(
  table(
    columns: (1.3fr, 1.2fr, 2.8fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([接口], [字段集合], [moss 实现要点]),
    [`stat/fstat/lstat`], [固定字段集], [设备文件的 mode/rdev 由 `SPECIAL_NODES` 表覆盖],
    [`statx`], [可选扩展字段], [`STATX_ATTR_*` 标记不可用字段；`stx_mnt_id` 报告挂载点 ID],
  ),
  caption: [stat 与 statx 的能力对比],
) <tab:statx>

=== 权限检查

`access` 和 `faccessat2` 看起来只是查 mode 位，实际要处理几条叠加规则，还要区分使用哪套 uid。moss 把这些都做全了：

#figure(
  table(
    columns: (1.2fr, 1fr, 2.5fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([规则 / 接口], [返回值 / uid], [说明]),
    [父目录搜索权限], [EACCES], [路径每一级父目录都须通过 execute bit，缺一即拒],
    [只读挂载写请求], [EROFS], [写权限被挂载标志拦下时返回 EROFS，不是 EACCES],
    [root 执行位要求], [EACCES], [root 执行文件须至少一个 x 位，与普通用户不同],
    [`access`], [real uid/gid], [反映调用者真实身份，setuid 程序中是原始调用者],
    [`faccessat2`+`AT_EACCESS`], [effective uid/gid], [反映当前运行身份，setuid 程序中是文件所有者],
  ),
  caption: [权限检查的叠加规则与 uid 语义],
) <tab:access-rules>

StarryOS 不区分 real 与 effective uid，moss 补上了这一差异。

inode flags（`FS_IMMUTABLE_FL`、`FS_APPEND_FL` 等）不依赖文件系统后端，moss 用全局 `INODE_FLAGS` 表按路径存储，`FS_IOC_GETFLAGS`/`FS_IOC_SETFLAGS` ioctl 读写，stat 和写操作时查表叠加到检查逻辑里。xattr 在 MemoryFs 上有完整实现，ext4 通过 lwext4 接口透传。

== 文件事件通知

用户程序有时需要知道文件系统上发生了什么——备份工具要感知哪些文件被修改，安全软件要在文件被访问时介入。Linux 提供了两套机制，能够覆盖不同的使用场景，moss 对此也进行了实现。

=== inotify

inotify 面向具体文件或目录的变化监视。用户态对目标路径添加 watch，之后 create、delete、modify、attrib、move 等操作产生的事件进入事件队列，通过 read 取出。移动操作会产生成对的 IN_MOVED_FROM 和 IN_MOVED_TO 事件，两者通过 cookie 字段关联，让接收方能识别是同一次 rename。

实现上，一个 inotify fd 维护两张表：`watches` 记录用户关注哪些 inode，`events` 保存已经发生但尚未被 read 取走的事件。

#figure(
  [
    ```rust
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Target {
        dev: u64,     // 文件系统设备号
        ino: u64,     // inode 号，和 dev 一起唯一定位目标
        is_dir: bool, // 事件目标是否是目录
    }

    #[derive(Clone)]
    struct Watch {
        wd: c_int,       // 用户态看到的 watch descriptor
        target: Target,  // 被监视的文件或目录
        mask: u32,       // 关注的事件集合，如 IN_CREATE/IN_DELETE
    }

    struct Event {
        wd: c_int,              // 事件对应哪个 watch
        mask: u32,              // 实际发生的事件
        cookie: u32,            // rename 时关联 MOVED_FROM/MOVED_TO
        name: Option<String>,   // 目录事件中的子项名称
    }

    pub struct InotifyFd {
        watches: Mutex<Vec<Watch>>,       // 当前 fd 上注册的所有 watch
        events: Mutex<VecDeque<Event>>,   // 等待用户态 read 的事件队列
        poll: PollSet,                    // 新事件到达时唤醒 read/poll
    }
    ```
  ],
  caption: [inotify：watch 表和事件队列],
) <lst:inotify-structs>

@fig:inotify-flow 展示了 inotify 从注册 watch 到用户态取出事件的完整数据流，其中 rename 产生的 IN_MOVED_FROM 与 IN_MOVED_TO 通过相同的 cookie 值关联为一对。moss 在 VFS 的创建、删除、重命名和属性更新路径上插入通知点，卸载时向相关 watch 发送 IN_UNMOUNT。

#figure(
  [
    #align(center)[
      #image("./pictrues/图5-7-1.svg", width: 90%)
    ]
  ],
  caption: [inotify 事件投递流程],
) <fig:inotify-flow>

=== fanotify

fanotify 解决的是 inotify 解决不了的两个问题：监视范围更大（可以作用在单个 inode 、整个挂载点乃至整个filesystem ），以及能在访问发生时主动介入，而 inotify 更像是一种事后通知。

mark 可以挂在单个文件、目录或整个挂载点上，mask 指定关注的事件类型。FID 模式下，事件报告中携带文件系统标识符和文件句柄，监听者无需持有路径就能定位文件——这对跨挂载点的全局监视很重要，因为文件可能在监听期间被移动过。

fanotify 的核心也是“关注集合 + 事件队列”，但 mark 的作用范围更丰富，事件中还可能携带文件句柄、子项名称和权限请求编号。

#figure(
  [
    ```rust
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FanotifyMarkKind {
        Inode,      // 监视单个文件或目录
        Mount,      // 监视整个挂载点
        Filesystem, // 监视同一文件系统
    }

    struct FanotifyMark {
        target: FanotifyTarget, // 被监视的 inode/挂载点/文件系统
        path: Option<String>,   // 需要按路径报告时保留别名
        kind: FanotifyMarkKind, // mark 的作用范围
        mask: u64,              // FAN_OPEN/FAN_ACCESS/FAN_CREATE 等
        ignored_mask: u64,      // 临时忽略的事件集合
    }

    struct FanotifyEvent {
        mask: u64,                      // 实际触发的 fanotify 事件
        file: Option<Arc<dyn FileLike>>, // 需要返回 fd 时保留文件对象
        fid: Option<FanotifyTarget>,    // FID 模式下报告的文件身份
        name: Option<String>,           // 目录项事件的子项名称
        permission_id: Option<u64>,     // 权限事件等待用户态响应
    }

    pub struct FanotifyFd {
        marks: Mutex<Vec<FanotifyMark>>,        // 当前监听实例上的所有 mark
        events: Mutex<VecDeque<FanotifyEvent>>, // 等待读取的事件队列
        permissions: Mutex<Vec<FanotifyPermission>>, // 尚未 allow/deny 的权限事件
        poll: PollSet,                          // 唤醒 read/poll 等待者
    }
    ```
  ],
  caption: [fanotify：mark、事件队列与权限等待表],
) <lst:fanotify-structs>

权限事件（`FAN_OPEN_PERM`、`FAN_ACCESS_PERM`）是 fanotify 最特殊的部分：内核在文件被打开或访问时，先等待监听进程的响应，再决定是否让操作继续。触发进程在等待期间阻塞，监听进程通过写入 allow 或 deny 响应。安全软件（防病毒、DLP）依赖这个机制在文件访问路径上做拦截。@fig:fanotify-perm 展示了这一阻塞-响应流程中三方的交互时序。

#figure(
  [
    #align(center)[
      #image("./pictrues/图5-7-2.svg", width: 90%)
    ]
  ],
  caption: [fanotify 权限事件时序],
) <fig:fanotify-perm>

moss 的 inotify 和 fanotify 实现覆盖了打开、读写、创建、删除、重命名、关闭等路径上的通知点。

=== 无监听快速路径

inotify 和 fanotify 的实现需要在 read、write、open、close 等系统调用路径上插入事件投递逻辑。没有注册监听者时，这些逻辑不应带来任何额外开销。

moss 用全局计数和掩码在事件投递入口做早期过滤：inotify 维护全局 watch 计数，计数为零时通知函数立即返回；fanotify 维护全局兴趣掩码，掩码对当前事件无命中时跳过后续所有处理。两处检查都只需一次原子读，无监听时不进入任何路径解析、mark 匹配或事件队列操作。一旦用户注册了 watch 或 mark，仍会走完整的匹配和投递流程，功能语义不受影响。

== 性能对比

为验证文件系统 I/O 优化效果，使用 iozone 进行吞吐性能测试。所有测试均在本地 QEMU 虚拟机（RISC-V 架构）上运行，单核，1GB 内存。对比对象包括优化前的 StarryOS、优化后的 moss，以及 Ubuntu 26.04 基准。以下选取部分有代表性的测试项进行对比（单位：kB/sec）；为便于观察相对差距，后文图中将 Ubuntu 作为 100% 参考线，展示优化前后相对 Ubuntu 的比例。

#figure(
  table(
    columns: (1.5fr, 1fr, 1fr, 1fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([测试场景], [优化前], [优化后], [Ubuntu]),
    [顺序写\ （initial writers）], [3524], [33221], [51922],
    [顺序读\ （readers）], [13136], [81470], [104754],
    [顺序重写\ （rewriters）], [7471], [81639], [72760],
    [随机读\ （random readers）], [11082], [57668], [57175],
    [随机写\ （random writers）], [7141], [59978], [59817],
    [反向读\ （reverse readers）], [10743], [58824], [67361],
    [pread 并发读], [14184], [108348], [125270],
    [pwrite 并发写], [3724], [28263], [39335],
  ),
  caption: [iozone 性能对比（glibc，本地文件系统）],
) <tab:iozone-glibc>

如 @tab:iozone-glibc 所示，glibc 环境下优化后的 moss 在顺序读写、随机读写和并发读写场景中均有明显提升。@fig:iozone-glibc-opt 进一步以 Ubuntu 为参考线展示了优化幅度，其中顺序重写、随机读和随机写已经达到或超过 Ubuntu 同场景基准。

#figure(
  [
    #align(center)[
      #image("./pictrues/iozone_glibc_优化对比.png", width: 95%)
    ]
  ],
  caption: [iozone 优化前后对比（glibc，Ubuntu 为 100% 参考线）],
) <fig:iozone-glibc-opt>

#figure(
  table(
    columns: (1.5fr, 1fr, 1fr, 1fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([测试场景], [优化前], [优化后], [Ubuntu]),
    [顺序写\ （initial writers）], [3524], [26407], [44818],
    [顺序读\ （readers）], [13136], [62733], [98518],
    [顺序重写\ （rewriters）], [7471], [62254], [69316],
    [随机读\ （random readers）], [11082], [43813], [91905],
    [随机写\ （random writers）], [7141], [51344], [57648],
    [反向读\ （reverse readers）], [10743], [54217], [78861],
    [pread 并发读], [14184], [88315], [141988],
    [pwrite 并发写], [3724], [28192], [49303],
  ),
  caption: [iozone 性能对比（musl，本地文件系统）],
) <tab:iozone-musl>

musl 环境的整体趋势与 glibc 一致：优化前的 StarryOS 与 Ubuntu 基准差距较大，优化后的 moss 在所有测试项上均显著提升。@fig:iozone-musl-opt 展示了 musl 环境下的相对比例，可以看到顺序写、顺序读、随机写和并发写等场景已大幅缩小与 Ubuntu 的差距。

#figure(
  [
    #align(center)[
      #image("./pictrues/iozone_musl_优化对比.png", width: 95%)
    ]
  ],
  caption: [iozone 优化前后对比（musl，Ubuntu 为 100% 参考线）],
) <fig:iozone-musl-opt>

综合两组 libc 结果来看，优化前的 StarryOS 在文件系统 I/O 吞吐测试中远低于 Ubuntu 基准；优化后，顺序写提升约 7-13 倍，顺序读提升约 5 倍，随机读写提升约 5-9 倍。优化效果主要来自页缓存容量扩展（从默认配置提升到 1MiB per-file）和统一的 LRU 管理策略，减少了小块 I/O 中的重复访盘和元数据访问开销。

== 本章小结

本章介绍了 moss 文件系统从底层存储到用户接口的整体设计。挂载子系统支持 bind mount、命名空间隔离与新式挂载 API，VFS 层通过 `Location` 和 `NodeFlags` 将路径解析与挂载切换解耦，使上层调用方无需感知挂载点跳转。I/O 子系统在 StarryOS 基础上引入 per-file 页缓存与元数据缓存，并在写路径、脏页写回、显式预读和 mmap fault 预热四个维度做了针对性优化，减少小块 I/O 中的无效读写和元数据开销。文件事件通知提供了 inotify 和 fanotify 的完整实现，在无监听者时通过全局计数与兴趣掩码早期过滤，使事件投递逻辑不干扰正常 I/O 路径的性能。
