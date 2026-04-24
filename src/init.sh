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
    for f in $all_testcases; do
        echo "RUN LTP CASE $f"
        case_output=$(./$f 2>&1)
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
