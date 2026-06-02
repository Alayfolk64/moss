# moss 相对 StarryOS 的增量贡献总结

## 对比范围

- 基线仓库：`https://github.com/Starry-OS/StarryOS.git`
- 基线提交：`upstream/main = 2e075acc`
- moss 提交：`main = 6d54abf5`
- 对比方法：以 Git 提交关系和文件差异为主，使用 `git fetch Starry-OS/StarryOS main:refs/remotes/upstream/main` 后比较 `upstream/main..main`。
- 总体规模：moss 在基线之后新增了 206 个提交。排除大体量 vendored 依赖和第三方 C 代码后，核心工程仍有 `146 files changed, 140974 insertions(+), 2035 deletions(-)`。

> 说明：moss 中有大量 `vendor/` 和 `third_party/` 文件，这是重要的工程化增量，主要用于离线构建、固定依赖版本、适配评测环境。下文会把这部分和内核功能增量分开说明。

## 总体定位

moss 的主要贡献，是把官方 StarryOS 基线推进为一个面向 OS 比赛评测和 LTP syscall 测试的 Linux 兼容型内核。增量不只是补几个 syscall，而是沿着完整运行链路做了系统性扩展：

1. 构建阶段固定工具链、离线依赖、双架构产物和镜像运行方式。
2. 启动入口改为可自动准备 musl/glibc 运行环境并驱动 LTP。
3. ELF 加载、动态链接器、脚本解释器和 busybox fallback 更贴近真实 Linux 用户态。
4. 进程、权限、信号、等待、定时器、内存、文件、网络等子系统围绕 LTP 行为补齐语义。
5. `/proc`、`/sys`、`/dev` 等伪文件系统补充大量用户程序探测所需接口。
6. 最后在 init 退出后统一卸载、flush 文件系统并关机，形成可重复评测闭环。

## 1. 构建与工程化入口

基线 StarryOS 依赖在线 crates 和默认 ArceOS 工具链流程。moss 在构建入口上做了较大改造：

- `Cargo.toml` 和 `kernel/Cargo.toml` 固定 ArceOS/Starry 相关 crate 版本，加入 `vendor/` 和 `third_party/` 的本地 patch，避免评测机因网络或版本漂移导致构建失败。
- `make/cargo.mk` 增加 vendored source 配置、`--locked`、`--offline`，强制离线、可复现构建。
- `make/deps.mk` 不再静默联网安装工具，而是优先使用本地 `.tools` 和 `vendor/axconfig-gen`、`vendor/cargo-binutils`。
- 顶层 `Makefile` 固定 `nightly-2025-05-20`，默认 release 构建，默认关闭 DWARF 和 NET，降低镜像体积和不确定性。
- 新增 `kernel-rv`、`kernel-la`、`all` 目标，把 RISC-V 和 LoongArch64 内核分别封装成比赛需要的 `kernel-rv`、`kernel-la`。
- 新增 `make/kernel-rv-wrap.ld`、`make/kernel-la-wrap.ld`，把裸 kernel binary 包装成目标架构 ELF。
- 新增 `prepare-sdcard-rv`、`prepare-sdcard-la`、`rv-shell`、`la-shell`、`debug-rv`、`debug-la`、`contest-run-rv` 等目标，覆盖普通运行、交互 shell、GDB 调试和评测运行。
- `run_all.sh`、`run_rv.sh`、`run_la.sh`、`shell_rv.sh`、`shell_la.sh`、`summary.sh`、`total.sh`、`scripts/extract_ltp_summary.sh` 等脚本形成自动构建、注入运行库、启动 QEMU、收集输出的评测流水线。

这部分贡献的价值在于：moss 不再只是“能在开发机上跑”，而是尽量把工具链、依赖、镜像和运行命令固定下来，让 RV/LA 两个架构在评测环境中可复现。

## 2. Rust app 入口与 init 选择

官方基线的入口更偏简单启动。moss 在 `src/main.rs` 中把用户态入口组织成可评测模式：

- 默认 `CMDLINE` 为 `["/musl/busybox", "sh", "-c", INIT_SCRIPT]`，优先进入 busybox shell 执行内嵌 init 脚本。
- `INIT_SCRIPT` 可根据 feature 在普通评测 init、交互 shell init、timerfd debug init 之间切换。
- `src/ltp_testcases.sh` 被编译进内核镜像，并通过 `LTP_CASES=` 环境变量传给 init，避免评测时找不到 testcase 列表。
- `shell-init` feature 会额外注入 `STARRY_SHELL_MODE=1`，方便构建同一内核的交互调试版本。

这使 moss 的入口从“启动一个用户程序”升级为“启动一个内置测试驱动器”，后续所有 LTP 和比赛测例都从这里展开。

## 3. 内核 entry 初始化

`kernel/src/entry.rs` 是从内核到第一个用户进程的关键入口。moss 的增量包括：

- 启动时先挂载全部伪文件系统，再启动全局 alarm task，为后续 timer、itimer、sleep、poll 等行为提供基础。
- 对 init 可执行文件做 fallback：`/musl/busybox`、`/glibc/busybox`、`/bin/busybox` 可互相替代，避免镜像中不同 libc 目录布局导致启动失败。
- 创建空用户地址空间，按架构决定是否复制 kernel page table，再加载用户 app。
- 为 init 进程创建 `ProcessData`、`Thread`、`FD_TABLE` 作用域和标准输入输出。
- 把 N_TTY 绑定到 init 进程，补足交互 shell 和 tty 行为。
- init 退出后，调用 rootfs `unmount_all()` 和 `flush()`，最后 `system_off()`，避免评测输出结束后内核悬挂或文件系统未落盘。

这条链路把启动、用户态、退出和关机串成闭环，是 moss 后续自动化测试能稳定结束的基础。

## 4. ELF 加载、动态链接和脚本执行

`kernel/src/mm/loader.rs` 的改造解决了比赛镜像中最常见的用户态启动问题：

- 动态链接器 fallback：当 ELF 的 `PT_INTERP` 指向 `/lib/...` 而镜像实际放在 `/glibc` 或 `/musl` 下时，会自动尝试 `/glibc + interp` 和 `/musl + interp`。
- `/bin/sh`、`/bin/busybox` fallback 到 `/musl/busybox`、`/glibc/busybox`，提升 basic、busybox、LTP 脚本兼容性。
- 对 `.sh` 脚本直接使用 `/bin/sh` fallback；对 shebang 文件解析解释器行并重组 argv。
- ELF header 加载使用缓存，减少频繁 fork/exec 下重复读取 ELF 的成本。
- 文件段映射使用 lazy COW，并记录文件长度，避免越过文件尾读取产生错误数据。
- 启动时映射 signal trampoline、用户栈和堆，保证信号返回、argv/env/auxv、动态链接器入口都能正常工作。

这部分贡献让 moss 可以运行 glibc/musl 动态程序、busybox 工具、shell 脚本和大量 LTP 二进制。

## 5. init 脚本与用户态运行环境

`src/init.sh` 从简单脚本扩展成完整评测驱动：

- 安装 busybox 到 `/bin`，补齐基础命令。
- 根据 RV/LA 架构创建动态链接器软链接，例如 LoongArch 的 `ld-linux-loongarch-lp64d.so.1`、RISC-V 的 `ld-linux-riscv64-lp64d.so.1` 和 musl loader。
- 创建 `/lib64`、`/usr/lib64` 等兼容路径。
- 初始化 `/etc/passwd`、`/etc/group`、`/etc/resolv.conf`，让权限、用户、DNS 相关测试有基本环境。
- 创建 `/lib/modules/10.0.0/build/.config`，暴露 `CONFIG_AIO`、`CONFIG_USER_NS`、`CONFIG_BSD_PROCESS_ACCT` 等配置给测试程序探测。
- 提供 `useradd`、`userdel`、`groupdel`、`mkfs.ext3`、`mkfs.ext4`、`mkfs.xfs`、`mkfs.btrfs`、`mount.ntfs` 等最小用户态 shim，减少测试因工具缺失而失败。
- 对 basic、busybox、cyclictest、iozone、iperf、libcbench、libctest、lmbench、lua、netperf、ltp 都有开关。
- LTP 执行逻辑支持从内核注入的 `LTP_CASES` 读取用例，逐条运行、打印 `RUN LTP CASE`、补齐缺失 Summary、输出 `FAIL LTP CASE ... : 0` 和时间戳。
- 同一套脚本分别在 `/musl` 和 `/glibc` 下运行，覆盖双 libc 场景。

这部分不是内核功能，但直接决定了 moss 能否在评测机上稳定、可计分地跑完目标用例。

## 6. 进程、线程、权限与等待语义

moss 对任务和进程模型做了多处 Linux 语义补齐：

- `ProcessData` 新增 real/effective/saved UID/GID、umask、ioprio、child CPU time、wait status、stop event、exit event 等状态。
- `sys_getuid/geteuid/getgid/getegid` 从固定返回 0 变为读取当前进程状态。
- 新增/完善 `getresuid`、`getresgid`、`setreuid`、`setregid`、`setresuid`、`setresgid`，包括非 root ID 切换限制和 saved id 语义。
- 新增 `capget/capset` 和 capability bounding 相关 `prctl` 支持，为权限类 LTP 提供基本模型。
- 新增 `execveat`，补齐 `AT_EMPTY_PATH`、dirfd 相关执行路径。
- 新增 `ptrace` 支持，包含 trace 状态、wait event、SIGTRAP/stop 交互，使 `ptrace02/05/06` 等测试能被正确计分。
- `wait4/waitpid/waitid` 支持 `WNOHANG`、`WUNTRACED`、`WCONTINUED`、`WNOWAIT`、`P_PIDFD`、pid group 过滤，以及 ptrace wait target。
- 阻塞 wait 被信号打断时实现 syscall restart，避免 LTP 父进程因心跳信号或 SIGCHLD 时序错误而失败。
- `clone/clone3/fork/unshare` 周边补充 namespace/time namespace 继承、clear child tid、robust list、child exit 唤醒等行为。
- `acct`、`add_key`、`keyctl`、`reboot` 等系统调用给出面向测试的最小可用实现。

这些改造把基线中大量“返回 0 或简单 stub”的进程相关行为，提升为能通过权限、等待、ptrace、namespace 探测的兼容模型。

## 7. 信号与异常处理

moss 在用户态循环和 syscall 层补齐了信号行为：

- `kernel/src/task/user.rs` 在每次进入用户态前处理 ptrace stop，返回内核后根据异常类型发送 `SIGSEGV`、`SIGBUS`、`SIGILL`、`SIGTRAP`。
- LoongArch64 misaligned exception 可尝试软件模拟，不成功再发信号。
- 用户内存 fault 时区分普通 page fault、growdown stack、mmap 文件尾写入导致的 SIGBUS。
- `pause`、`rt_sigsuspend`、`rt_sigtimedwait`、`sigaltstack` 等信号 syscall 更完整地参与阻塞/解除阻塞流程。
- job-control stop/continue 状态能被 wait 系列系统调用观察。

这部分贡献尤其影响 LTP 中 signal、wait、ptrace、mmap 类用例的稳定性。

## 8. 时间、定时器与 time namespace

`kernel/src/syscall/time.rs` 和 `kernel/src/task/timer.rs` 是 moss 的重要增量：

- `clock_gettime` 扩展支持 `CLOCK_REALTIME_ALARM`、`CLOCK_TAI`、`CLOCK_BOOTTIME_ALARM`、process/thread CPU time。
- 新增 `clock_settime`，通过 realtime offset 实现可调 realtime。
- 新增 `adjtimex`、`clock_adjtime`，带 modes 校验、权限校验和 timex snapshot。
- `gettimeofday` 支持 `timezone` 指针。
- `times()` 改为 USER_HZ tick，并统计 child CPU time。
- `setitimer/getitimer` 使用可取消、可去重的 alarm runtime，避免旧 timer 重复触发。
- 新增 POSIX timer 系列：`timer_create`、`timer_delete`、`timer_getoverrun`、`timer_gettime`、`timer_settime`。
- 新增 timerfd 文件对象和 syscall：`timerfd_create`、`timerfd_gettime`、`timerfd_settime`，支持 nonblock、absolute timer、周期 timer、poll/read 过期次数。
- 新增 time namespace 偏移：`/proc/[pid]/timens_offsets` 可读写 child monotonic/boottime offset，fork/clone 时继承。
- epoll wait 修复了零超时行为和 packed `epoll_event` 读取问题，解决 timerfd/epoll 组合测试卡死或误判。

这些改造让 sleep、poll、timerfd、POSIX timer、epoll、time namespace 等测试不再互相拖垮。

## 9. 内存管理与 mmap 语义

moss 对 `mmap` 和 COW 后端做了较深修改：

- `mmap` 支持并校验 `MAP_SHARED_VALIDATE`、`MAP_GROWSDOWN`、`MAP_LOCKED`、`MAP_FIXED`、`MAP_FIXED_NOREPLACE`、huge page flag 等。
- 对 anonymous/file-backed mapping 的 fd、offset、读写权限做 Linux 风格校验。
- 文件映射记录文件长度，避免文件尾之后读出脏数据。
- `MAP_SHARED | PROT_WRITE` 文件映射会触发 inotify modify 通知。
- 记录进程 mmap 区间，用于 `/proc/[pid]/maps` 输出 shared/private 标记。
- `MAP_GROWSDOWN` 区域在 page fault 时按需向下扩展，并能被后续 fixed mapping 阻断。
- 对某些 shared file remap 场景记录 SIGBUS 区间，写 fault 时返回 SIGBUS。
- `mremap` 支持缩小、原地扩展、`MREMAP_MAYMOVE` 迁移和数据复制。
- `mlock`、`mlock2`、`munlock`、`mlockall`、`munlockall` 增加参数校验和 `/proc/[pid]/status` 中 `VmLck` 统计。
- COW frame refcount 从 `u8` 扩展到 `usize`，避免 fork 风暴引用计数溢出。
- 对未触碰的 lazy COW 区域，fork 时不再逐页复制 page table；对只读私有文件映射可提前共享，降低 fork/exec 压力。
- page cache 新页清零，修复 mmap/read 数据污染。

这部分贡献明显提升了 fork、exec、mmap、mremap、page fault、文件映射类用例的正确性和性能。

## 10. 文件描述符、fcntl、pipe、epoll

moss 重构和补齐了文件描述符层：

- `FileLike` 增加 `status_flags`、`set_status_flags`、`proc_fdinfo`、`ioctl`、`nonblocking` 等统一接口。
- `FileDescriptor` 保留 `cloexec`，`F_GETFD/F_SETFD`、`dup/dup2/dup3`、`F_DUPFD/F_DUPFD_CLOEXEC` 语义更完整。
- `fcntl` 支持 `F_SETFL/F_GETFL`、`F_SETOWN/F_GETOWN`、`F_SETOWN_EX/F_GETOWN_EX`、`F_SETSIG/F_GETSIG`。
- 支持 POSIX record lock、BSD `flock`、file lease 的核心行为，并在 close/dup 覆盖时释放相关锁。
- `F_GETPIPE_SZ/F_SETPIPE_SZ` 支持 pipe 容量查询和扩容权限校验。
- `close_range`、`pidfd_open/getfd/send_signal` 周边行为被完善。
- pipe、socket、regular file 的 nonblock/poll 行为被统一接入 `axpoll`。
- epoll 实现 ready queue、level/edge/oneshot 触发模式、waker 注册、删除和修改兴趣集，修复 `epoll_wait` 的零超时和自添加校验。

这些变更是大量 IO 多路复用、fcntl、pipe、fork-close、epoll LTP 用例通过的基础。

## 11. 文件系统、路径解析与权限

文件系统是 moss 增量最大的区域之一：

- `resolve_at` 统一处理 dirfd、`AT_EMPTY_PATH`、`AT_SYMLINK_NOFOLLOW`，并支持 fd 指向 file/dir/其他 FileLike。
- `open/openat/creat` 补齐 `O_CREAT`、`O_EXCL`、`O_DIRECTORY`、`O_NOFOLLOW`、`O_PATH`、`O_DIRECT`、`O_TRUNC`、`O_APPEND`、mount flag 和权限检查。
- 支持 `protected_symlinks`、`protected_hardlinks`，并通过 `/proc/sys/fs/protected_*` 暴露可读写开关。
- `mkdir/mknod/link/unlink/symlink/readlink/rename/chown/chmod/utime/utimensat/sync/syncfs` 等路径类 syscall 增加权限、umask、setgid 目录继承、只读挂载、不可变/append-only flag、事件通知等行为。
- `mknod/mknodat` 支持 regular、fifo、char device、block device、socket，并记录 special node metadata。
- `ioctl` 增加 `FIONBIO`、`FS_IOC_GETFLAGS`、`FS_IOC_SETFLAGS`，同时降低常见 tty probe 的无意义告警。
- `getdents64` 和 MemoryFS readdir 使用稳定 cookie，避免目录遍历 offset 混乱。
- `stat/fstat/lstat/fstatat/statx` 补齐 uid/gid、rdev、blocks、birth time、DIO align、mount root、inode flags 等字段。
- 新增 `name_to_handle_at`、`open_by_handle_at`，通过 dev/inode 编码和 handle 记录支持 fanotify FID 类测试。
- `access/faccessat/faccessat2` 区分 real/effective id，检查父目录 execute 权限和只读文件系统。
- `statfs/fstatfs` 返回真实 mount flag、fsid，对 pipe/anon inode 返回伪 statfs。

这些改造让文件系统行为从“基本能 open/read/write”提升为能支撑权限、路径、目录、mount、stat、fanotify/inotify 的组合语义。

## 12. mount、新 mount API 与块设备

moss 对 mount 相关 syscall 做了系统补齐：

- `mount` 支持 ext2/ext3/ext4、tmpfs/ramfs，以及 xfs/btrfs/vfat/fat/msdos/exfat/ntfs 的兼容内存文件系统后端。
- 支持 `MS_BIND`、`MS_MOVE`、`MS_PRIVATE`、`MS_REMOUNT`、`MS_RDONLY`、`MS_NODEV`、`MS_NOEXEC`、`MS_NOSUID`、`MS_NOATIME`、`MS_NODIRATIME`、`MS_NOSYMFOLLOW`、`MS_STRICTATIME`。
- `umount2` 增加 flag 校验、flush、mount path 注销和 inotify unmount 事件。
- 新增新 mount API：`fsopen`、`fsconfig`、`fsmount`、`move_mount`，以 fs context fd 和 detached mount fd 形式模拟 Linux 行为。
- `proc_mounts()` 动态输出已挂载路径，`/proc/mounts` 和 `/proc/[pid]/mounts` 不再固定写死。
- 支持 `/dev/loop*` 作为 loop mount source，`LoopFileBlockDevice` 能把 loop 绑定文件暴露给 ext4 挂载。
- `/dev/vda`、`/dev/vdb`、`/dev/vdc`、`/dev/vdd` 作为最小 block node stub 暴露，满足 mount source 探测。
- `/sys/block/loop0/queue/logical_block_size`、`dma_alignment` 等 sysfs 文件补齐给 mkfs/mount 测试读取。

这部分使 mount、fanotify all-filesystems matrix、loop device、fsconfig/fsmount 等复杂测试有了可运行基础。

## 13. xattr 与 ext4/lwext4 支持

moss 新增完整 xattr syscall 层：

- 支持 `setxattr/getxattr/listxattr/removexattr`。
- 支持 `lsetxattr/lgetxattr/llistxattr/lremovexattr`，即不跟随 symlink 的路径版本。
- 支持 `fsetxattr/fgetxattr/flistxattr/fremovexattr`。
- 校验 name 长度、value 大小、`XATTR_CREATE`、`XATTR_REPLACE`、`ENODATA`、`ERANGE/E2BIG` 等错误路径。
- 对 immutable/append-only inode flag 做写入限制。
- MemoryFS 增加 xattr map，支持临时文件系统上的 xattr。
- `third_party/lwext4_rust/src/inode/xattr.rs` 和相关 inode/file/dir/attr 封装为 ext4 xattr 提供底层能力。

这部分贡献覆盖 LTP xattr/fxattr/lxattr 测试，也为 fanotify FID 和权限类测试提供更真实的 inode metadata 基础。

## 14. inotify 与 fanotify

文件事件通知是 moss 最突出的功能增量之一。

### inotify

- 新增 `inotify_init1`、`inotify_add_watch`、`inotify_rm_watch`。
- 支持 `IN_CLOEXEC`、`IN_NONBLOCK`、`IN_ONLYDIR`、`IN_DONT_FOLLOW`、`IN_EXCL_UNLINK`、`IN_MASK_ADD`、`IN_MASK_CREATE`、`IN_ONESHOT`。
- 支持事件队列、去重、overflow、watch descriptor、oneshot 自动移除。
- 文件读写、truncate、chmod、utime、create/delete/rename/unmount 等路径接入通知。
- `/proc/sys/fs/inotify/max_queued_events` 可读写，`max_user_instances`、`max_user_watches` 可探测。

### fanotify

- 新增 `fanotify_init`、`fanotify_mark`，实现超过 2000 行的 fanotify 子系统。
- 支持 access、modify、attrib、close、open、open_exec、create、delete、move、rename、delete_self、move_self 等事件。
- 支持 permission event：open/access/open_exec 权限检查和响应。
- 支持 `FAN_REPORT_FID`、`FAN_REPORT_DIR_FID`、`FAN_REPORT_NAME`、`FAN_REPORT_TARGET_FID`、`FAN_REPORT_PIDFD`、`FAN_REPORT_TID`。
- 支持 mount/filesystem mark、ignore mask、survive modify、flush、FID/name 信息结构、PIDFD 信息、事件队列 overflow。
- open/read/write/close/rename/unlink/mount/drop_caches 等路径都接入 fanotify。
- `/proc/sys/fs/fanotify/max_queued_events`、`max_user_groups`、`max_user_marks` 暴露系统限制。
- 针对 fanotify01 到 fanotify21 的多个边界场景修复：FID 模式、rename、ignore、bind mount、symlink follow、unprivileged listener、TID/PIDFD 上报、独立 event fd offset 等。

这部分使 moss 具备了基线 StarryOS 没有的文件事件观察与权限拦截能力，是文件系统兼容性的核心增量。

## 15. 文件 IO、splice、sendfile、AIO

moss 补齐了大量 IO syscall 行为：

- `read/readv/write/writev` 接入 fanotify/inotify access/modify 通知。
- `pread64/pwrite64` 增加负 offset 校验、pipe 错误转换、append 文件写入语义。
- `preadv/pwritev/preadv2/pwritev2` 支持 `offset == -1` 使用当前文件偏移，非零 flags 返回 `EOPNOTSUPP`。
- `sendfile` 修正 offset 上限、读写权限校验和 partial transfer。
- `copy_file_range` 检查读写权限和跨设备 `EXDEV`。
- `splice` 重构为 file/pipe/socket endpoint 模型，要求至少一端 pipe，检查 append、path fd、offset、socket 错误转换，修复超时问题。
- 新增 `sync_file_range`，对 pipe 返回 `ESPIPE`，普通文件执行 sync。
- 新增 `readahead`，对 pidfd、pipe、O_PATH、write-only、非 regular file 做 Linux 风格返回。
- 新增 `fallocate` 基本行为。
- 新增 Linux AIO：`io_setup`、`io_destroy`、`io_submit`、`io_getevents`、`io_cancel`，支持 pread/pwrite iocb 和 event 队列。

这部分主要服务 LTP 的 sendfile、splice、readv/writev/pwritev2、aio、sync_file_range、readahead 等用例。

## 16. `/proc`、`/sys`、`/dev` 伪文件系统

moss 大量扩展伪文件系统，解决用户程序“探测 Linux 环境”失败的问题：

- `/proc/[pid]/status` 增加 `VmLck`。
- `/proc/[pid]/maps` 从固定 vdso 文本改为根据当前地址空间输出真实 VMA。
- 新增 `/proc/[pid]/pagemap`，返回 present bit 的最小兼容内容。
- 新增 `/proc/[pid]/fdinfo`，输出 fd 的 pos/flags/mnt_id 或具体 fdinfo。
- 新增 `/proc/[pid]/setgroups`、`uid_map`、`gid_map`、`timens_offsets`。
- `/proc/mounts`、`/proc/[pid]/mounts` 使用动态 mount 列表。
- 新增 `/proc/cpuinfo`、`/proc/sched_debug`、`/proc/key-users`。
- `/proc/sys/kernel/pid_max` 可写，新增 `tainted`、`core_pattern`。
- `/proc/sys/kernel/random/entropy_avail`、`/dev/random` 的 `RNDGETENTCNT` ioctl 支持熵探测。
- `/proc/sys/kernel/keys/*` 支持 key quota 相关探测。
- `/proc/sys/fs/pipe-max-size`、`aio-max-nr`、`aio-nr`、`protected_hardlinks`、`protected_symlinks`、`fanotify/*`、`inotify/*` 补齐。
- `/proc/sys/vm/vfs_cache_pressure`、`drop_caches` 可读写，其中写 `drop_caches` 会触发 fanotify mark evict。
- `/proc/sys/user/max_user_namespaces` 补齐 user namespace 探测。
- `/dev/tty0`、loop device、block disk stub、random ioctl 等设备节点补齐。
- `/dev/log` 因 NET 默认关闭改为明确 disabled warning，避免依赖网络 Unix socket 服务。

这些伪文件是大量 glibc、busybox、LTP harness 和 mkfs/mount 工具正常运行的前置条件。

## 17. 网络与 BPF 兼容

moss 默认关闭 NET 以降低评测不确定性，但仍对网络路径做了兼容增强：

- `kernel/src/syscall/net/mod.rs` 在 `net` feature 关闭时提供完整 syscall stub，统一返回 `ENOSYS`，保证编译期接口完整。
- 开启 `net` feature 时，socket 支持 IPv6 domain 作为 IPv4 后端兼容，接受 SCTP/UDPLITE protocol 参数。
- 支持 Unix `SOCK_SEQPACKET`。
- `bind` 增加低端口非 root `EACCES` 校验。
- `setsockopt` 支持 `SO_ATTACH_BPF`、`MCAST_JOIN_GROUP`、`MCAST_LEAVE_GROUP`。
- 新增最小 BPF 子系统：`bpf(BPF_MAP_CREATE)`、`BPF_MAP_LOOKUP_ELEM`、`BPF_MAP_UPDATE_ELEM`、`BPF_PROG_LOAD`。
- BPF map 支持 hash、array、ringbuf 类型的最小建模；BPF program 可被 `SO_ATTACH_BPF` 执行，按 LTP 预期写 array map 或返回 verifier 错误。
- datagram Unix socket 改为有界队列，修复 sendfile/socket 相关超时。

这部分不是完整 Linux 网络/BPF，但足以让一批网络、bind、bpf、socket option 测试进入正确错误路径或最小成功路径。

## 18. 第三方库、平台与架构适配

moss 把许多外部依赖纳入仓库并做本地 patch：

- `vendor/axfs` 提供 ext4/fat/highlevel file/fs 能力，并修复 close 时全盘 flush 导致 LA 超时的问题。
- `vendor/axnet` 提供 socket、Unix socket、TCP/UDP/vsock、option、transport 等实现。
- `vendor/axcpu` 增加多架构用户态 trap/context/user_copy，包含 LoongArch64 unaligned 处理。
- `vendor/axhal`、`vendor/axtask`、`vendor/axplat-*` 等固定平台、调度、HAL 行为。
- `third_party/lwext4_rust` 新增 Rust binding 层，覆盖 blockdev、fs、inode、attr、dir、file、xattr。
- `third_party/syscalls` 本地化 syscall number 表，减少 crates.io 依赖。
- `Cargo.lock`、`.cargo/config.toml`、`cargo-config.toml` 保证依赖解析稳定。

这部分贡献看起来像“把依赖放进仓库”，但对比赛评测很关键：它消除了网络依赖、版本漂移和 LA/RV 平台差异造成的构建失败。

## 19. 从入口到结束的完整运行顺序

按一次典型 `run_all.sh` 或评测运行来看，moss 的贡献可以串成如下流程：

1. `make all` 使用固定 nightly、离线 vendor、本地 axconfig/cargo-binutils 构建 RV 和 LA 内核。
2. 生成 `kernel-rv`、`kernel-la`，并准备/复制 `sdcard-rv.img`、`sdcard-la.img`。
3. `scripts/inject_libgcc.sh` 向镜像注入运行库，解决 glibc/musl 动态程序缺库问题。
4. QEMU 启动目标架构内核。
5. 内核进入 `starry_kernel::entry::init`，挂载 `/proc`、`/sys`、`/dev`、`/tmp` 等伪文件系统。
6. 内核解析 init 命令，必要时在 musl/glibc/bin busybox 间 fallback。
7. ELF loader 解析 busybox 和动态链接器，必要时 fallback 到 `/glibc/...` 或 `/musl/...`。
8. 创建 init 进程、线程、地址空间、stdio、tty，进入用户态。
9. busybox shell 执行内嵌 `src/init.sh`。
10. init 脚本补齐 `/bin`、`/lib`、`/etc`、`/proc/sys` 探测需要的用户态环境。
11. init 根据开关选择 musl/glibc 下的测试组，当前主要运行 LTP。
12. 每个 LTP case 调用真实 syscall，进入 moss 的 syscall 分发层。
13. syscall 层按领域进入 task/mm/fs/net/time/sync/io_mpx 等模块。
14. 文件读写、路径操作、mount、rename、close 等动作同时触发 inotify/fanotify。
15. poll/epoll/timerfd/itimer/POSIX timer 通过 `axpoll`、alarm task 和 timer worker 唤醒等待线程。
16. fork/exec/wait/ptrace/signal 在进程表、task ext、ProcessData 和 signal manager 间协作。
17. 测例读取 `/proc`、`/sys`、`/dev` 时，由 moss 的伪文件系统返回 Linux 兼容探测数据。
18. init 脚本为缺少 Summary 的 LTP 输出补齐统计，并打印评测需要的 group/case 标记。
19. 所有测试结束后 init `exit 0`。
20. 内核 join init task，卸载所有文件系统，flush rootfs。
21. 内核打印完成信息并调用 `system_off()`，QEMU 退出，脚本收集 `output_rv.txt`、`output_la.txt`。

## 20. 贡献性质总结

moss 的增量贡献可以归纳为四类：

- **Linux 兼容性补齐**：大量 syscall、权限、文件系统、内存、时间、信号、等待、事件通知行为从简单实现升级为 LTP 可验证语义。
- **评测可运行性增强**：init、运行库、busybox、glibc/musl、LTP case 选择、Summary 修复、RV/LA 双架构脚本都服务于稳定计分。
- **工程可复现性增强**：vendored 依赖、固定工具链、离线 cargo、本地工具构建、镜像准备和 wrapper ELF 输出让项目能在隔离环境中构建运行。
- **平台适配与性能修复**：LoongArch64 用户态异常、COW fork 优化、page cache 清零、close flush 优化、timer/epoll 超时修复，解决了实际运行中的卡死、panic 和超时问题。

需要注意的是，部分实现是“面向比赛/LTP 的最小兼容实现”，例如 BPF verifier、xfs/btrfs/ntfs 的 compat memory mount、部分 keyctl/acct 行为，并不等价于完整 Linux 内核实现。但从 base OS 到 moss 的增量看，它已经把 StarryOS 从基础内核运行框架，扩展成了一个可自动评测、可运行大量 Linux 用户程序和 LTP syscall 用例的系统。
