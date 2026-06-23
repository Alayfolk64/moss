# Format: <case> <command> [args...]
# Active/commented state comes from init.sh; summary-only additions are commented.
# Previously commented TODO syscall cases moved to todo_syscall.sh.

# abort01 abort01
accept01 accept01
# accept02 accept02
accept03 accept03
accept4_01 accept4_01
access01 access01
access02 access02
access03 access03
access04 access04
acct01 acct01
# acct02 acct02
add_key01 add_key01
add_key02 add_key02
# add_key03 add_key03
# add_key04 add_key04
add_key05 add_key05
adjtimex01 adjtimex01
adjtimex02 adjtimex02
adjtimex03 adjtimex03
alarm02 alarm02
alarm03 alarm03
# alarm05 alarm05
# alarm06 alarm06
# alarm07 alarm07
bind01 bind01
# bind02 bind02
bind03 bind03
bind04 bind04
bind05 bind05
bpf_map01 bpf_map01
bpf_prog01 bpf_prog01
bpf_prog02 bpf_prog02
bpf_prog03 bpf_prog03
bpf_prog04 bpf_prog04
brk01 brk01
brk02 brk02
capget01 capget01
capget02 capget02
capset01 capset01
capset02 capset02
# capset03 capset03
# capset04 capset04
# chdir01 chdir01
chdir04 chdir04
chmod01 chmod01
# chmod03 chmod03
# chmod06 chmod06
# chown01 chown01
# chown02 chown02
# chown03 chown03
# chown04 chown04
# chown05 chown05
# chroot02 chroot02
# chroot03 chroot03
# clock_adjtime01 clock_adjtime01
# clock_adjtime02 clock_adjtime02
# clock_getres01 clock_getres01
# clock_nanosleep01 clock_nanosleep01
# clock_nanosleep04 clock_nanosleep04
clock_gettime01 clock_gettime01
clock_gettime02 clock_gettime02
clone01 clone01
# clone02 clone02
clone03 clone03
# clone04 clone04
clone06 clone06
clone07 clone07
clone08 clone08
clone301 clone301
clone302 clone302
close01 close01
# close02 close02
# close_range01 close_range01
close_range02 close_range02
confstr01 confstr01
# connect01 connect01
creat01 creat01
creat03 creat03
# creat05 creat05
creat06 creat06
creat09 creat09
dup01 dup01
dup02 dup02
dup03 dup03
dup04 dup04
# dup06 dup06
dup07 dup07
dup201 dup201
dup202 dup202
dup203 dup203
dup204 dup204
dup205 dup205
# dup206 dup206
dup207 dup207
dup3_01 dup3_01
dup3_02 dup3_02
epoll_create01 epoll_create01
epoll_create02 epoll_create02
epoll_create1_01 epoll_create1_01
epoll_create1_02 epoll_create1_02
# epoll01 epoll-ltp
epoll_ctl01 epoll_ctl01
epoll_ctl02 epoll_ctl02
epoll_ctl03 epoll_ctl03
epoll_ctl04 epoll_ctl04
epoll_ctl05 epoll_ctl05
epoll_wait01 epoll_wait01
# epoll_wait02 epoll_wait02
epoll_wait03 epoll_wait03
# epoll_wait04 epoll_wait04
epoll_wait05 epoll_wait05
epoll_wait06 epoll_wait06
epoll_wait07 epoll_wait07
epoll_pwait01 epoll_pwait01
epoll_pwait02 epoll_pwait02
epoll_pwait04 epoll_pwait04
epoll_pwait05 epoll_pwait05
eventfd01 eventfd01
eventfd02 eventfd02
eventfd03 eventfd03
eventfd04 eventfd04
eventfd05 eventfd05
eventfd2_01 eventfd2_01
eventfd2_02 eventfd2_02
eventfd2_03 eventfd2_03
execl01 execl01
execle01 execle01
execlp01 execlp01
execv01 execv01
execve01 execve01
# execve02 execve02
execve03 execve03
execve04 execve04
# execve05 execve05 -i 5 -n 32
execve06 execve06
execvp01 execvp01
execveat01 execveat01
execveat02 execveat02
# exit01 exit01
exit02 exit02
exit_group01 exit_group01
faccessat01 faccessat01
faccessat02 faccessat02
faccessat201 faccessat201
faccessat202 faccessat202
# fallocate01 fallocate01
fallocate03 fallocate03
# fallocate04 fallocate04
fsetxattr01 fsetxattr01
# fsetxattr02 fsetxattr02
posix_fadvise01                      posix_fadvise01
posix_fadvise01_64                posix_fadvise01_64
posix_fadvise03                      posix_fadvise03
posix_fadvise03_64                posix_fadvise03_64
fchdir01 fchdir01
fchdir02 fchdir02
fchmod01 fchmod01
# fchmod03 fchmod03
fchmod04 fchmod04
fchmodat01 fchmodat01
fchmodat02 fchmodat02
fchown01 fchown01
fchown02 fchown02
# fchown03 fchown03
fchown04 fchown04
fchown05 fchown05
# fchownat01 fchownat01
# fchownat02 fchownat02
fcntl02 fcntl02
fcntl02_64 fcntl02_64
fcntl03 fcntl03
fcntl03_64 fcntl03_64
fcntl04 fcntl04
fcntl04_64 fcntl04_64
fcntl05 fcntl05
fcntl05_64 fcntl05_64
fcntl08 fcntl08
fcntl08_64 fcntl08_64
# fcntl09 fcntl09
# fcntl09_64 fcntl09_64
# fcntl10 fcntl10
# fcntl10_64 fcntl10_64
# fcntl11 fcntl11
# fcntl11_64 fcntl11_64
fcntl12 fcntl12
fcntl12_64 fcntl12_64
fcntl13 fcntl13
fcntl13_64 fcntl13_64
# fcntl14 fcntl14
# fcntl14_64 fcntl14_64
fcntl15 fcntl15
fcntl15_64 fcntl15_64
# fcntl16 fcntl16
# fcntl16_64 fcntl16_64
# fcntl17 fcntl17
# fcntl17_64 fcntl17_64
# fcntl18 fcntl18
# fcntl18_64 fcntl18_64
# fcntl19 fcntl19
# fcntl19_64 fcntl19_64
# fcntl20 fcntl20
# fcntl20_64 fcntl20_64
# fcntl21 fcntl21
# fcntl21_64 fcntl21_64
# fcntl22 fcntl22
# fcntl22_64 fcntl22_64
# fcntl23 fcntl23
# fcntl23_64 fcntl23_64
fcntl27 fcntl27
fcntl27_64 fcntl27_64
fcntl29 fcntl29
fcntl29_64 fcntl29_64
fcntl30 fcntl30
fcntl30_64 fcntl30_64
# fcntl31 fcntl31
# fcntl31_64 fcntl31_64
fcntl34 fcntl34
fcntl34_64 fcntl34_64
fcntl35 fcntl35
fcntl35_64 fcntl35_64
# fcntl36 fcntl36
# fcntl36_64 fcntl36_64
fcntl37 fcntl37
fcntl37_64 fcntl37_64
# fdatasync01 fdatasync01
# fdatasync02 fdatasync02
fgetxattr01 fgetxattr01
fgetxattr02 fgetxattr02
fgetxattr03 fgetxattr03
flistxattr01 flistxattr01
flistxattr02 flistxattr02
flistxattr03 flistxattr03
flock01 flock01
flock03 flock03
flock04 flock04
flock06 flock06
fork01 fork01
fork03 fork03
fork04 fork04
# fork06 fork_procs -n 1000
# fork07 fork07
fork08 fork08
# fork09 fork09
fork10 fork10
fork11 fork_procs -n 100
fpathconf01 fpathconf01
# fremovexattr01 fremovexattr01
fremovexattr02 fremovexattr02
# fsconfig01 fsconfig01
fsconfig02 fsconfig02
# fsconfig03 fsconfig03
fsmount01 fsmount01
fsmount02 fsmount02
# fsopen01 fsopen01
# fsopen02 fsopen02
fstat02 fstat02
fstat02_64 fstat02_64
fstat03 fstat03
fstat03_64 fstat03_64
# fstatat01 fstatat01
# fstatfs01 fstatfs01
# fstatfs01_64 fstatfs01_64
fstatfs02 fstatfs02
fstatfs02_64 fstatfs02_64
fsync01 fsync01
ftruncate01 ftruncate01
ftruncate01_64 ftruncate01_64
getcwd01 getcwd01
getcwd02 getcwd02
getcwd03 getcwd03
getdents01 getdents01
getdents02 getdents02
getdomainname01 getdomainname01
getegid01 getegid01
getegid01_16 getegid01_16
getegid02 getegid02
getegid02_16 getegid02_16
geteuid01 geteuid01
geteuid02 geteuid02
getgid01 getgid01
getgid03 getgid03
# getgroups01 getgroups01
# getgroups03 getgroups03
# gethostbyname_r01 gethostbyname_r01
gethostname01 gethostname01
gethostname02 gethostname02
getitimer01 getitimer01
# getitimer02 getitimer02
# getpagesize01 getpagesize01
# getpeername01 getpeername01
# getpgid01 getpgid01
# getpgid02 getpgid02
# getpgrp01 getpgrp01
getpid01 getpid01
getpid02 getpid02
getppid01 getppid01
getppid02 getppid02
getpriority01 getpriority01
getpriority02 getpriority02
getrandom01 getrandom01
getrandom02 getrandom02
getrandom03 getrandom03
getrandom04 getrandom04
getrandom05 getrandom05
# getresgid01 getresgid01
# getresgid02 getresgid02
# getresgid03 getresgid03
# getresuid01 getresuid01
# getresuid02 getresuid02
# getresuid03 getresuid03
getrlimit01 getrlimit01
getrlimit02 getrlimit02
# get_robust_list01 get_robust_list01
getrusage01 getrusage01
getrusage02 getrusage02
# getrusage03 getrusage03
getsid01 getsid01
getsid02 getsid02
getsockname01 getsockname01
getsockopt01 getsockopt01
getsockopt02 getsockopt02
gettid01 gettid01
gettid02 gettid02
gettimeofday01 gettimeofday01
# gettimeofday02 gettimeofday02
getuid01 getuid01
getuid03 getuid03
getxattr01 getxattr01
getxattr02 getxattr02
# getxattr03 getxattr03
# getxattr04 getxattr04
ioctl02      test_ioctl
# ioctl04      ioctl04
# ioctl05      ioctl05
# ioctl06      ioctl06
ioctl07      ioctl07
# ioctl08      ioctl08
# ioctl_loop01 ioctl_loop01
# ioctl_loop02 ioctl_loop02
# ioctl_loop03 ioctl_loop03
# ioctl_loop04 ioctl_loop04
# ioctl_loop06 ioctl_loop06
# ioctl_loop07 ioctl_loop07
ioctl_ns01 ioctl_ns01
ioctl_ns02 ioctl_ns02
ioctl_ns03 ioctl_ns03
ioctl_ns04 ioctl_ns04
ioctl_ns05 ioctl_ns05
ioctl_ns06 ioctl_ns06
ioctl_ns07 ioctl_ns07
inotify_init1_01 inotify_init1_01
inotify_init1_02 inotify_init1_02
inotify01 inotify01
inotify02 inotify02
# inotify03 inotify03
inotify04 inotify04
# inotify05 inotify05
# inotify06 inotify06
# inotify09 inotify09
inotify10 inotify10
# inotify11 inotify11
inotify12 inotify12
# fanotify01 fanotify01
# fanotify02 fanotify02
fanotify03 fanotify03
fanotify04 fanotify04
# fanotify05 fanotify05
# fanotify06 fanotify06
fanotify07 fanotify07
fanotify08 fanotify08
fanotify09 fanotify09
# fanotify10 fanotify10
# fanotify11 fanotify11
fanotify12 fanotify12
# fanotify13 fanotify13
fanotify14 fanotify14
fanotify15 fanotify15
# fanotify16 fanotify16
# fanotify17 fanotify17
# fanotify18 fanotify18
fanotify19 fanotify19
fanotify20 fanotify20
fanotify21 fanotify21
# fanotify23 fanotify23
ioprio_get01 ioprio_get01
ioprio_set01 ioprio_set01
ioprio_set02 ioprio_set02
ioprio_set03 ioprio_set03
io_cancel01 io_cancel01
io_destroy02 io_destroy02
io_getevents01 io_getevents01
io_setup02 io_setup02
io_submit02 io_submit02
io_submit03 io_submit03
# kill02 kill02
kill03 kill03
# kill05 kill05
kill06 kill06
# kill07 kill07
# kill08 kill08
# kill09 kill09
# kill10 kill10
kill11 kill11
# kill12 kill12
# lchown01 lchown01
# lchown02  lchown02
# lchown03  lchown03
lgetxattr01 lgetxattr01
lgetxattr02 lgetxattr02
# link01 symlink01 -T link01
link02 link02
link04 link04
# link05 link05
link08 link08
# linkat01 linkat01
# linkat02 linkat02
# listen01 listen01
listxattr01 listxattr01
listxattr02 listxattr02
listxattr03 listxattr03
llistxattr01 llistxattr01
llistxattr02 llistxattr02
llistxattr03 llistxattr03
llseek01 llseek01
llseek02 llseek02
llseek03 llseek03
# lremovexattr01 lremovexattr01
lseek01 lseek01
lseek07 lseek07
lstat01 lstat01
lstat01_64 lstat01_64
lstat02 lstat02
lstat02_64 lstat02_64
memset01 memset01
memcmp01 memcmp01
memcpy01 memcpy01
# mlockall01 mlockall01
# mlockall02 mlockall02
mkdir02 mkdir02
mkdir03 mkdir03
mkdir04 mkdir04
mkdir05 mkdir05
# mkdir09 mkdir09
# mkdirat01 mkdirat01
mkdirat02 mkdirat02
mknod01 mknod01
mknod02 mknod02
# mknod03 mknod03
# mknod04 mknod04
# mknod05 mknod05
# mknod06 mknod06
# mknod07 mknod07
# mknod08 mknod08
mknod09 mknod09
# mknodat01 mknodat01
# mknodat02 mknodat02
mlock01 mlock01
mlock03 mlock03 -i 20
mlock04 mlock04
mlock202 mlock202
# mmap01 mmap01
mmap02 mmap02
# mmap03 mmap03
mmap04 mmap04
mmap05 mmap05
mmap06 mmap06
mmap08 mmap08
mmap09 mmap09
# mmap12 mmap12
mmap13 mmap13
# mmap14 mmap14
# mmap11 mmap11 -i 30000
mmap15 mmap15
mmap17 mmap17
mmap18 mmap18
mmap19 mmap19
mmap20 mmap20
# mount01 mount01
# mount02 mount02
# mount03 mount03
# mount04 mount04
# mount05 mount05
# mount06 mount06
# mount07 mount07
# move_mount01 move_mount01
move_mount02 move_mount02
# mremap02 mremap02
# mremap05 mremap05
msgctl03 msgctl03
msgctl12 msgctl12
# msync01 msync01
# msync02 msync02
munlock01 munlock01
# munmap01 munmap01
# munmap02 munmap02
# munmap03 munmap03
nanosleep02 nanosleep02
nanosleep04 nanosleep04
# nftw01 nftw01
# nftw6401 nftw6401
open01 open01
# open01A symlink01 -T open01
open02 open02
open03 open03
# open04 open04
open06 open06
open07 open07
open08 open08
open09 open09
open10 open10
open11 open11
# open12 open12
# open13 open13
openat01 openat01
# openat02 openat02
open_by_handle_at01 open_by_handle_at01
open_by_handle_at02 open_by_handle_at02
# open_tree01 open_tree01
# open_tree02 open_tree02
madvise01 madvise01
madvise05 madvise05
madvise10 madvise10
# newuname01 newuname01
pathconf01 pathconf01
pathconf02 pathconf02
pidfd_open01 pidfd_open01
pidfd_open02 pidfd_open02
pidfd_open03 pidfd_open03
pidfd_open04 pidfd_open04
pipe01 pipe01
# pipe02 pipe02
pipe03 pipe03
# pipe04 pipe04
# pipe05 pipe05
# pipe06 pipe06
# pipe07 pipe07
# pipe08 pipe08
# pipe09 pipe09
# pipe10 pipe10
pipe11 pipe11
pipe12 pipe12
poll01 poll01
prctl02 prctl02
prctl05 prctl05
# pread01 pread01
# pread01_64 pread01_64
pread02 pread02
pread02_64 pread02_64
preadv01 preadv01
preadv01_64 preadv01_64
preadv02 preadv02
preadv02_64 preadv02_64
preadv201 preadv201
preadv201_64 preadv201_64
preadv202 preadv202
preadv202_64 preadv202_64
# prot_hsymlinks prot_hsymlinks
pselect02 pselect02
pselect02_64 pselect02_64
# pselect03 pselect03
# pselect03_64 pselect03_64
ptrace01 ptrace01
# ptrace02 ptrace02
ptrace03 ptrace03
# ptrace05 ptrace05
# ptrace06 ptrace06
# ptrace11 ptrace11
# pwrite01 pwrite01
pwrite02 pwrite02
# pwrite03 pwrite03
# pwrite04 pwrite04
# pwrite01_64 pwrite01_64
pwrite02_64 pwrite02_64
# pwrite03_64 pwrite03_64
# pwrite04_64 pwrite04_64
pwritev01 pwritev01
pwritev01_64 pwritev01_64
pwritev02 pwritev02
pwritev02_64 pwritev02_64
# pwritev03 pwritev03
# pwritev03_64 pwritev03_64
pwritev201 pwritev201
pwritev201_64 pwritev201_64
pwritev202 pwritev202
pwritev202_64 pwritev202_64
# read01 read01
read02 read02
# read04 read04
readahead01 readahead01
# readdir01 readdir01
# readlink01 readlink01
readlink03 readlink03
readlinkat02 readlinkat02
readv01 readv01
readv02 readv02
# recv01 recv01
# recvfrom01 recvfrom01
recvmsg01 recvmsg01
recvmsg02 recvmsg02
# removexattr01 removexattr01
# removexattr02 removexattr02
rename01 rename01
rename03 rename03
# rename04 rename04
# rename05 rename05
# rename06 rename06
# rename08 rename08
# rename10 rename10
# rename11 rename11
# rename13 rename13
# rename14 rename14
# renameat01 renameat01
# renameat201 renameat201
# rmdir01 rmdir01
rmdir02 rmdir02
# rt_sigaction01 rt_sigaction01
# rt_sigaction02 rt_sigaction02
# rt_sigaction03 rt_sigaction03
# rt_sigprocmask01 rt_sigprocmask01
# rt_sigprocmask02 rt_sigprocmask02
sbrk01 sbrk01
# sbrk02 sbrk02
sched_getscheduler01 sched_getscheduler01
# sched_yield01 sched_yield01
sched_setaffinity01 sched_setaffinity01
sched_getaffinity01 sched_getaffinity01
select03 select03
sendfile02 sendfile02
sendfile02_64 sendfile02_64
sendfile03 sendfile03
sendfile03_64 sendfile03_64
sendfile04 sendfile04
sendfile04_64 sendfile04_64
# sendfile05 sendfile05
# sendfile05_64 sendfile05_64
# sendfile06 sendfile06
# sendfile06_64 sendfile06_64
# sendfile07 sendfile07
# sendfile07_64 sendfile07_64
# sendfile08 sendfile08
# sendfile08_64 sendfile08_64
sendmmsg02 sendmmsg02
# sendto01 sendto01
# set_robust_list01 set_robust_list01
# set_tid_address01 set_tid_address01
# setgid01 setgid01
setgid03 setgid03
# setgroups01 setgroups01
setgroups02 setgroups02
# setpgrp01 setpgrp01
setpgrp02 setpgrp02
setregid01 setregid01
setregid02 setregid02
setregid03 setregid03
setregid04 setregid04
# setresgid01 setresgid01
setresgid02 setresgid02
setresgid03 setresgid03
# setresgid04 setresgid04
setresuid01 setresuid01
setresuid02 setresuid02
setresuid03 setresuid03
setresuid04 setresuid04
setresuid05 setresuid05
setreuid01 setreuid01
setreuid02 setreuid02
setreuid03 setreuid03
# setreuid04 setreuid04
setreuid05 setreuid05
setreuid06 setreuid06
setreuid07 setreuid07
setsockopt01 setsockopt01
# setsockopt03 setsockopt03
settimeofday02 settimeofday02
setuid01 setuid01
# setxattr01 setxattr01
setxattr02 setxattr02
setxattr03 setxattr03
shmat03 shmat03
shmctl07 shmctl07
shmctl08 shmctl08
# sigaction01 sigaction01
# sigaction02 sigaction02
# sigaltstack01 sigaltstack01
sigaltstack02 sigaltstack02
# sighold02 sighold02
signal01 signal01
signal02 signal02
signal03 signal03
signal04 signal04
signal05 signal05
# signalfd01 signalfd01
# signalfd4_01 signalfd4_01
# signalfd4_02 signalfd4_02
sigpending02 sigpending02
# sigprocmask01 sigprocmask01
sigwait01 sigwait01
socket01 socket01
socket02 socket02
socketpair01 socketpair01
socketpair02 socketpair02
# sockioctl01 sockioctl01
splice01 splice01
# splice02 splice02
splice03 splice03
# splice04 splice04
splice05 splice05
splice07 splice07
splice08 splice08
# splice09 splice09
stat01 stat01
stat01_64 stat01_64
stat02 stat02
stat02_64 stat02_64
stat03 stat03
stat03_64 stat03_64
# statfs01 statfs01
# statfs01_64 statfs01_64
statfs02 statfs02
statfs02_64 statfs02_64
# statvfs01 statvfs01
statvfs02 statvfs02
# string01 string01
# symlink01 symlink01
# symlink02 symlink02
# symlink03 symlink03
symlink04 symlink04
syscall01 syscall01
# sysconf01 sysconf01
# sysinfo01 sysinfo01
# sysinfo02 sysinfo02
time01 time01
# times01 times01
times03 times03
timerfd01 timerfd01
# timerfd02 timerfd02
# timerfd04 timerfd04
# timerfd_create01 timerfd_create01
# timerfd_gettime01 timerfd_gettime01
# timerfd_settime01 timerfd_settime01
# timer_delete01 timer_delete01
# timer_delete02 timer_delete02
# timer_getoverrun01 timer_getoverrun01
# timer_gettime01 timer_gettime01
timer_settime01 timer_settime01
timer_settime02 timer_settime02
# timer_settime03 timer_settime03
tkill01 tkill01
tkill02 tkill02
truncate02 truncate02
truncate02_64 truncate02_64
truncate03 truncate03
truncate03_64 truncate03_64
# ulimit01 ulimit01
# umask01 umask01
uname01 uname01
# uname02 uname02
uname04 uname04
unlink05 unlink05
unlink07 unlink07
unlink08 unlink08
unlinkat01 unlinkat01
unshare01 unshare01
unshare02 unshare02
# umount01 umount01
# umount02 umount02
# umount2_02 umount2_02
# utime01 utime01
# utime02 utime02
# utime03 utime03
# utime04 utime04
# utime05 utime05
utime06 utime06
utime07 utime07
utimes01 utimes01
utimensat01 utimensat01
# vfork02 vfork02
# wait01 wait01
# wait02 wait02
wait401 wait401
# wait402 wait402
# wait403 wait403
waitpid01 waitpid01
waitpid03 waitpid03
waitpid04 waitpid04
# waitpid06 waitpid06
# waitpid07 waitpid07
# waitpid08 waitpid08
waitpid09 waitpid09
# waitpid10 waitpid10
# waitpid11 waitpid11
# waitpid12 waitpid12
waitpid13 waitpid13
waitid01 waitid01
# waitid02 waitid02
waitid03 waitid03
waitid04 waitid04
waitid05 waitid05
waitid06 waitid06
waitid07 waitid07
waitid08 waitid08
# waitid09 waitid09
waitid10 waitid10
waitid11 waitid11
# write01 write01
write02 write02
# write03 write03
write04 write04
write05 write05
write06 write06
writev01 writev01
# writev02 writev02
# writev05 writev05
# writev06 writev06
writev07 writev07
# futex_cmp_requeue01 futex_cmp_requeue01
futex_cmp_requeue02 futex_cmp_requeue02
futex_wait01 futex_wait01
# futex_wait02 futex_wait02
# futex_wait03 futex_wait03
futex_wait04 futex_wait04
# futex_wait05 futex_wait05
futex_wake01 futex_wake01
# futex_wake02 futex_wake02
futex_wake03 futex_wake03
memfd_create01 memfd_create01
memfd_create02 memfd_create02
# copy_file_range01 copy_file_range01
# copy_file_range02 copy_file_range02
statx01 statx01
statx02 statx02
statx03 statx03
statx04 statx04
# statx06 statx06
# statx08 statx08
statx12 statx12
membarrier01 membarrier01
