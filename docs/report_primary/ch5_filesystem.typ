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

每次目录查找完成后，VFS 自动调用 `resolve_mountpoint()` 检查是否到达挂载点，如果是则切换到对应文件系统的根。这个操作对调用方透明，穿越挂载点和普通的目录查找在接口上没有区别，可以让所有系统调用都共享同一套正确路径语义。

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

inode 用 `slab::Slab<Arc<Inode>>` 分配。slab 分配的好处是 inode 号直接等于 slab 下标加一，分配和按号查找都是 O(1)，不需要维护额外的 id 映射表。

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

=== 页缓存

文件 I/O 如果每次都直接读写块设备，性能无法接受。但如果每个文件各自维护缓存，多个进程打开同一个文件时就会有多份副本，内存浪费，写入之间还会互相不可见。全局页缓存是解决这两个问题的标准方法：把文件数据统一缓存，以文件系统对象和页号为键，跨进程共享同一份物理页。

全局页缓存的 LRU 组织结构和它与 mmap 的联动在内存管理章节已经说明，本节关注文件系统侧的读写策略。

#figure(
  table(
    columns: (1.2fr, 1.3fr, 1fr, 2fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([层级], [容量], [淘汰], [适用场景]),
    [全局缓存], [16384 页（64 MiB）], [LRU], [所有磁盘文件共享],
    [per-file 缓存], [无界], [LRU], [tmpfs 等内存文件系统],
  ),
  caption: [两级页缓存结构],
) <tab:page-cache>

tmpfs 用独立的 per-file 缓存而不进入全局池，原因是它的内容本来就在内存里，不涉及磁盘淘汰逻辑，也不需要统一的容量限制。

脏页写回是另一个需要考虑的点。朴素的做法是每页脏了就提交一次写入，但这会产生大量零散的块设备请求。moss 的写回路径先把脏页按页号排序，找出连续范围后合并为一次 I/O，每次最多 16 页（64 KiB）。顺序写场景下，这和逐页写回相比请求数量差距明显。

O_DIRECT 是个边界情况：调用者绕过缓存直接访问块设备，但同时可能有其他进程在用 cached I/O 访问同一个文件。moss 的做法是，O_DIRECT 读写前先 flush 目标范围的脏缓存页，完成后 invalidate 对应的干净页。这保证两种访问路径不会读到不一致的内容。

atime 的更新策略也影响 I/O 开销。默认启用 `relatime`：只有当 atime 早于 mtime 或 ctime，或者距上次更新超过 24 小时时，才在读操作后更新 atime。这让大多数只读访问不再触发元数据写，把"读文件"从"读数据 + 写元数据"变成纯粹的读操作。`MS_NOATIME` 和 `MS_STRICTATIME` 挂载标志可以覆盖这个行为。

=== I/O 多路复用

服务器程序需要同时等待多个 fd 上的事件，轮询是一种做法但 CPU 开销不可接受。正确的实现是：没有事件时挂起任务，有事件时精准唤醒。

moss 实现了 select、poll 和 epoll 三种接口。select 和 poll 的实现相对直接：通过 `block_on(timeout(poll_io(...)))` 把当前任务交给调度器，等到有 fd 就绪或超时时再返回，全程不占用 CPU。

epoll 需要更精细的控制，因为它要在 fd 就绪时立刻通知，而不是等调用者再次轮询。moss 的实现里，每个被监视的 fd 对应一个 `EpollInterest`，持有一个实现了 `std::task::Wake` 的 `InterestWaker`。当底层文件对象（pipe、socket、设备等）的状态变化时，它调用 `InterestWaker::wake()`，后者把这个 interest 推入 epoll 的 ready queue，并唤醒阻塞在 `epoll_wait` 上的任务。整条路径是事件驱动的，没有任何轮询。

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

== 挂载子系统

=== 传统挂载接口

`sys_mount` 不只是"把文件系统接到路径上"，它需要处理几种不同的操作模式：

#figure(
  table(
    columns: (1.4fr, 2.6fr),
    inset: 6pt,
    stroke: 0.5pt + gray,
    table.header([标志], [含义]),
    [`MS_BIND`], [bind mount：让已有目录树在另一个路径下可见，不复制任何文件],
    [`MS_MOVE`], [迁移挂载点：将挂载从一个路径移动到另一个路径，不卸载重挂],
    [`MS_REMOUNT`], [修改现有挂载的标志，如将读写挂载改为只读],
    [`MS_PRIVATE`], [命名空间隔离：本挂载点的变动不向父/子命名空间传播],
  ),
  caption: [sys_mount 支持的操作模式],
) <tab:mount-flags>

bind mount 是容器化场景里常用的操作，让宿主机的某个目录在容器内以另一个路径暴露。moss 的实现直接复用源挂载点对象，新的 `Location` 指向同一个 `Mountpoint`，device_id 天然相同，`stat()` 的结果与 Linux 一致，不需要特殊处理。

`sys_umount2` 卸载时会向所有关注该挂载点的 inotify watch 发送 IN_UNMOUNT 事件，这样监视 /mnt 的应用在文件系统被卸载时能收到通知。

=== 新式挂载 API

Linux 5.2 引入了基于文件描述符的挂载 API，解决的问题是：传统 `sys_mount` 需要调用者同时持有挂载能力和目标路径的权限，这在容器场景里很难满足——容器进程可能没有宿主机路径的权限，但应该能在自己的命名空间里做挂载。

新 API 把挂载分解为几个步骤，每个步骤返回一个 fd：

#figure(
  [
    ```
    fd  = fsopen("ext4")           // 创建文件系统上下文
          fsconfig(fd, "source", "/dev/loop0")  // 设置设备
          fsconfig(fd, "cmd", "create")          // 完成配置
    mfd = fsmount(fd, 0, 0)        // 实例化为挂载对象
          move_mount(mfd, "/mnt")  // 接入目录树
    ```
  ],
  caption: [新式挂载 API 的调用流程],
) <lst:new-mount-api>

分步骤的好处是：`fsopen` 和 `fsconfig` 可以在无特权的命名空间里执行，`move_mount` 才需要对目标路径的权限。`open_tree` 则反向工作，把一棵挂载子树克隆为 fd，可以传给另一个命名空间使用。moss 完整实现了这五个调用，通过 LTP 对应测试集。

== 文件元数据与权限

=== 元数据查询

stat 系列（stat/fstat/lstat）返回固定字段集合。statx 是 Linux 4.11 扩展的版本，增加了两类信息：文件系统无法提供的字段（如 `stx_btime` 创建时间）可以用 `STATX_ATTR_*` 标志标记为"不可用"而不是返回错误；`stx_mnt_id` 返回挂载 ID，让调用者知道这个文件在哪个挂载点上。moss 实现了完整的 statx，`SPECIAL_NODES` 表中的设备文件在 stat 时用表项值覆盖 mode 和 rdev。

=== 权限检查

`access` 和 `faccessat2` 的检查比想象中复杂。表面上是"看文件的 mode 位"，但实际上要处理几个叠加的规则：

路径中每一级父目录都需要通过搜索权限（execute bit）检查，缺少任意一级就返回 EACCES。目标文件在只读挂载上时，写权限请求返回 EROFS 而不是 EACCES，原因不同，错误码也不同。root 用户执行文件时，目标文件须带有至少一个执行位（三组权限中任意一个 x 位为 1），与普通用户的检查逻辑不同。

`access` 用 real uid/gid 判断，`faccessat2` 加了 `AT_EACCESS` 标志后改用 effective uid/gid。两者差别在 setuid 程序里才显现：setuid 程序运行时 effective uid 是文件所有者，real uid 是调用者。StarryOS 的基础实现不区分这两者，moss 在此处做了补强，让 `access` 行为与 Linux 手册的描述吻合。

inode flags（`FS_IMMUTABLE_FL`、`FS_APPEND_FL` 等）不依赖文件系统后端的支持，moss 用全局 `INODE_FLAGS` 表按路径存储这些标志，`FS_IOC_GETFLAGS`/`FS_IOC_SETFLAGS` ioctl 读写此表，stat 和写操作时从表里查出标志叠加到检查逻辑里。

xattr 在 MemoryFs 上有完整实现，ext4 通过 lwext4 接口透传。

== 文件事件通知

用户程序有时需要知道文件系统上发生了什么——备份工具要感知哪些文件被修改，安全软件要在文件被访问时介入。Linux 提供了两套机制，覆盖不同的使用场景。

=== inotify

inotify 面向具体文件或目录的变化监视。用户态对目标路径添加 watch，之后 create、delete、modify、attrib、move 等操作产生的事件进入事件队列，通过 read 取出。移动操作会产生成对的 IN_MOVED_FROM 和 IN_MOVED_TO 事件，两者通过 cookie 字段关联，让接收方能识别是同一次 rename。

moss 在 VFS 的创建、删除、重命名和属性更新路径上插入通知点，卸载时向相关 watch 发送 IN_UNMOUNT。

=== fanotify

fanotify 解决的是 inotify 解决不了的两个问题：监视范围更大（可以作用在整个挂载点上），以及能在访问发生时主动介入。

mark 可以挂在单个文件、目录或整个挂载点上，mask 指定关注的事件类型。FID 模式下，事件报告中携带文件系统标识符和文件句柄，监听者无需持有路径就能定位文件——这对跨挂载点的全局监视很重要，因为文件可能在监听期间被移动过。

权限事件（`FAN_OPEN_PERM`、`FAN_ACCESS_PERM`）是 fanotify 最特殊的部分：内核在文件被打开或访问时，先等待监听进程的响应，再决定是否让操作继续。触发进程在等待期间阻塞，监听进程通过写入 allow 或 deny 响应。安全软件（防病毒、DLP）依赖这个机制在文件访问路径上做拦截。

moss 的 fanotify 实现覆盖了打开、读写、创建、删除、重命名、关闭等路径上的通知点，约 2200 行。没有活跃监听时，通知路径上只有一次轻量的标志检查，普通文件操作不承担额外开销。
