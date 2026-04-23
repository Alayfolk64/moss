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
        ptrace01
        ptrace02
        ptrace03
        ptrace04
        ptrace05
        ptrace06
        ptrace07
        ptrace08
        ptrace09
        ptrace10
        ptrace11
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


