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

ARCH=RISCV
if [[ -e /glibc/lib/ld-linux-loongarch-lp64d.so.1 ]]; then
    ARCH=LOONGARCH
fi

export PATH=/bin:$PATH
export HOME=/root
export LTP_TIMEOUT_MUL=0.5
export LTP_DEV_FS_TYPE=tmpfs
export LTP_SINGLE_FS_TYPE=tmpfs

musl_runtime() {
    echo "prepare musl runtime"
    rm -rf /lib/libc.so
    rm -rf /lib/libm.so
    rm -rf /lib/libc.so.6
    rm -rf /lib/libm.so.6
    ln -v -s /musl/lib/libc.so /lib/libc.so
}

glibc_runtime() {
    echo "prepare glibc runtime"
    rm -rf /lib/libc.so
    ln -v -s /glibc/lib/libc.so.6 /lib/libc.so.6
    ln -v -s /glibc/lib/libm.so.6 /lib/libm.so.6
    ln -v -s /lib/libc.so.6 /lib/libc.so
    ln -v -s /lib/libm.so.6 /lib/libm.so
}

timerfd_debug_cases="
    timer_settime01
    timer_settime02
    timer_settime03
    timerfd_settime02
"

run_timerfd_debug_cases() {
    runtime="$1"
    echo "#### TIMERFD DEBUG START $runtime ####"
    cd "ltp/testcases/bin" || exit 1

    for f in $(printf '%s\n' "$timerfd_debug_cases" | awk 'NF && $1 !~ /^#/ { print $1 }'); do
        echo
        echo "===== TIMERFD DEBUG CASE START runtime=$runtime case=$f ====="
        date '+%Y-%m-%d-%H-%M-%S'

        tmp_output="/tmp/${ARCH}_${runtime}_${f}_output.txt"
        tmp_status="/tmp/${ARCH}_${runtime}_${f}_status.txt"
        rm -f "$tmp_output" "$tmp_status"

        {
            ./"$f"
            echo $? >"$tmp_status"
        } 2>&1 | tee "$tmp_output"

        status=255
        if [ -f "$tmp_status" ]; then
            status=$(cat "$tmp_status")
        fi

        echo "===== TIMERFD DEBUG CASE END runtime=$runtime case=$f status=$status ====="
        date '+%Y-%m-%d-%H-%M-%S'
    done

    cd ../../..
    echo "#### TIMERFD DEBUG END $runtime ####"
}

echo @@@@@@@@@@ timerfd-debug @@@@@@@@@@
ls -lh /musl /glibc 2>/dev/null

cd /musl || exit 1
musl_runtime
run_timerfd_debug_cases musl

cd /glibc || exit 1
glibc_runtime
run_timerfd_debug_cases glibc
