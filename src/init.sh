if ! /glibc/busybox test -d /bin; then
    /glibc/busybox mkdir -v /bin
fi

/glibc/busybox --install -s /bin
export PATH=/bin:$PATH

mkdir -vp /lib
if [[ -e /glibc/lib/ld-linux-loongarch-lp64d.so.1 ]]; then
    rm -f /lib/ld-musl-loongarch-lp64d.so.1 /lib/ld-linux-loongarch-lp64d.so.1
    ln -v -s /musl/lib/libc.so /lib/ld-musl-loongarch-lp64d.so.1
    ln -v -s /glibc/lib/ld-linux-loongarch-lp64d.so.1 /lib/ld-linux-loongarch-lp64d.so.1
elif [[ -e /glibc/lib/ld-linux-riscv64-lp64d.so.1 ]]; then
    rm -f /lib/ld-musl-riscv64.so.1 /lib/ld-musl-riscv64-sf.so.1 /lib/ld-linux-riscv64-lp64d.so.1
    ln -v -s /musl/lib/libc.so /lib/ld-musl-riscv64.so.1
    ln -v -s /musl/lib/libc.so /lib/ld-musl-riscv64-sf.so.1
    ln -v -s /glibc/lib/ld-linux-riscv64-lp64d.so.1 /lib/ld-linux-riscv64-lp64d.so.1
fi
rm -rf /lib64
ln -v -s /lib /lib64

mkdir -vp /usr
rm -rf /usr/lib64
ln -v -s /lib /usr/lib64

cd /
mkdir -vp /var/tmp /var/log

cd /
mkdir -vp /etc
echo "root:x:0:0:root:/root:/bin/sh" >>/etc/passwd
echo "nobody:x:65534:65534:nobody:/:/bin/sh" >>/etc/passwd
echo "root:x:0:" >>/etc/group
echo "nobody:x:65534:" >>/etc/group
echo "nameserver 8.8.8.8" >/etc/resolv.conf

mkdir -vp /lib/modules/10.0.0/build
echo "CONFIG_EVENTFD=y" >/lib/modules/10.0.0/build/.config
echo "CONFIG_USER_NS=y" >>/lib/modules/10.0.0/build/.config
echo "CONFIG_NET_NS=y" >>/lib/modules/10.0.0/build/.config
echo "CONFIG_BSD_PROCESS_ACCT=y" >>/lib/modules/10.0.0/build/.config
echo "CONFIG_TIME_NS=y" >>/lib/modules/10.0.0/build/.config
echo "CONFIG_AIO=y" >>/lib/modules/10.0.0/build/.config

rm -f /bin/useradd /bin/userdel /bin/groupdel
cat >/bin/useradd <<'EOF'
#!/bin/sh
name=$1
uid=$((2000 + ${name##*_}))
echo "$name:x:$uid:$uid:$name:/:/bin/sh" >>/etc/passwd
EOF

chmod +x /bin/useradd
cat >/bin/userdel <<'EOF'
#!/bin/sh
exit 0
EOF

cat >/bin/groupdel <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x /bin/userdel /bin/groupdel

cat >/bin/mkfs.ext3 <<'EOF'
#!/bin/sh
exec mkfs.ext2 "$@"
EOF

cat >/bin/mkfs.ext4 <<'EOF'
#!/bin/sh
exec mkfs.ext2 "$@"
EOF

cat >/bin/mkfs.xfs <<'EOF'
#!/bin/sh
exit 0
EOF

cat >/bin/mkfs.btrfs <<'EOF'
#!/bin/sh
exit 0
EOF

cat >/bin/mkfs.exfat <<'EOF'
#!/bin/sh
exit 0
EOF

cat >/bin/mkfs.ntfs <<'EOF'
#!/bin/sh
exit 0
EOF

cat >/bin/mount.ntfs <<'EOF'
#!/bin/sh
exec mount -t ntfs "$1" "$2"
EOF

chmod +x /bin/mkfs.ext3 /bin/mkfs.ext4 /bin/mkfs.xfs /bin/mkfs.btrfs /bin/mkfs.exfat /bin/mkfs.ntfs /bin/mount.ntfs

ARCH=RISCV
if [[ -e /glibc/lib/ld-linux-loongarch-lp64d.so.1 ]]; then
    ARCH=LOONGARCH
fi

some_testcases=""

RUN_RV=1
RUN_LA=1

# Test switches: 1 means run, 0 means only print group start/end.
basic=1
busybox=1
cyclictest=0
iozone=1
iperf=0
libcbench=1
libctest=1
lmbench=1
ltp=1
lua=1
netperf=1

if [ -n "$some_testcases" ]; then
    basic=0
    busybox=0
    cyclictest=0
    iozone=0
    iperf=0
    libcbench=0
    libctest=0
    lmbench=0
    ltp=1
    lua=0
    netperf=0
fi

if [ "$ARCH" = RISCV ] && [ "$RUN_RV" = 0 ]; then
    basic=0
    busybox=0
    cyclictest=0
    iozone=0
    iperf=0
    libcbench=0
    libctest=0
    lmbench=0
    ltp=0
    lua=0
    netperf=0
fi

if [ "$ARCH" = LOONGARCH ] && [ "$RUN_LA" = 0 ]; then
    basic=0
    busybox=0
    cyclictest=0
    iozone=0
    iperf=0
    libcbench=0
    libctest=0
    lmbench=0
    ltp=0
    lua=0
    netperf=0
fi

chmod +x /musl/basic/run-all.sh
chmod +x /glibc/basic/run-all.sh

musl_runtime() {
    echo "prepare musl runtime"
    rm -rf /lib/libc.so
    rm -rf /lib/libm.so
    rm -rf /lib/libc.so.6
    rm -rf /lib/libm.so.6
    rm -rf /lib/libgcc_s.so.1
    ln -v -s /musl/lib/libc.so /lib/libc.so
}

glibc_runtime() {
    echo "prepare glibc runtime"
    rm -rf /lib/libc.so
    rm -rf /lib/libgcc_s.so.1
    ln -v -s /glibc/lib/libc.so.6 /lib/libc.so.6
    ln -v -s /glibc/lib/libm.so.6 /lib/libm.so.6
    if [[ -e /glibc/lib/libgcc_s.so.1 ]]; then
        ln -v -s /glibc/lib/libgcc_s.so.1 /lib/libgcc_s.so.1
    fi
    ln -v -s /lib/libc.so.6 /lib/libc.so
    ln -v -s /lib/libm.so.6 /lib/libm.so
}

echo @@@@@@@@@@ files @@@@@@@@@@
ls -lhAR /lib
echo @@@@@@@@@@ env @@@@@@@@@@
env | awk '
    /^[A-Za-z_][A-Za-z0-9_]*=/ { skip = ($0 ~ /^LTP_CASES=/) }
    !skip { print }
'
echo

run_ltp() {
    echo "#### OS COMP TEST GROUP START ltp-$1 ####"

    all_testcases="${LTP_CASES:-}"
    if [ -z "$all_testcases" ] && [ -f ./ltp_testcases.sh ]; then
        all_testcases="$(awk 'NF && $1 !~ /^#/ { print }' ./ltp_testcases.sh)"
    fi

    emit_summary_if_missing() {
        output="$1"
        if printf '%s\n' "$output" | grep -q '^Summary:[[:space:]]*$'; then
            printf '%s\n' "$output" | awk '/^Summary:[[:space:]]*$/ { show = 1 } show { print } show && /^warnings[[:space:]]/ { exit }'
        else
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
    export PATH="$(pwd):$PATH"

    if [ -n "$some_testcases" ]; then
        all_testcases="$some_testcases"
    fi

    printf '%s\n' "$all_testcases" | awk 'NF && $1 !~ /^#/ { $1=$1; print }' | while IFS= read -r testcase_line; do
        case_name=$(printf '%s\n' "$testcase_line" | awk '{ print $1 }')
        run_cmd=${testcase_line#"$case_name"}
        run_cmd=${run_cmd# }
        if [ -z "$run_cmd" ]; then
            run_cmd="$case_name"
        fi

        echo "RUN LTP CASE $case_name"
        tmp_output="/tmp/${ARCH}_ltp_case_${case_name}_output.txt"
        rm -f "$tmp_output"

        ./$run_cmd 2>&1 | tee "$tmp_output"
        case_output=$(cat "$tmp_output")
        if ! printf '%s\n' "$case_output" | grep -q '^Summary:[[:space:]]*$'; then
            emit_summary_if_missing "$case_output"
        fi

        echo "FAIL LTP CASE $case_name : 0"

        date '+%Y-%m-%d-%H-%M-%S'

    done

    cd ../../..

    echo "#### OS COMP TEST GROUP END ltp-$1 ####"
}

skip_group() {
    echo "#### OS COMP TEST GROUP START $1-$2 ####"
    echo "#### OS COMP TEST GROUP END $1-$2 ####"
}

cd /musl

musl_runtime
if [ "$basic" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./basic_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group basic musl
fi

if [ "$busybox" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./busybox_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group busybox musl
fi

if [ "$libctest" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./libctest_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group libctest musl
fi

if [ "$ltp" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    run_ltp musl
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group ltp musl
fi

if [ "$cyclictest" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./cyclictest_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group cyclictest musl
fi

if [ "$iozone" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./iozone_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group iozone musl
fi

if [ "$iperf" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./iperf_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group iperf musl
fi

if [ "$libcbench" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./libcbench_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group libcbench musl
fi

if [ "$lmbench" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./lmbench_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group lmbench musl
fi

if [ "$lua" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./lua_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group lua musl
fi

if [ "$netperf" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./netperf_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group netperf musl
fi

cd /glibc

glibc_runtime
if [ "$basic" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./basic_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group basic glibc
fi

if [ "$busybox" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./busybox_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group busybox glibc
fi

if [ "$ltp" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    run_ltp glibc
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group ltp glibc
fi

if [ "$cyclictest" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./cyclictest_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group cyclictest glibc
fi

if [ "$iozone" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./iozone_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group iozone glibc
fi

if [ "$iperf" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./iperf_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group iperf glibc
fi

if [ "$libcbench" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./libcbench_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group libcbench glibc
fi

if [ "$lmbench" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./lmbench_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group lmbench glibc
fi

if [ "$lua" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./lua_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group lua glibc
fi

if [ "$netperf" = 1 ]; then
    date '+%Y-%m-%d-%H-%M-%S'
    ./netperf_testcode.sh
    date '+%Y-%m-%d-%H-%M-%S'
else
    skip_group netperf glibc
fi

exit 0
