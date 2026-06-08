# LTP syscall 测试项分类（testcases/kernel/syscalls）

- 总目录数：365
- 分类数：18
- 说明：包含少量“库函数/辅助测试目录”（非严格 syscall）

## I/O多路复用与事件（15）

epoll, epoll_create, epoll_create1, epoll_ctl, epoll_pwait, epoll_wait, eventfd, eventfd2, fanotify, inotify, inotify_init, poll, ppoll, pselect, select

## IPC（System V/POSIX）（7）

futex, ipc, mq_notify, mq_open, mq_timedreceive, mq_timedsend, mq_unlink

## 信号机制（25）

kill, pause, rt_sigaction, rt_sigprocmask, rt_sigqueueinfo, rt_sigsuspend, rt_sigtimedwait, rt_tgsigqueueinfo, sgetmask, sigaction, sigaltstack, sighold, signal, signalfd, signalfd4, sigpending, sigprocmask, sigrelse, sigsuspend, sigtimedwait, sigwait, sigwaitinfo, ssetmask, tgkill, tkill

## 内存管理（24）

brk, get_mempolicy, madvise, mbind, memfd_create, migrate_pages, mincore, mlock, mlock2, mlockall, mmap, move_pages, mprotect, mremap, msync, munlock, munlockall, munmap, pkeys, process_madvise, remap_file_pages, sbrk, set_mempolicy, userfaultfd

## 命名空间/cgroup/进程句柄（9）

get_robust_list, kcmp, pidfd_getfd, pidfd_open, pidfd_send_signal, set_robust_list, set_tid_address, setns, unshare

## 安全/权限/密钥（6）

add_key, capget, capset, getrandom, keyctl, request_key

## 库函数/非syscall测试辅助（25）

abort, cma, confstr, fmtmsg, fpathconf, getdomainname, gethostbyname_r, gethostid, gethostname, getpagesize, mallinfo, mallinfo2, mallopt, memcmp, memcpy, memset, nftw, pathconf, setdomainname, sethostname, string, switch, syscall, sysconf, utils

## 异步I/O（8）

io_cancel, io_destroy, io_getevents, io_pgetevents, io_setup, io_submit, io_uring, ioprio

## 挂载与文件系统配置（11）

fsconfig, fsmount, fsopen, fspick, mount, mount_setattr, move_mount, open_tree, pivot_root, umount, umount2

## 文件系统/路径与元数据（80）

access, chdir, chmod, chown, chroot, close, close_range, copy_file_range, creat, faccessat, faccessat2, fadvise, fallocate, fchdir, fchmod, fchmodat, fchown, fchownat, fdatasync, fstat, fstatat, fstatfs, fsync, ftruncate, futimesat, getcwd, getdents, lchown, link, linkat, llseek, lseek, lstat, mkdir, mkdirat, mknod, mknodat, open, openat, openat2, pread, preadv, preadv2, pwrite, pwritev, pwritev2, read, readahead, readdir, readlink, readlinkat, readv, realpath, rename, renameat, renameat2, rmdir, sendfile, splice, stat, statfs, statvfs, statx, symlink, symlinkat, sync, sync_file_range, syncfs, tee, truncate, umask, unlink, unlinkat, ustat, utime, utimensat, utimes, vmsplice, write, writev

## 文件锁/扩展属性/文件描述符（22）

dup, dup2, dup3, fcntl, fgetxattr, flistxattr, flock, fremovexattr, fsetxattr, getxattr, ioctl, lgetxattr, listxattr, llistxattr, lremovexattr, name_to_handle_at, open_by_handle_at, pipe, pipe2, removexattr, setxattr, sockioctl

## 时间/定时器（21）

adjtimex, alarm, clock_adjtime, clock_getres, clock_gettime, clock_nanosleep, clock_settime, getitimer, gettimeofday, nanosleep, setitimer, settimeofday, stime, time, timer_create, timer_delete, timer_getoverrun, timer_gettime, timer_settime, timerfd, times

## 硬件与体系结构相关（6）

cacheflush, ioperm, iopl, modify_ldt, profil, set_thread_area

## 系统控制/内核管理（18）

acct, bpf, delete_module, finit_module, init_module, membarrier, newuname, perf_event_open, quotactl, reboot, swapoff, swapon, sysctl, sysfs, sysinfo, syslog, uname, vhangup

## 网络套接字（20）

accept, accept4, bind, connect, getpeername, getsockname, getsockopt, listen, recv, recvfrom, recvmmsg, recvmsg, send, sendmmsg, sendmsg, sendto, setsockopt, socket, socketcall, socketpair

## 调度与CPU亲和（13）

getcpu, sched_get_priority_max, sched_get_priority_min, sched_getaffinity, sched_getattr, sched_getparam, sched_getscheduler, sched_rr_get_interval, sched_setaffinity, sched_setattr, sched_setparam, sched_setscheduler, sched_yield

## 进程创建/执行/退出/等待（17）

clone, clone3, execl, execle, execlp, execv, execve, execveat, execvp, exit, exit_group, fork, vfork, wait, wait4, waitid, waitpid

## 进程属性/身份/优先级/资源限制（38）

arch_prctl, getcontext, getegid, geteuid, getgid, getgroups, getpgid, getpgrp, getpid, getppid, getpriority, getresgid, getresuid, getrlimit, getrusage, getsid, gettid, getuid, nice, personality, prctl, ptrace, setegid, setfsgid, setfsuid, setgid, setgroups, setpgid, setpgrp, setpriority, setregid, setresgid, setresuid, setreuid, setrlimit, setsid, setuid, ulimit
