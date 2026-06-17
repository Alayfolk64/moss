# LTP skip recommendations

数据来源: `output_rv.txt`

当前结论:

- musl 已跑完: `passed = 11614`
- 新约束: musl 侧达到 `5500` 即可
- musl 当前可牺牲 passed 预算: `11614 - 5500 = 6114`
- glibc 当前卡在: `execve05`
- glibc 已完成部分: `passed = 1289`
- 已完成 Summary 的 passed 合计: `12903`
- 当前重点不是保满分，而是保证测试能稳定结束，并给写文档留时间。

一个极限参考: 当前日志中 musl 耗时 `>=5s` 的 case 共 72 个，总耗时约 `752s`，总 passed 约 `6056`。如果全部跳过，musl 仍约 `5558` 分，刚好高于 `5500`，但只有约 `58` 分缓冲。这个方案很激进，适合最后时间极紧时使用。

## 目标方案: fanotify 留约一半，其它凑 5500

fanotify 总分约 `4079`。推荐保留:

```sh
fanotify16 fanotify16   # 1386 passed, ~8s
fanotify10 fanotify10   # 1047 passed, ~6s
```

这两个合计 `2433` 分，约等于 fanotify 总分的 `60%`。比严格卡在一半略高，但只需要约 `14s`，性价比最好。

如果一定要更接近一半，可以保留 `fanotify16 + fanotify14 + fanotify09 + fanotify03 + fanotify20 + fanotify21 + fanotify19`，合计约 `2048` 分，但用例更多、耗时约 `27s`。

推荐按第一种保留 fanotify 后，其它非 fanotify 保留下面这组短平快用例。它们合计约 `3223` 分、约 `78s`，加上 fanotify 的 `2433` 分，总计约 `5656`，比 `5500` 多 `156` 分缓冲。

```sh
splice07 splice07
prot_hsymlinks prot_hsymlinks
epoll_ctl03 epoll_ctl03
access01 access01
rt_sigaction02 rt_sigaction02
rt_sigaction01 rt_sigaction01
rt_sigaction03 rt_sigaction03
waitpid01 waitpid01
getpid01 getpid01
mount07 mount07
pipe11 pipe11
ptrace06 ptrace06
ptrace05 ptrace05
timer_settime02 timer_settime02
splice08 splice08
sysconf01 sysconf01
chmod01 chmod01
confstr01 confstr01
posix_fadvise03 posix_fadvise03
posix_fadvise03_64 posix_fadvise03_64
timer_settime01 timer_settime01
signal03 signal03
signal05 signal05
getitimer01 getitimer01
open11 open11
signal04 signal04
fsconfig02 fsconfig02
kill11 kill11
linkat01 linkat01
setregid03 setregid03
madvise01 madvise01
mlock03 mlock03
readahead01 readahead01
llseek03 llseek03
copy_file_range02 copy_file_range02
pathconf01 pathconf01
bind04 bind04
clock_gettime01 clock_gettime01
getrlimit01 getrlimit01
select03 select03
access02 access02
bind05 bind05
link04 link04
memfd_create02 memfd_create02
mmap04 mmap04
setreuid03 setreuid03
setreuid05 setreuid05
fgetxattr02 fgetxattr02
stat01 stat01
access04 access04
clone302 clone302
fcntl15 fcntl15
membarrier01 membarrier01
setregid02 setregid02
stat01_64 stat01_64
fcntl15_64 fcntl15_64
mount02 mount02
timerfd01 timerfd01
mount05 mount05
mkdir03 mkdir03
futex_wake02 futex_wake02
```

执行方式建议反过来做: 把 `ltp_testcases.sh` 里不在上述 fanotify 保留项和非 fanotify 保留项里的 LTP case 先注释掉。这样 musl 仍能约 `5656` 分，同时大幅减少长测例和卡死风险。

## P0: 建议立即注释

这些用例要么已经造成 glibc 卡死，要么耗时很长但得分很低。先注释这一组，收益最大，风险最低。

| case | 当前证据 | musl passed 损失 | 建议原因 |
| --- | --- | ---: | --- |
| `execve05` | glibc 当前卡死在这里，没有 Summary | 160 | 必须先跳过。虽然 musl 得分高，但它会阻塞 glibc 后续所有分数。 |
| `inotify09` | musl 约 151s | 1 | 极低分、高耗时，最不划算。 |
| `mmap11` | musl 约 39s，命令为 `mmap11 -i 30000` | 1 | 循环压力用例，耗时高、得分低。 |
| `gettimeofday02` | musl 约 11s | 1 | 低收益等待类用例。 |
| `kill02` | musl 约 12s | 2 | 低收益，容易拖慢整体。 |
| `epoll_wait02` | musl 约 20s，glibc 约 21s | 7 | 等待类用例，两侧都慢。 |
| `getrusage03` | musl 约 20s，且 `failed = 3` | 6 | 慢且失败，不值得维护。 |
| `futex_wait05` | musl 约 21s | 7 | futex 等待类，时间不可控风险较高。 |
| `copy_file_range01` | musl 约 10s，且有 skipped | 8 | 分数不高，文件系统语义容易继续牵扯。 |

P0 估计只减少 musl 约 `193` 个 passed。musl 侧仍约 `11421`，即使 glibc 不计分也还高于 `11000`。

建议对应删除或注释这些行:

```sh
execve05 execve05 -i 5 -n 32
inotify09 inotify09
mmap11 mmap11 -i 30000
gettimeofday02 gettimeofday02
kill02 kill02
epoll_wait02 epoll_wait02
getrusage03 getrusage03
futex_wait05 futex_wait05
copy_file_range01 copy_file_range01
```

## P1: 如果还觉得慢，再注释

这些用例耗时明显，但有一部分得分。现在 musl 只要求 `5500`，所以这组可以比较放心地跳过。

| case | 当前证据 | musl passed 损失 | 备注 |
| --- | --- | ---: | --- |
| `epoll01` | musl 约 29s 且 `failed = 1`，glibc 约 49s | 32 | 慢，glibc 更慢。 |
| `fanotify13` | musl 约 27s，`skipped = 216` | 135 | 有分但 skip 很多，时间也长。 |
| `mount03` | musl 约 26s | 114 | 文件系统挂载类，容易牵扯较多语义。 |
| `mkdir09` | musl 约 16s | 54 | 得分中等但耗时偏高。 |
| `fcntl36` | musl 约 8s | 7 | 低分中耗时。 |
| `fcntl36_64` | musl 约 8s | 7 | 同上。 |
| `fcntl14` | musl 约 7s | 96 | 分数较高，建议只在非常赶时间时跳。 |
| `fcntl14_64` | musl 约 8s | 96 | 分数较高，建议只在非常赶时间时跳。 |
| `chdir01` | musl 约 6s，glibc 约 7s | 144 | 分数高，优先级低于上面低分慢用例。 |
| `close_range01` | musl 约 6s，glibc 约 6s | 36 | 可选。 |

## P2: 主要为减少失败噪声

这些不一定慢，但已经有 TFAIL/TBROK/TWARN 或 0 pass。要写文档和保稳定时，可以一起清掉。

```sh
clone04 clone04
close_range02 close_range02
fcntl35 fcntl35
fcntl35_64 fcntl35_64
fanotify17 fanotify17
fanotify23 fanotify23
nftw01 nftw01
nftw6401 nftw6401
symlink03 symlink03
fallocate01 fallocate01
fallocate03 fallocate03
fallocate04 fallocate04
memfd_create01 memfd_create01
```

## P3: 极限省时方案

如果目标变成“能出分就行，优先别卡死”，可以直接注释所有 musl 侧耗时 `>=5s` 的 case。按当前日志估算:

- 节省时间: 约 `752s`
- musl passed 损失: 约 `6056`
- musl 剩余: 约 `5558`
- 风险: 只比 `5500` 多约 `58` 分，后续如果又删了别的有分 case，可能跌破线。

极限方案候选如下:

```sh
inotify09 inotify09
mmap11 mmap11 -i 30000
epoll01 epoll-ltp
fanotify13 fanotify13
mount03 mount03
futex_wait05 futex_wait05
epoll_wait02 epoll_wait02
getrusage03 getrusage03
mkdir09 mkdir09
execve05 execve05 -i 5 -n 32
kill02 kill02
gettimeofday02 gettimeofday02
copy_file_range01 copy_file_range01
inotify11 inotify11
times03 times03
fcntl14_64 fcntl14_64
fcntl36 fcntl36
fcntl36_64 fcntl36_64
fanotify16 fanotify16
fcntl14 fcntl14
fsmount02 fsmount02
fanotify14 fanotify14
rename14 rename14
statfs01 statfs01
futex_cmp_requeue01 futex_cmp_requeue01
chdir01 chdir01
close_range01 close_range01
fsetxattr01 fsetxattr01
fgetxattr01 fgetxattr01
fremovexattr02 fremovexattr02
fsconfig01 fsconfig01
fsconfig03 fsconfig03
fsopen01 fsopen01
fstatfs01_64 fstatfs01_64
fsync01 fsync01
getxattr02 getxattr02
fanotify01 fanotify01
fanotify10 fanotify10
fanotify15 fanotify15
lremovexattr01 lremovexattr01
move_mount01 move_mount01
open_tree02 open_tree02
pwritev03_64 pwritev03_64
rename01 rename01
rename03 rename03
rename04 rename04
rename05 rename05
rename06 rename06
rename08 rename08
rename10 rename10
setxattr01 setxattr01
statfs01_64 statfs01_64
statvfs01 statvfs01
utime02 utime02
statx08 statx08
creat09 creat09
fremovexattr01 fremovexattr01
fsmount01 fsmount01
fstatfs01 fstatfs01
getxattr03 getxattr03
mount01 mount01
mount06 mount06
move_mount02 move_mount02
open_tree01 open_tree01
pwritev03 pwritev03
rename13 rename13
utime01 utime01
utime03 utime03
utime04 utime04
utime05 utime05
statx04 statx04
statx12 statx12
```

更稳一点的激进方案: 注释 P0 + P1 + P2，再从 P3 里优先删 `passed <= 32` 的慢 case。这样能省掉大部分卡顿风险，同时 musl 分数仍有很大余量。

## 已经删除的 skipped-only 用例

这些用例在当前日志里只贡献 skipped，不贡献 passed，已经适合从 `ltp_testcases.sh` 里移除:

```sh
bind06
execveat03
fanotify22
fcntl38
fcntl38_64
fcntl39
fcntl39_64
futex_waitv02
futex_wake04
ioctl09
memfd_create03
memfd_create04
open14
openat03
openat04
openat201
openat202
openat203
perf_event_open01
perf_event_open02
readahead02
statx05
statx09
userfaultfd01
vhangup01
vhangup02
writev03
```

## 建议执行顺序

1. 先注释 P0，尤其是 `execve05`。
2. 由于 musl 只要求 `5500`，可以直接继续注释 P1。
3. 如果只是想减少失败噪声，再处理 P2。
4. 如果时间压到极限，再采用 P3；但 P3 全部跳过的分数缓冲很小。

保守策略: 只注释 P0。  
推荐策略: 注释 P0 + P1 + P2。  
极限策略: 注释 P3 全部，但需要注意 musl 只剩约 `5558`。
