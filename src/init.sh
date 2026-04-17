if ! /musl/busybox test -d /bin; then
    echo @@@@@@@@@@ setup @@@@@@@@@@

    /musl/busybox mkdir -v /bin
    /musl/busybox --install -s /bin
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
    echo "nameserver 8.8.8.8" >/etc/resolv.conf

    mkdir -vp /lib/modules/10.0.0/build
    echo "CONFIG_EVENTFD=y" >>/lib/modules/10.0.0/build/.config
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
    tkill01
    tkill02
    truncate02
    truncate02_64
    truncate03
    truncate03_64
    ulimit01
    umask01
    uname01
    uname02
    uname04
    unlink05
    unlink07
    unlink08
    unlink09
    unlinkat01
    utime01
    utime02
    utime03
    utime04
    utime05
    utime06
    utime07
    utimes01
    utsname01
    utsname04
    wait01
    wait02
    wait401
    wait402
    wait403
    waitid04
    waitid05
    waitid06
    waitpid01
    waitpid03
    waitpid04
    waitpid06
    waitpid07
    waitpid08
    waitpid09
    waitpid10
    waitpid11
    waitpid12
    waitpid13
    write01
    write02
    write03
    write04
    write05
    write06
    writev01
    writev02
    writev05
    writev06
    writev07
    "
    if [ "$1" = "glibc" ]; then
        all_testcases="$all_testcases sigtimedwait01 sigwaitinfo01"
    fi

    cd ltp/testcases/bin
    for f in $all_testcases; do
        echo "RUN LTP CASE $f"
        ./$f
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

cd /musl
musl_runtime
timeout 60 ./basic_testcode.sh
timeout 60 ./lua_testcode.sh
timeout 60 ./busybox_testcode.sh

cd /glibc
glibc_runtime
timeout 60 ./basic_testcode.sh
timeout 60 ./lua_testcode.sh
timeout 60 ./busybox_testcode.sh

cd /musl
musl_runtime
timeout 60 ./libctest_testcode.sh
timeout 60 ./libcbench_testcode.sh

cd /glibc
glibc_runtime
timeout 60 ./libcbench_testcode.sh

if [[ -e /glibc/lib/ld-linux-riscv64-lp64d.so.1 ]]; then
  cd /musl
  musl_runtime
  timeout 60 ./iperf_testcode.sh
fi

cd /glibc
glibc_runtime
timeout 60 ./iperf_testcode.sh

cd /musl
musl_runtime

timeout 60 ./netperf_testcode.sh
cd /glibc
glibc_runtime
timeout 60 ./netperf_testcode.sh

cd /tmp
ln -v -s /musl/busybox busybox
ln -v -s /musl/iozone iozone
musl_runtime
timeout 300 /musl/iozone_testcode.sh
ln -v -s -f /glibc/iozone iozone
glibc_runtime
timeout 300 /glibc/iozone_testcode.sh

export ENOUGH=1000
cd /musl
musl_runtime
timeout 600 ./lmbench_testcode.sh
if [[ -e /glibc/lib/ld-linux-riscv64-lp64d.so.1 ]];then
  cd /glibc
  glibc_runtime
  timeout 600 ./lmbench_testcode.sh
fi
