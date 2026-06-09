#import "../template.typ": evidence

= 附录 C：关键数据结构摘要

#table(
  columns: (2fr, 3fr, 5fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*结构*], [*路径*], [*摘要*],
  [`AddrSpace`], [`kernel/src/mm/aspace/mod.rs`], [用户虚拟地址空间：地址范围、VMA 集合和页表。],
  [`Backend`], [`kernel/src/mm/aspace/backend/mod.rs`], [映射后端枚举：Linear、Cow、Shared、File。],
  [`Thread`], [`kernel/src/task/mod.rs`], [线程级状态：进程共享数据、signal、timer、robust list、退出事件、用户内存访问标记。],
  [`ProcessData`], [`kernel/src/task/mod.rs`], [进程共享状态：Process、地址空间、scope、rlimit、signal、futex、namespace、uid/gid。],
  [`ProcessNamespaces`], [`kernel/src/task/mod.rs`], [pid/uts/user namespace inode 和父子关系状态。],
  [`FileLike`], [`kernel/src/file/mod.rs`], [文件描述符对象 trait，统一 read/write/stat/ioctl/poll/path/status flags。],
  [`Kstat`], [`kernel/src/file/mod.rs`], [内核 stat 结构，可转换为 Linux `stat` 和 `statx`。],
  [`DeviceOps`], [`kernel/src/pseudofs/device.rs`], [设备节点操作：read/write/ioctl/poll/mmap。],
  [`FutexKey` / `FutexTable`], [`kernel/src/task/futex.rs`], [按私有地址、共享页或文件映射定位 futex wait queue。],
  [`MessageQueue`], [`kernel/src/syscall/ipc/msg.rs`], [SysV message queue 的消息、统计和权限结构。],
  [`ShmInner`], [`kernel/src/syscall/ipc/shm.rs`], [SysV shared memory 段状态、共享页、attach 关系和 `ShmidDs`。],
)

== 代码依据

#evidence((
  [`数据结构来源`], [`kernel/src/mm`, `kernel/src/task`, `kernel/src/file`, `kernel/src/pseudofs`, `kernel/src/syscall/ipc`], [上表仅列源码中可以直接定位的结构。],
))
