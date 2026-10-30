# 原本已注释的 LTP 测例

数据来源: `git show HEAD:src/ltp_testcases.sh` 中原本带 `#` 的条目。这里不包含后来为了快速出分而批量注释的测例。

## 明确未实现或占位的系统调用

| case | command | 说明 |
| --- | --- | --- |
| `delete_module02` | `delete_module02` | delete_module 未实现，当前 syscall 分发表没有 Sysno::delete_module。 |
| `fspick01` | `fspick01` | fspick 当前走 sys_dummy_fd，占位实现。 |
| `fspick02` | `fspick02` | fspick 当前走 sys_dummy_fd，占位实现。 |
| `getcpu01` | `getcpu01` | getcpu 未实现，当前 syscall 分发表没有 Sysno::getcpu。 |
| `modify_ldt01` | `modify_ldt01` | modify_ldt 未实现，当前 syscall 分发表没有 Sysno::modify_ldt。 |
| `modify_ldt02` | `modify_ldt02` | modify_ldt 未实现，当前 syscall 分发表没有 Sysno::modify_ldt。 |
| `modify_ldt03` | `modify_ldt03` | modify_ldt 未实现，当前 syscall 分发表没有 Sysno::modify_ldt。 |
| `mount_setattr01` | `mount_setattr01` | mount_setattr 未实现，当前 syscall 分发表没有 Sysno::mount_setattr。 |
| `mq_notify01` | `mq_notify01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_notify02` | `mq_notify02` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_notify03` | `mq_notify03` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_open01` | `mq_open01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_timedreceive01` | `mq_timedreceive01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_timedsend01` | `mq_timedsend01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_unlink01` | `mq_unlink01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `pivot_root01` | `pivot_root01` | pivot_root 未实现，当前 syscall 分发表没有 Sysno::pivot_root。 |
| `process_vm_readv01` | `process_vm01 -r` | process_vm_readv 未实现，当前 syscall 分发表没有 Sysno::process_vm_readv。 |
| `process_vm_readv02` | `process_vm_readv02` | process_vm_readv 未实现，当前 syscall 分发表没有 Sysno::process_vm_readv。 |
| `process_vm_readv03` | `process_vm_readv03` | process_vm_readv 未实现，当前 syscall 分发表没有 Sysno::process_vm_readv。 |
| `process_vm_writev01` | `process_vm01` | process_vm_writev 未实现，当前 syscall 分发表没有 Sysno::process_vm_writev。 |
| `process_vm_writev02` | `process_vm_writev02` | process_vm_writev 未实现，当前 syscall 分发表没有 Sysno::process_vm_writev。 |
| `quotactl04` | `quotactl04` | quotactl 未实现，当前 syscall 分发表没有 Sysno::quotactl。 |
| `quotactl08` | `quotactl08` | quotactl 未实现，当前 syscall 分发表没有 Sysno::quotactl。 |
| `quotactl09` | `quotactl09` | quotactl 未实现，当前 syscall 分发表没有 Sysno::quotactl。 |
| `request_key01` | `request_key01` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `request_key02` | `request_key02` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `request_key03` | `request_key03` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `request_key04` | `request_key04` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `request_key05` | `request_key05` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `semctl01` | `semctl01` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl02` | `semctl02` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl03` | `semctl03` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl04` | `semctl04` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl05` | `semctl05` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl06` | `semctl06` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl07` | `semctl07` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl09` | `semctl09` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semget01` | `semget01` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semget02` | `semget02` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semget05` | `semget05` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop01` | `semop01` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop02` | `semop02` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop03` | `semop03` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop04` | `semop04` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop05` | `semop05` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `setdomainname01` | `setdomainname01` | setdomainname 未实现，当前 syscall 分发表没有 Sysno::setdomainname。 |
| `setdomainname02` | `setdomainname02` | setdomainname 未实现，当前 syscall 分发表没有 Sysno::setdomainname。 |
| `setdomainname03` | `setdomainname03` | setdomainname 未实现，当前 syscall 分发表没有 Sysno::setdomainname。 |
| `setfsgid01` | `setfsgid01` | setfsgid 未实现，当前 syscall 分发表没有 Sysno::setfsgid。 |
| `setfsgid02` | `setfsgid02` | setfsgid 未实现，当前 syscall 分发表没有 Sysno::setfsgid。 |
| `setfsgid03` | `setfsgid03` | setfsgid 未实现，当前 syscall 分发表没有 Sysno::setfsgid。 |
| `setfsuid01` | `setfsuid01` | setfsuid 未实现，当前 syscall 分发表没有 Sysno::setfsuid。 |
| `setfsuid02` | `setfsuid02` | setfsuid 未实现，当前 syscall 分发表没有 Sysno::setfsuid。 |
| `setfsuid03` | `setfsuid03` | setfsuid 未实现，当前 syscall 分发表没有 Sysno::setfsuid。 |
| `setfsuid04` | `setfsuid04` | setfsuid 未实现，当前 syscall 分发表没有 Sysno::setfsuid。 |
| `sethostname01` | `sethostname01` | sethostname 未实现，当前 syscall 分发表没有 Sysno::sethostname。 |
| `sethostname02` | `sethostname02` | sethostname 未实现，当前 syscall 分发表没有 Sysno::sethostname。 |
| `sethostname03` | `sethostname03` | sethostname 未实现，当前 syscall 分发表没有 Sysno::sethostname。 |
| `setitimer01` | `setitimer01` | setitimer 未实现，当前 syscall 分发表没有 Sysno::setitimer。 |
| `setitimer02` | `setitimer02` | setitimer 未实现，当前 syscall 分发表没有 Sysno::setitimer。 |
| `setns01` | `setns01` | setns 未实现，当前 syscall 分发表没有 Sysno::setns。 |
| `setns02` | `setns02` | setns 未实现，当前 syscall 分发表没有 Sysno::setns。 |
| `tee01` | `tee01` | tee 未实现，当前 syscall 分发表没有 Sysno::tee。 |
| `tee02` | `tee02` | tee 未实现，当前 syscall 分发表没有 Sysno::tee。 |
| `stime01` | `stime01` | stime 未实现，当前 syscall 分发表没有 Sysno::stime。 |
| `stime02` | `stime02` | stime 未实现，当前 syscall 分发表没有 Sysno::stime。 |
| `swapoff01` | `swapoff01` | swapoff 未实现，当前 syscall 分发表没有 Sysno::swapoff。 |
| `swapoff02` | `swapoff02` | swapoff 未实现，当前 syscall 分发表没有 Sysno::swapoff。 |
| `swapon01` | `swapon01` | swapon 未实现，当前 syscall 分发表没有 Sysno::swapon。 |
| `swapon02` | `swapon02` | swapon 未实现，当前 syscall 分发表没有 Sysno::swapon。 |
| `swapon03` | `swapon03` | swapon 未实现，当前 syscall 分发表没有 Sysno::swapon。 |
| `sync_file_range01` | `sync_file_range01` | sync_file_range 已分发表，但当前实现接近空实现/占位，需按未完整实现记录。 |
| `sync_file_range02` | `sync_file_range02` | sync_file_range 已分发表，但当前实现接近空实现/占位，需按未完整实现记录。 |
| `tgkill01` | `tgkill01` | tgkill 已分发表；若注释原因是未实现，需复核，当前代码已有 sys_tgkill。 |
| `tgkill02` | `tgkill02` | tgkill 已分发表；若注释原因是未实现，需复核，当前代码已有 sys_tgkill。 |
| `tgkill03` | `tgkill03` | tgkill 已分发表；若注释原因是未实现，需复核，当前代码已有 sys_tgkill。 |
| `userfaultfd01` | `userfaultfd01` | userfaultfd 当前显式返回 EPERM，占位禁用。 |
| `vhangup01` | `vhangup01` | vhangup 未实现，当前 syscall 分发表没有 Sysno::vhangup。 |
| `vhangup02` | `vhangup02` | vhangup 未实现，当前 syscall 分发表没有 Sysno::vhangup。 |
| `perf_event_open01` | `perf_event_open01` | perf_event_open 当前走 sys_dummy_fd，占位实现。 |
| `perf_event_open02` | `perf_event_open02` | perf_event_open 当前走 sys_dummy_fd，占位实现。 |
| `futex_waitv02` | `futex_waitv02` | futex_waitv 未实现，当前 syscall 分发表没有 Sysno::futex_waitv。 |
| `io_uring02` | `io_uring02` | io_uring_setup 当前显式返回 Unsupported。 |

## 原本已注释完整清单

| case | command | 未实现/占位说明 |
| --- | --- | --- |
| `chdir01A` | `symlink01 -T chdir01` |  |
| `chmod01A` | `symlink01 -T chmod01` |  |
| `chmod05` | `chmod05` |  |
| `chmod07` | `chmod07` |  |
| `chroot01` | `chroot01` |  |
| `chroot04` | `chroot04` |  |
| `clock_nanosleep02` | `clock_nanosleep02` |  |
| `clock_nanosleep03` | `clock_nanosleep03` |  |
| `clock_gettime03` | `clock_gettime03` |  |
| `clock_gettime04` | `clock_gettime04` |  |
| `leapsec01` | `leapsec01` |  |
| `clock_settime01` | `clock_settime01` |  |
| `clock_settime02` | `clock_settime02` |  |
| `clock_settime03` | `clock_settime03` |  |
| `clone05` | `clone05` |  |
| `clone09` | `clone09` |  |
| `clone303` | `clone303` |  |
| `connect02` | `connect02` |  |
| `creat04` | `creat04` |  |
| `creat07` | `creat07` |  |
| `creat08` | `creat08` |  |
| `delete_module02` | `delete_module02` | delete_module 未实现，当前 syscall 分发表没有 Sysno::delete_module。 |
| `dup05` | `dup05` |  |
| `epoll_pwait03` | `epoll_pwait03` |  |
| `fallocate02` | `fallocate02` |  |
| `fallocate05` | `fallocate05` |  |
| `fallocate06` | `fallocate06` |  |
| `posix_fadvise02` | `posix_fadvise02` |  |
| `posix_fadvise02_64` | `posix_fadvise02_64` |  |
| `posix_fadvise04` | `posix_fadvise04` |  |
| `posix_fadvise04_64` | `posix_fadvise04_64` |  |
| `fchdir03` | `fchdir03` |  |
| `fchmod02` | `fchmod02` |  |
| `fchmod05` | `fchmod05` |  |
| `fchmod06` | `fchmod06` |  |
| `fcntl01` | `fcntl01` |  |
| `fcntl01_64` | `fcntl01_64` |  |
| `fcntl07` | `fcntl07` |  |
| `fcntl07_64` | `fcntl07_64` |  |
| `fdatasync03` | `fdatasync03` |  |
| `flock02` | `flock02` |  |
| `fmtmsg01` | `fmtmsg01` |  |
| `fork05` | `fork05` |  |
| `fork13` | `fork13` |  |
| `fork14` | `fork14` |  |
| `fspick01` | `fspick01` | fspick 当前走 sys_dummy_fd，占位实现。 |
| `fspick02` | `fspick02` | fspick 当前走 sys_dummy_fd，占位实现。 |
| `fsync02` | `fsync02` |  |
| `fsync03` | `fsync03` |  |
| `fsync04` | `fsync04` |  |
| `ftruncate03` | `ftruncate03` |  |
| `ftruncate03_64` | `ftruncate03_64` |  |
| `getcpu01` | `getcpu01` | getcpu 未实现，当前 syscall 分发表没有 Sysno::getcpu。 |
| `getcwd04` | `getcwd04` |  |
| `getrlimit03` | `getrlimit03` |  |
| `getrusage04` | `getrusage04` |  |
| `ioctl01` | `ioctl01` |  |
| `ioctl03` | `ioctl03` |  |
| `keyctl01` | `keyctl01` |  |
| `keyctl02` | `keyctl02` |  |
| `keyctl03` | `keyctl03` |  |
| `keyctl04` | `keyctl04` |  |
| `keyctl05` | `keyctl05` |  |
| `keyctl06` | `keyctl06` |  |
| `keyctl07` | `keyctl07` |  |
| `keyctl08` | `keyctl08` |  |
| `keyctl09` | `keyctl09` |  |
| `kcmp01` | `kcmp01` |  |
| `kcmp02` | `kcmp02` |  |
| `kcmp03` | `kcmp03` |  |
| `lseek02` | `lseek02` |  |
| `lseek11` | `lseek11` |  |
| `lstat01A` | `symlink01 -T lstat01` |  |
| `lstat01A_64` | `symlink01 -T lstat01_64` |  |
| `mlockall03` | `mlockall03` |  |
| `mlock02` | `mlock02` |  |
| `mlock05` | `mlock05` |  |
| `mlock201` | `mlock201` |  |
| `mlock203` | `mlock203` |  |
| `qmm01` | `mmap001 -m 1` |  |
| `mmap16` | `mmap16` |  |
| `modify_ldt01` | `modify_ldt01` | modify_ldt 未实现，当前 syscall 分发表没有 Sysno::modify_ldt。 |
| `modify_ldt02` | `modify_ldt02` | modify_ldt 未实现，当前 syscall 分发表没有 Sysno::modify_ldt。 |
| `modify_ldt03` | `modify_ldt03` | modify_ldt 未实现，当前 syscall 分发表没有 Sysno::modify_ldt。 |
| `mount_setattr01` | `mount_setattr01` | mount_setattr 未实现，当前 syscall 分发表没有 Sysno::mount_setattr。 |
| `mprotect01` | `mprotect01` |  |
| `mprotect02` | `mprotect02` |  |
| `mprotect03` | `mprotect03` |  |
| `mprotect04` | `mprotect04` |  |
| `mprotect05` | `mprotect05` |  |
| `mq_notify01` | `mq_notify01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_notify02` | `mq_notify02` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_notify03` | `mq_notify03` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_open01` | `mq_open01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_timedreceive01` | `mq_timedreceive01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_timedsend01` | `mq_timedsend01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mq_unlink01` | `mq_unlink01` | POSIX message queue syscall 未实现，当前没有 mq_* syscall 分发表项。 |
| `mremap01` | `mremap01` |  |
| `mremap03` | `mremap03` |  |
| `mremap04` | `mremap04` |  |
| `mremap06` | `mremap06` |  |
| `msgctl01` | `msgctl01` |  |
| `msgctl02` | `msgctl02` |  |
| `msgctl04` | `msgctl04` |  |
| `msgctl06` | `msgctl06` |  |
| `msgstress01` | `msgstress01` |  |
| `msgget01` | `msgget01` |  |
| `msgget02` | `msgget02` |  |
| `msgget03` | `msgget03` |  |
| `msgget04` | `msgget04` |  |
| `msgget05` | `msgget05` |  |
| `msgrcv01` | `msgrcv01` |  |
| `msgrcv02` | `msgrcv02` |  |
| `msgrcv03` | `msgrcv03` |  |
| `msgrcv05` | `msgrcv05` |  |
| `msgrcv06` | `msgrcv06` |  |
| `msgrcv07` | `msgrcv07` |  |
| `msgrcv08` | `msgrcv08` |  |
| `msgsnd01` | `msgsnd01` |  |
| `msgsnd02` | `msgsnd02` |  |
| `msgsnd05` | `msgsnd05` |  |
| `msgsnd06` | `msgsnd06` |  |
| `msync03` | `msync03` |  |
| `msync04` | `msync04` |  |
| `munlock02` | `munlock02` |  |
| `munlockall01` | `munlockall01` |  |
| `nanosleep01` | `nanosleep01` |  |
| `name_to_handle_at01` | `name_to_handle_at01` |  |
| `name_to_handle_at02` | `name_to_handle_at02` |  |
| `nice01` | `nice01` |  |
| `nice02` | `nice02` |  |
| `nice03` | `nice03` |  |
| `nice04` | `nice04` |  |
| `nice05` | `nice05` |  |
| `mincore01` | `mincore01` |  |
| `mincore02` | `mincore02` |  |
| `mincore03` | `mincore03` |  |
| `mincore04` | `mincore04` |  |
| `madvise02` | `madvise02` |  |
| `madvise03` | `madvise03` |  |
| `madvise08` | `madvise08` |  |
| `pause01` | `pause01` |  |
| `pause02` | `pause02` |  |
| `pause03` | `pause03` |  |
| `personality01` | `personality01` |  |
| `personality02` | `personality02` |  |
| `pidfd_getfd01` | `pidfd_getfd01` |  |
| `pidfd_getfd02` | `pidfd_getfd02` |  |
| `pidfd_send_signal01` | `pidfd_send_signal01` |  |
| `pidfd_send_signal02` | `pidfd_send_signal02` |  |
| `pidfd_send_signal03` | `pidfd_send_signal03` |  |
| `pipe13` | `pipe13` |  |
| `pipe14` | `pipe14` |  |
| `pipe15` | `pipe15` |  |
| `pipe2_01` | `pipe2_01` |  |
| `pipe2_02` | `pipe2_02` |  |
| `pipe2_04` | `pipe2_04` |  |
| `pivot_root01` | `pivot_root01` | pivot_root 未实现，当前 syscall 分发表没有 Sysno::pivot_root。 |
| `poll02` | `poll02` |  |
| `ppoll01` | `ppoll01` |  |
| `prctl01` | `prctl01` |  |
| `prctl03` | `prctl03` |  |
| `prctl04` | `prctl04` |  |
| `prctl06` | `prctl06` |  |
| `prctl08` | `prctl08` |  |
| `prctl09` | `prctl09` |  |
| `preadv03` | `preadv03` |  |
| `preadv03_64` | `preadv03_64` |  |
| `preadv203` | `preadv203` |  |
| `preadv203_64` | `preadv203_64` |  |
| `process_vm_readv01` | `process_vm01 -r` | process_vm_readv 未实现，当前 syscall 分发表没有 Sysno::process_vm_readv。 |
| `process_vm_readv02` | `process_vm_readv02` | process_vm_readv 未实现，当前 syscall 分发表没有 Sysno::process_vm_readv。 |
| `process_vm_readv03` | `process_vm_readv03` | process_vm_readv 未实现，当前 syscall 分发表没有 Sysno::process_vm_readv。 |
| `process_vm_writev01` | `process_vm01` | process_vm_writev 未实现，当前 syscall 分发表没有 Sysno::process_vm_writev。 |
| `process_vm_writev02` | `process_vm_writev02` | process_vm_writev 未实现，当前 syscall 分发表没有 Sysno::process_vm_writev。 |
| `dirtyc0w` | `dirtyc0w` |  |
| `dirtyc0w_shmem` | `dirtyc0w_shmem` |  |
| `dirtypipe` | `dirtypipe` |  |
| `pselect01` | `pselect01` |  |
| `pselect01_64` | `pselect01_64` |  |
| `quotactl04` | `quotactl04` | quotactl 未实现，当前 syscall 分发表没有 Sysno::quotactl。 |
| `quotactl08` | `quotactl08` | quotactl 未实现，当前 syscall 分发表没有 Sysno::quotactl。 |
| `quotactl09` | `quotactl09` | quotactl 未实现，当前 syscall 分发表没有 Sysno::quotactl。 |
| `read03` | `read03` |  |
| `readlink01A` | `symlink01 -T readlink01` |  |
| `readlinkat01` | `readlinkat01` |  |
| `realpath01` | `realpath01` |  |
| `reboot01` | `reboot01` |  |
| `reboot02` | `reboot02` |  |
| `recvmmsg01` | `recvmmsg01` |  |
| `remap_file_pages01` | `remap_file_pages01` |  |
| `remap_file_pages02` | `remap_file_pages02` |  |
| `rename01A` | `symlink01 -T rename01` |  |
| `rename07` | `rename07` |  |
| `rename09` | `rename09` |  |
| `rename12` | `rename12` |  |
| `renameat202` | `renameat202 -i 10` |  |
| `request_key01` | `request_key01` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `request_key02` | `request_key02` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `request_key03` | `request_key03` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `request_key04` | `request_key04` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `request_key05` | `request_key05` | request_key 未实现，当前 syscall 分发表没有 Sysno::request_key。 |
| `rmdir03` | `rmdir03` |  |
| `rmdir03A` | `symlink01 -T rmdir03` |  |
| `rt_sigqueueinfo01` | `rt_sigqueueinfo01` |  |
| `rt_sigsuspend01` | `rt_sigsuspend01` |  |
| `rt_sigtimedwait01` | `rt_sigtimedwait01` |  |
| `rt_tgsigqueueinfo01` | `rt_tgsigqueueinfo01` |  |
| `sched_get_priority_max01` | `sched_get_priority_max01` |  |
| `sched_get_priority_max02` | `sched_get_priority_max02` |  |
| `sched_get_priority_min01` | `sched_get_priority_min01` |  |
| `sched_get_priority_min02` | `sched_get_priority_min02` |  |
| `sched_getparam01` | `sched_getparam01` |  |
| `sched_getparam03` | `sched_getparam03` |  |
| `sched_rr_get_interval01` | `sched_rr_get_interval01` |  |
| `sched_rr_get_interval02` | `sched_rr_get_interval02` |  |
| `sched_rr_get_interval03` | `sched_rr_get_interval03` |  |
| `sched_setparam01` | `sched_setparam01` |  |
| `sched_setparam02` | `sched_setparam02` |  |
| `sched_setparam03` | `sched_setparam03` |  |
| `sched_setparam04` | `sched_setparam04` |  |
| `sched_setparam05` | `sched_setparam05` |  |
| `sched_getscheduler02` | `sched_getscheduler02` |  |
| `sched_setscheduler01` | `sched_setscheduler01` |  |
| `sched_setscheduler02` | `sched_setscheduler02` |  |
| `sched_setscheduler03` | `sched_setscheduler03` |  |
| `sched_setscheduler04` | `sched_setscheduler04` |  |
| `sched_setattr01` | `sched_setattr01` |  |
| `sched_getattr01` | `sched_getattr01` |  |
| `sched_getattr02` | `sched_getattr02` |  |
| `select01` | `select01` |  |
| `select02` | `select02` |  |
| `select04` | `select04` |  |
| `semctl01` | `semctl01` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl02` | `semctl02` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl03` | `semctl03` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl04` | `semctl04` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl05` | `semctl05` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl06` | `semctl06` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl07` | `semctl07` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semctl09` | `semctl09` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semget01` | `semget01` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semget02` | `semget02` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semget05` | `semget05` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop01` | `semop01` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop02` | `semop02` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop03` | `semop03` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop04` | `semop04` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `semop05` | `semop05` | System V semaphore syscall 未实现，当前没有 semctl/semget/semop 分发表项。 |
| `send01` | `send01` |  |
| `send02` | `send02` |  |
| `sendmsg01` | `sendmsg01` |  |
| `sendmsg02` | `sendmsg02` |  |
| `sendmsg03` | `sendmsg03` |  |
| `sendmmsg01` | `sendmmsg01` |  |
| `sendto02` | `sendto02` |  |
| `sendto03` | `sendto03` |  |
| `setdomainname01` | `setdomainname01` | setdomainname 未实现，当前 syscall 分发表没有 Sysno::setdomainname。 |
| `setdomainname02` | `setdomainname02` | setdomainname 未实现，当前 syscall 分发表没有 Sysno::setdomainname。 |
| `setdomainname03` | `setdomainname03` | setdomainname 未实现，当前 syscall 分发表没有 Sysno::setdomainname。 |
| `setfsgid01` | `setfsgid01` | setfsgid 未实现，当前 syscall 分发表没有 Sysno::setfsgid。 |
| `setfsgid02` | `setfsgid02` | setfsgid 未实现，当前 syscall 分发表没有 Sysno::setfsgid。 |
| `setfsgid03` | `setfsgid03` | setfsgid 未实现，当前 syscall 分发表没有 Sysno::setfsgid。 |
| `setfsuid01` | `setfsuid01` | setfsuid 未实现，当前 syscall 分发表没有 Sysno::setfsuid。 |
| `setfsuid02` | `setfsuid02` | setfsuid 未实现，当前 syscall 分发表没有 Sysno::setfsuid。 |
| `setfsuid03` | `setfsuid03` | setfsuid 未实现，当前 syscall 分发表没有 Sysno::setfsuid。 |
| `setfsuid04` | `setfsuid04` | setfsuid 未实现，当前 syscall 分发表没有 Sysno::setfsuid。 |
| `setgid02` | `setgid02` |  |
| `setegid01` | `setegid01` |  |
| `setegid02` | `setegid02` |  |
| `setgroups03` | `setgroups03` |  |
| `sethostname01` | `sethostname01` | sethostname 未实现，当前 syscall 分发表没有 Sysno::sethostname。 |
| `sethostname02` | `sethostname02` | sethostname 未实现，当前 syscall 分发表没有 Sysno::sethostname。 |
| `sethostname03` | `sethostname03` | sethostname 未实现，当前 syscall 分发表没有 Sysno::sethostname。 |
| `setitimer01` | `setitimer01` | setitimer 未实现，当前 syscall 分发表没有 Sysno::setitimer。 |
| `setitimer02` | `setitimer02` | setitimer 未实现，当前 syscall 分发表没有 Sysno::setitimer。 |
| `setns01` | `setns01` | setns 未实现，当前 syscall 分发表没有 Sysno::setns。 |
| `setns02` | `setns02` | setns 未实现，当前 syscall 分发表没有 Sysno::setns。 |
| `setpgid01` | `setpgid01` |  |
| `setpgid02` | `setpgid02` |  |
| `setpgid03` | `setpgid03` |  |
| `setpriority01` | `setpriority01` |  |
| `setpriority02` | `setpriority02` |  |
| `setrlimit01` | `setrlimit01` |  |
| `setrlimit02` | `setrlimit02` |  |
| `setrlimit03` | `setrlimit03` |  |
| `setrlimit04` | `setrlimit04` |  |
| `setrlimit05` | `setrlimit05` |  |
| `setrlimit06` | `setrlimit06` |  |
| `setsid01` | `setsid01` |  |
| `setsockopt02` | `setsockopt02` |  |
| `setsockopt04` | `setsockopt04` |  |
| `setsockopt05` | `setsockopt05` |  |
| `setsockopt06` | `setsockopt06` |  |
| `setsockopt07` | `setsockopt07` |  |
| `setsockopt09` | `setsockopt09` |  |
| `setsockopt10` | `setsockopt10` |  |
| `settimeofday01` | `settimeofday01` |  |
| `setuid03` | `setuid03` |  |
| `setuid04` | `setuid04` |  |
| `shmat01` | `shmat01` |  |
| `shmat02` | `shmat02` |  |
| `shmat04` | `shmat04` |  |
| `shmctl01` | `shmctl01` |  |
| `shmctl02` | `shmctl02` |  |
| `shmctl03` | `shmctl03` |  |
| `shmctl04` | `shmctl04` |  |
| `shmctl05` | `shmctl05` |  |
| `shmdt01` | `shmdt01` |  |
| `shmdt02` | `shmdt02` |  |
| `shmget02` | `shmget02` |  |
| `shmget03` | `shmget03` |  |
| `shmget04` | `shmget04` |  |
| `shmget05` | `shmget05` |  |
| `shmget06` | `shmget06` |  |
| `sigrelse01` | `sigrelse01` |  |
| `sigsuspend01` | `sigsuspend01` |  |
| `sigtimedwait01` | `sigtimedwait01` |  |
| `sigwaitinfo01` | `sigwaitinfo01` |  |
| `splice06` | `splice06` |  |
| `tee01` | `tee01` | tee 未实现，当前 syscall 分发表没有 Sysno::tee。 |
| `tee02` | `tee02` | tee 未实现，当前 syscall 分发表没有 Sysno::tee。 |
| `stat04` | `symlink01 -T stat04` |  |
| `stat04_64` | `symlink01 -T stat04_64` |  |
| `statfs03` | `statfs03` |  |
| `statfs03_64` | `statfs03_64` |  |
| `stime01` | `stime01` | stime 未实现，当前 syscall 分发表没有 Sysno::stime。 |
| `stime02` | `stime02` | stime 未实现，当前 syscall 分发表没有 Sysno::stime。 |
| `swapoff01` | `swapoff01` | swapoff 未实现，当前 syscall 分发表没有 Sysno::swapoff。 |
| `swapoff02` | `swapoff02` | swapoff 未实现，当前 syscall 分发表没有 Sysno::swapoff。 |
| `swapon01` | `swapon01` | swapon 未实现，当前 syscall 分发表没有 Sysno::swapon。 |
| `swapon02` | `swapon02` | swapon 未实现，当前 syscall 分发表没有 Sysno::swapon。 |
| `swapon03` | `swapon03` | swapon 未实现，当前 syscall 分发表没有 Sysno::swapon。 |
| `symlinkat01` | `symlinkat01` |  |
| `sync01` | `sync01` |  |
| `syncfs01` | `syncfs01` |  |
| `sync_file_range01` | `sync_file_range01` | sync_file_range 已分发表，但当前实现接近空实现/占位，需按未完整实现记录。 |
| `sync_file_range02` | `sync_file_range02` | sync_file_range 已分发表，但当前实现接近空实现/占位，需按未完整实现记录。 |
| `sysinfo03` | `sysinfo03` |  |
| `syslog11` | `syslog11` |  |
| `syslog12` | `syslog12` |  |
| `tgkill01` | `tgkill01` | tgkill 已分发表；若注释原因是未实现，需复核，当前代码已有 sys_tgkill。 |
| `tgkill02` | `tgkill02` | tgkill 已分发表；若注释原因是未实现，需复核，当前代码已有 sys_tgkill。 |
| `tgkill03` | `tgkill03` | tgkill 已分发表；若注释原因是未实现，需复核，当前代码已有 sys_tgkill。 |
| `unlink01` | `symlink01 -T unlink01` |  |
| `unlink09` | `unlink09` |  |
| `umount03` | `umount03` |  |
| `umount2_01` | `umount2_01` |  |
| `userfaultfd01` | `userfaultfd01` | userfaultfd 当前显式返回 EPERM，占位禁用。 |
| `vfork01` | `vfork01` |  |
| `vhangup01` | `vhangup01` | vhangup 未实现，当前 syscall 分发表没有 Sysno::vhangup。 |
| `vhangup02` | `vhangup02` | vhangup 未实现，当前 syscall 分发表没有 Sysno::vhangup。 |
| `vmsplice01` | `vmsplice01` |  |
| `vmsplice02` | `vmsplice02` |  |
| `vmsplice03` | `vmsplice03` |  |
| `vmsplice04` | `vmsplice04` |  |
| `perf_event_open01` | `perf_event_open01` | perf_event_open 当前走 sys_dummy_fd，占位实现。 |
| `perf_event_open02` | `perf_event_open02` | perf_event_open 当前走 sys_dummy_fd，占位实现。 |
| `futex_waitv02` | `futex_waitv02` | futex_waitv 未实现，当前 syscall 分发表没有 Sysno::futex_waitv。 |
| `futex_wait_bitset01` | `futex_wait_bitset01` |  |
| `copy_file_range02` | `copy_file_range02` |  |
| `copy_file_range03` | `copy_file_range03` |  |
| `membarrier01` | `membarrier01` |  |
| `io_uring02` | `io_uring02` | io_uring_setup 当前显式返回 Unsupported。 |
| `creat07_child` | `creat07_child` |  |
| `mallinfo01` | `mallinfo01` |  |
| `mmap10` | `mmap10` |  |
| `mount03_suid_child` | `mount03_suid_child` |  |
| `open12_child` | `open12_child` |  |
| `openat02_child` | `openat02_child` |  |
| `pipe2_02_child` | `pipe2_02_child` |  |
| `prctl06_execve` | `prctl06_execve` |  |
| `set_mempolicy05` | `set_mempolicy05` |  |
| `setgroups04` | `setgroups04` |  |
| `setgroups04_16` | `setgroups04_16` |  |
| `setpgid03_child` | `setpgid03_child` |  |
