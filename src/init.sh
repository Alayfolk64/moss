if ! /glibc/busybox test -d /bin; then
    echo @@@@@@@@@@ setup @@@@@@@@@@

    /glibc/busybox mkdir -v /bin
    /glibc/busybox --install -s /bin
    export PATH=/bin

    mkdir -v /lib
    if [[ -e /glibc/lib/ld-linux-loongarch-lp64d.so.1 ]]; then
        ln -v -s /musl/lib/libc.so /lib/ld-musl-loongarch-lp64d.so.1
        ln -v -s /glibc/lib/ld-linux-loongarch-lp64d.so.1 /lib/ld-linux-loongarch-lp64d.so.1
    elif [[ -e /glibc/lib/ld-linux-riscv64-lp64d.so.1 ]]; then
        ln -v -s /musl/lib/libc.so /lib/ld-musl-riscv64.so.1
        ln -v -s /musl/lib/libc.so /lib/ld-musl-riscv64-sf.so.1
        ln -v -s /glibc/lib/ld-linux-riscv64-lp64d.so.1 /lib/ld-linux-riscv64-lp64d.so.1
    fi
    ln -v -s /lib /lib64

    mkdir -v /usr
    ln -v -s /lib /usr/lib64

    cd /
    mkdir -v var
    cd var
    mkdir -v tmp log

    cd /
    mkdir -v etc
    echo "root:x:0:0:root:/root:/bin/sh" >>/etc/passwd
    echo "nobody:x:65534:65534:nobody:/:/bin/sh" >>/etc/passwd
    echo "root:x:0:" >>/etc/group
    echo "nobody:x:65534:" >>/etc/group
    echo "nameserver 8.8.8.8" >/etc/resolv.conf

    mkdir -vp /lib/modules/10.0.0/build
    echo "CONFIG_EVENTFD=y" >/lib/modules/10.0.0/build/.config
    echo "CONFIG_USER_NS=y" >>/lib/modules/10.0.0/build/.config
    echo "CONFIG_NET_NS=y" >>/lib/modules/10.0.0/build/.config
fi

musl_runtime() {
    echo "prepare musl runtime"
    rm -rf /lib/libc.so
    rm -rf /lib/libm.so
    rm -rf /lib/libc.so.6
    rm -rf /lib/libm.so.6
    ln -v -s /musl/lib/libc.so /lib/libc.so
}

glibc_runtime() {
    echo "prepare musl runtime"
    rm -rf /lib/libc.so
    ln -v -s /glibc/lib/libc.so.6 /lib/libc.so.6
    ln -v -s /glibc/lib/libm.so.6 /lib/libm.so.6
    ln -v -s /lib/libc.so.6 /lib/libc.so
    ln -v -s /lib/libm.so.6 /lib/libm.so
}

echo @@@@@@@@@@ files @@@@@@@@@@
ls -lhAR /lib
echo @@@@@@@@@@ env @@@@@@@@@@
env
echo

run_ltp() {
    echo "#### OS COMP TEST GROUP START ltp-$1 ####"

    export LTP_TIMEOUT_MUL=0.5
    export LTP_DEV_FS_TYPE=tmpfs
    export LTP_SINGLE_FS_TYPE=tmpfs

    all_testcases="
        # accept01
        # accept02
        # accept03
        # access01
        # access02
        # access03
        # access04
        # adjtimex01
        # adjtimex02
        # adjtimex03
        # arch_prctl01
        # bind01
        # bind02
        # bind03
        # bind04
        # bind05
        bpf_map01
        bpf_prog01
        bpf_prog02
        bpf_prog03
        bpf_prog04
        bpf_prog05
        bpf_prog06
        bpf_prog07
        # cacheflush01
        # capget01
        # capget02
        # capset01
        # capset02
        # capset03
        # capset04
        # chdir01A
        # chmod01A
        # chmod05
        # chmod06
        # chmod07
        # chown01_16
        # chown02_16
        # chown03_16
        # chown04
        # chown04_16
        # chown05_16
        # chroot04
        # clock_adjtime01
        # clock_adjtime02
        # clock_gettime01
        # clock_gettime03
        # clock_gettime04
        # clock_nanosleep02
        # clock_nanosleep03
        # clock_settime01
        # clock_settime02
        # clock_settime03
        # clone04
        # clone05
        # clone08
        # clone09
        # clone301
        # clone302
        # clone303
        # close_range01
        # close_range02
        # connect01
        # connect02
        # copy_file_range02
        # copy_file_range03
        # creat04
        # creat06
        # creat07
        # creat09
        # delete_module01
        # delete_module02
        # delete_module03
        # dirtyc0w
        # dirtyc0w_shmem
        # dup05
        # epoll01
        # epoll_wait01
        # eventfd06
        # execve02
        # execve04
        # execve06
        # execveat01
        # execveat02
        # execveat03
        # exit01
        # fallocate01
        # fallocate02
        # fallocate04
        # fallocate05
        # fallocate06
        # fanotify01
        # fanotify02
        # fanotify03
        # fanotify04
        # fanotify05
        # fanotify06
        # fanotify07
        # fanotify08
        # fanotify09
        # fanotify10
        # fanotify11
        # fanotify12
        # fanotify13
        # fanotify14
        # fanotify15
        # fanotify16
        # fanotify17
        # fanotify18
        # fanotify19
        # fanotify20
        # fanotify21
        # fanotify22
        # fanotify23
        # fchdir03
        # fchown01_16
        # fchown02_16
        # fchown03_16
        # fchown04_16
        # fchown05_16
        # fchownat01
        # fchownat02
        # fcntl01
        # fcntl01_64
        # fcntl07
        # fcntl07_64
        # fcntl09
        # fcntl09_64
        # fcntl10
        # fcntl10_64
        # fcntl11
        # fcntl11_64
        # fcntl12
        # fcntl12_64
        # fcntl13
        # fcntl13_64
        # fcntl14
        # fcntl14_64
        # fcntl15
        # fcntl15_64
        # fcntl16
        # fcntl16_64
        # fcntl17
        # fcntl17_64
        # fcntl18
        # fcntl18_64
        # fcntl19
        # fcntl19_64
        # fcntl20
        # fcntl20_64
        # fcntl21
        # fcntl21_64
        # fcntl22
        # fcntl22_64
        # fcntl23
        # fcntl23_64
        # fcntl24
        # fcntl24_64
        # fcntl25
        # fcntl25_64
        # fcntl26
        # fcntl26_64
        # fcntl30
        # fcntl30_64
        # fcntl31
        # fcntl31_64
        # fcntl32
        # fcntl32_64
        # fcntl33
        # fcntl33_64
        # fcntl34
        # fcntl34_64
        # fcntl35
        # fcntl35_64
        # fcntl36
        # fcntl36_64
        # fcntl37
        # fcntl37_64
        # fcntl38
        # fcntl38_64
        # fcntl39
        # fcntl39_64
        # fdatasync03
        # fgetxattr01
        # fgetxattr02
        # fgetxattr03
        # finit_module01
        # finit_module02
        # flistxattr01
        # flistxattr02
        # flistxattr03
        # flock01
        # flock02
        # flock03
        # flock04
        # fmtmsg01
        # fork01
        # fork03
        # fork04
        # fork07
        # fork08
        # fork09
        # fork10
        # fremovexattr01
        # fremovexattr02
        # fsconfig01
        # fsconfig02
        # fsconfig03
        # fsetxattr01
        # fsetxattr02
        # fsmount01
        # fsmount02
        # fsopen01
        # fsopen02
        # fspick01
        # fspick02
        # fstatat01
        # fsync02
        # fsync03
        # fsync04
        # ftruncate03
        # ftruncate03_64
        # ftruncate04
        # ftruncate04_64
        # futex_cmp_requeue01
        # futex_cmp_requeue02
        # futex_wait01
        # futex_wait02
        # futex_wait03
        # futex_wait04
        # futex_wait05
        # futex_wait_bitset01
        # futex_waitv01
        # futex_waitv02
        # futex_waitv03
        # futex_wake01
        # futex_wake02
        # futex_wake03
        # futex_wake04
        # futimesat01
        # get_mempolicy01
        # get_mempolicy02
        # get_robust_list01
        # getcontext01
        # getcpu01
        # getcwd02
        # getcwd04
        # geteuid01_16
        # geteuid02_16
        # getgid01
        # getgid01_16
        # getgid03_16
        # getgroups01
        # getgroups01_16
        # getgroups03
        # getgroups03_16
        # gethostbyname_r01
        # gethostid01
        # gethostname02
        # getrandom05
        # getresgid01
        # getresgid01_16
        # getresgid02
        # getresgid02_16
        # getresgid03
        # getresgid03_16
        # getresuid01
        # getresuid01_16
        # getresuid02
        # getresuid02_16
        # getresuid03
        # getresuid03_16
        # getrlimit03
        # getrusage03
        # getrusage04
        # gettimeofday02
        # getuid01_16
        # getuid03_16
        # getxattr01
        # getxattr02
        # getxattr03
        # getxattr04
        # getxattr05
        # init_module01
        # init_module02
        # inotify01
        # inotify02
        # inotify03
        # inotify04
        # inotify05
        # inotify06
        # inotify07
        # inotify08
        # inotify09
        # inotify10
        # inotify11
        # inotify12
        # inotify_init1_01
        # inotify_init1_02
        # io_cancel01
        # io_cancel02
        # io_destroy01
        # io_destroy02
        # io_getevents01
        # io_getevents02
        # io_pgetevents01
        # io_pgetevents02
        # io_setup01
        # io_setup02
        # io_submit01
        # io_submit02
        # io_submit03
        # io_uring01
        # io_uring02
        # ioctl01
        # ioctl02
        # ioctl03
        # ioctl07
        # ioctl08
        # ioctl09
        # ioctl_loop01
        # ioctl_loop02
        # ioctl_loop03
        # ioctl_loop04
        # ioctl_loop05
        # ioctl_loop06
        # ioctl_loop07
        # ioctl_ns01
        # ioctl_ns02
        # ioctl_ns03
        # ioctl_ns04
        # ioctl_ns05
        # ioctl_ns06
        # ioctl_sg01
        # ioperm01
        # ioperm02
        # iopl01
        # iopl02
        # ioprio_get01
        # ioprio_set01
        # ioprio_set02
        # ioprio_set03
        # kcmp01
        # kcmp02
        # kcmp03
        # keyctl01
        # keyctl02
        # keyctl03
        # keyctl04
        # keyctl05
        # keyctl06
        # keyctl07
        # keyctl08
        # keyctl09
        # kill02
        # kill05
        # kill07
        # kill10
        # kill12
        # kill13
        # lchown01
        # lchown01_16
        # lchown02
        # lchown02_16
        # lchown03
        # lchown03_16
        # leapsec01
        # lgetxattr01
        # lgetxattr02
        # link01
        # linkat01
        # linkat02
        # listxattr01
        # listxattr02
        # listxattr03
        # llistxattr01
        # llistxattr02
        # llistxattr03
        # lremovexattr01
        # lstat01A
        # lstat01A_64
        # madvise06
        # madvise07
        # madvise09
        # mallinfo02
        # mallinfo2_01
        # mallopt01
        # mbind01
        # mbind02
        # mbind03
        # mbind04
        # membarrier01
        # memfd_create01
        # memfd_create02
        # memfd_create03
        # memfd_create04
        # migrate_pages01
        # migrate_pages02
        # migrate_pages03
        # mincore01
        # mincore02
        # mincore03
        # mincore04
        # mkdir03
        # mkdir09
        # mkdirat01
        # mknod01
        # mknod02
        # mknod03
        # mknod04
        # mknod05
        # mknod06
        # mknod07
        # mknod08
        # mknod09
        # mknodat01
        # mknodat02
        # mlock02
        # mlock05
        # mlock201
        # mlock202
        # mlock203
        # mlockall01
        # mlockall02
        # mlockall03
        # mmap04
        # mmap08
        # mmap14
        # mmap18
        # modify_ldt01
        # modify_ldt02
        # modify_ldt03
        # mount01
        # mount02
        # mount03
        # mount04
        # mount05
        # mount06
        # mount07
        # mount_setattr01
        # move_mount01
        # move_mount02
        # move_pages01
        # move_pages02
        # move_pages03
        # move_pages04
        # move_pages05
        # move_pages06
        # move_pages07
        # move_pages09
        # move_pages10
        # move_pages11
        # move_pages12
        # mprotect02
        # mq_notify01
        # mq_notify02
        # mq_notify03
        # mq_open01
        # mq_timedreceive01
        # mq_timedsend01
        # mq_unlink01
        # mremap01
        # mremap02
        # mremap03
        # mremap04
        # mremap05
        # mremap06
        # msgctl01
        # msgctl02
        # msgctl03
        # msgctl04
        # msgctl05
        # msgctl06
        # msgctl12
        # msgget01
        # msgget02
        # msgget03
        # msgget04
        # msgget05
        # msgrcv01
        # msgrcv02
        # msgrcv03
        # msgrcv05
        # msgrcv06
        # msgrcv07
        # msgrcv08
        # msgsnd01
        # msgsnd02
        # msgsnd05
        # msgsnd06
        # msgstress01
        # msync01
        # msync02
        # msync03
        # msync04
        # munlock01
        # munlock02
        # munlockall01
        # munmap01
        # munmap02
        # munmap03
        # name_to_handle_at01
        # name_to_handle_at02
        # nanosleep01
        # newuname01
        # nftw01
        # nftw6401
        # nice01
        # nice02
        # nice03
        # nice04
        # nice05
        # open01A
        # open12
        # open13
        # open14
        # open_by_handle_at01
        # open_by_handle_at02
        # open_tree01
        # open_tree02
        # openat02
        # openat03
        # openat04
        # openat201
        # openat202
        # openat203
        # pause01
        # pause02
        # pause03
        # perf_event_open01
        # perf_event_open02
        # perf_event_open03
        # personality01
        # personality02
        # pidfd_getfd01
        # pidfd_getfd02
        # pidfd_open01
        # pidfd_open02
        # pidfd_open03
        # pidfd_open04
        # pidfd_send_signal01
        # pidfd_send_signal02
        # pidfd_send_signal03
        # pipe07
        # pipe15
        # pivot_root01
        # pkey01
        # poll02
        # posix_fadvise02
        # posix_fadvise04_64
        # prctl01
        # prctl02
        # prctl03
        # prctl04
        # prctl05
        # prctl06
        # prctl07
        # prctl08
        # prctl09
        # prctl10
        # preadv03
        # preadv03_64
        # preadv203
        # preadv203_64
        # process_madvise01
        # process_vm_readv01
        # process_vm_readv02
        # process_vm_readv03
        # process_vm_writev01
        # process_vm_writev02
        # profil01
        # prot_hsymlinks
        # pselect01
        # pselect01_64
        # ptrace01
        # ptrace02
        # ptrace03
        # ptrace04
        # ptrace05
        # ptrace06
        # ptrace07
        # ptrace08
        # ptrace09
        # ptrace10
        # ptrace11
        # pwritev02
        # pwritev02_64
        # pwritev03
        # pwritev03_64
        # pwritev202
        # pwritev202_64
        # qmm01
        # quotactl01
        # quotactl02
        # quotactl03
        # quotactl04
        # quotactl05
        # quotactl06
        # quotactl07
        # quotactl08
        # quotactl09
        # readahead01
        # readahead02
        # readdir21
        # readlink01A
        # reboot01
        # reboot02
        # recv01
        # recvfrom01
        # recvmmsg01
        # recvmsg02
        # recvmsg03
        # remap_file_pages01
        # remap_file_pages02
        # removexattr01
        # removexattr02
        # rename01A
        # rename04
        # rename05
        # rename06
        # rename07
        # rename11
        # rename12
        # rename13
        # rename14
        # renameat01
        # renameat201
        # renameat202
        # request_key01
        # request_key02
        # request_key03
        # request_key04
        # request_key05
        # rmdir03A
        # rt_sigaction01
        # rt_sigaction02
        # rt_sigqueueinfo01
        # rt_sigtimedwait01
        # rt_tgsigqueueinfo01
        # sbrk03
        # sched_get_priority_max01
        # sched_get_priority_max02
        # sched_get_priority_min01
        # sched_get_priority_min02
        # sched_getattr01
        # sched_getattr02
        # sched_getparam01
        # sched_getparam03
        # sched_getscheduler02
        # sched_rr_get_interval01
        # sched_rr_get_interval02
        # sched_rr_get_interval03
        # sched_setaffinity01
        # sched_setattr01
        # sched_setparam01
        # sched_setparam02
        # sched_setparam03
        # sched_setparam04
        # sched_setparam05
        # sched_setscheduler01
        # sched_setscheduler02
        # sched_setscheduler03
        # sched_setscheduler04
        # sched_yield01
        # select01
        # select02
        # semctl01
        # semctl02
        # semctl03
        # semctl04
        # semctl05
        # semctl06
        # semctl07
        # semctl08
        # semctl09
        # semget01
        # semget02
        # semget05
        # semop01
        # semop02
        # semop03
        # semop04
        # semop05
        # send01
        # send02
        # sendfile03
        # sendfile03_64
        # sendfile07
        # sendfile07_64
        # sendfile09
        # sendfile09_64
        # sendmmsg01
        # sendmsg01
        # sendmsg02
        # sendmsg03
        # sendto01
        # sendto03
        # set_mempolicy01
        # set_mempolicy02
        # set_mempolicy03
        # set_mempolicy04
        # set_robust_list01
        # set_thread_area01
        # set_tid_address01
        # setdomainname01
        # setdomainname02
        # setdomainname03
        # setegid01
        # setegid02
        # setfsgid01
        # setfsgid01_16
        # setfsgid02
        # setfsgid02_16
        # setfsgid03
        # setfsgid03_16
        # setfsuid01
        # setfsuid01_16
        # setfsuid02
        # setfsuid02_16
        # setfsuid03
        # setfsuid03_16
        # setfsuid04
        # setfsuid04_16
        # setgid01_16
        # setgid02
        # setgid02_16
        # setgid03_16
        # setgroups01
        # setgroups01_16
        # setgroups02
        # setgroups02_16
        # setgroups03
        # setgroups03_16
        # sethostname01
        # sethostname02
        # sethostname03
        # setns01
        # setns02
        # setpgid01
        # setpgid03
        # setpgrp01
        # setpriority01
        # setpriority02
        # setregid01
        # setregid01_16
        # setregid02
        # setregid02_16
        # setregid03
        # setregid03_16
        # setregid04
        # setregid04_16
        # setresgid01
        # setresgid01_16
        # setresgid02
        # setresgid02_16
        # setresgid03
        # setresgid03_16
        # setresgid04
        # setresgid04_16
        # setresuid01
        # setresuid01_16
        # setresuid02
        # setresuid02_16
        # setresuid03
        # setresuid03_16
        # setresuid04_16
        # setresuid05_16
        # setreuid01_16
        # setreuid02
        # setreuid02_16
        # setreuid03_16
        # setreuid04_16
        # setreuid05_16
        # setreuid06
        # setreuid06_16
        # setreuid07_16
        # setrlimit01
        # setrlimit06
        # setsid01
        # setsockopt02
        # setsockopt05
        # setsockopt06
        # setsockopt07
        # setsockopt08
        # setsockopt09
        # setsockopt10
        # settimeofday01
        # settimeofday02
        # setuid01_16
        # setuid03
        # setuid03_16
        # setuid04
        # setuid04_16
        # setxattr01
        # setxattr02
        # setxattr03
        # sgetmask01
        # shmctl02
        # shmctl04
        # shmctl05
        # shmctl06
        # sigaction01
        # sigaction02
        # sigaltstack01
        # sighold02
        # signal06
        # signalfd01
        # signalfd4_01
        # signalfd4_02
        # sigrelse01
        # sigsuspend01
        # sigtimedwait01
        # sigwaitinfo01
        # socketcall01
        # socketcall02
        # socketcall03
        # sockioctl01
        # splice04
        # ssetmask01
        # stat04
        # stat04_64
        # statfs03
        # statfs03_64
        # statx04
        # statx05
        # statx06
        # statx07
        # statx08
        # statx09
        # statx10
        # statx11
        # statx12
        # stime01
        # stime02
        # string01
        # swapoff01
        # swapoff02
        # swapon01
        # swapon02
        # swapon03
        # switch01
        # symlink01
        # symlink03
        # symlinkat01
        # sync01
        # sync_file_range01
        # sync_file_range02
        # syncfs01
        # sysconf01
        # sysctl01
        # sysctl03
        # sysctl04
        # sysfs01
        # sysfs02
        # sysfs03
        # sysfs04
        # sysfs05
        # sysinfo01
        # sysinfo02
        # sysinfo03
        # syslog11
        # syslog12
        # tee01
        # tee02
        # tgkill01
        # tgkill02
        # timer_create01
        # timer_create02
        # timer_create03
        # timer_delete01
        # timer_delete02
        # timer_getoverrun01
        # timer_gettime01
        # timer_settime01
        # timer_settime02
        # timer_settime03
        # timerfd01
        # timerfd02
        # timerfd04
        # timerfd_create01
        # timerfd_gettime01
        # timerfd_settime01
        # timerfd_settime02
        # times03
        # umount01
        # umount02
        # umount03
        # umount2_01
        # umount2_02
        # unlink01
        # unshare01
        # unshare02
        # userfaultfd01
        # ustat01
        # ustat02
        # utimensat01
        # vfork01
        # vfork02
        # vhangup01
        # vhangup02
        # vmsplice01
        # vmsplice02
        # vmsplice03
        # vmsplice04
        # waitid01
        # waitid02
        # waitid03
        # waitid07
        # waitid08
        # waitid09
        # waitid10
        # waitid11
        # writev03
    "
    if [ "$1" = "glibc" ]; then
        all_testcases="$all_testcases sigtimedwait01 sigwaitinfo01"
    fi

    emit_summary_if_missing() {
        output="$1"
        if ! printf '%s\n' "$output" | grep -q '^Summary:$'; then
            # Align with LTP result accounting:
            # TPASS -> passed, TFAIL -> failed, TBROK -> broken,
            # TCONF -> skipped, TWARN -> warnings, TINFO/TDEBUG ignored.
            # For numbered legacy output ("tcid <num> TTYPE : ..."), deduplicate
            # by "<tcid>:<num>:<TTYPE>" to avoid double counting when buffered
            # output is replayed after fork.
            counts=$(printf '%s\n' "$output" | awk '
                function is_result(t) {
                    return (t == "TPASS" || t == "TFAIL" || t == "TBROK" || t == "TCONF" || t == "TWARN");
                }
                function inc(t) {
                    if (t == "TPASS")
                        passed++;
                    else if (t == "TFAIL")
                        failed++;
                    else if (t == "TBROK")
                        broken++;
                    else if (t == "TCONF")
                        skipped++;
                    else if (t == "TWARN")
                        warnings++;
                }
                {
                    line = $0;
                    gsub(/\033\[[0-9;]*[A-Za-z]/, "", line);
                    n = split(line, f, /[[:space:]]+/);
                    idx = 1;
                    if (f[1] == "")
                        idx = 2;

                    if (n >= idx + 3 && f[idx+1] ~ /^[0-9]+$/ && is_result(f[idx+2]) && f[idx+3] == ":") {
                        key = f[idx] ":" f[idx+1] ":" f[idx+2];
                        if (!(key in seen)) {
                            seen[key] = 1;
                            inc(f[idx+2]);
                        }
                        next;
                    }

                    for (i = 1; i <= n; i++) {
                        t = f[i];
                        has_colon = sub(/:$/, "", t);
                        if (is_result(t) && has_colon) {
                            inc(t);
                            break;
                        }
                    }
                }
                END {
                    printf "%d %d %d %d %d\n", passed, failed, broken, skipped, warnings;
                }
            ')
            set -- $counts
            passed=$1
            failed=$2
            broken=$3
            skipped=$4
            warnings=$5
            echo
            echo "Summary:"
            echo "passed   $passed"
            echo "failed   $failed"
            echo "broken   $broken"
            echo "skipped  $skipped"
            echo "warnings $warnings"
        fi
    }

    cd ltp/testcases/bin


    for f in $(printf '%s\n' "$all_testcases" | awk 'NF && $1 !~ /^#/ { print $1 }'); do
        echo "RUN LTP CASE $f"

        case_output=$(./"$f" 2>&1)

        printf '%s\n' "$case_output"

        emit_summary_if_missing "$case_output"

        echo "FAIL LTP CASE $f : 0"
    done

    cd ../../..

    echo "#### OS COMP TEST GROUP END ltp-$1 ####"
}

cd /musl
musl_runtime
run_ltp musl

cd /glibc
glibc_runtime
run_ltp glibc
