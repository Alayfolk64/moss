BOOT_BB=/musl/busybox

# Test switches: 1 means run, 0 means only print group start/end.
cagent=1
buildstorm=0

# Architecture switches: 1 means run tests on this architecture, 0 means skip.
RUN_RV=1
RUN_LA=1

case "$("$BOOT_BB" uname -m 2>/dev/null)" in
    riscv64) RUN_ARCH=$RUN_RV ;;
    loongarch64) RUN_ARCH=$RUN_LA ;;
    *)
        "$BOOT_BB" echo "FINAL_INIT_ERROR: unsupported architecture"
        exit 1
        ;;
esac

run_cagent() {
    BB=$BOOT_BB
    "$BB" mkdir -p /bin /tmp /var/tmp
    "$BB" chmod 1777 /tmp /var/tmp
    "$BB" --install -s /bin
    export PATH=/bin
    "$BB" rm -f /bin/date
    cat >/bin/date <<'EOF'
#!/bin/sh
if [ "$1" = -d ] && [ "$2" = "100 days ago" ]; then
    now=$(/musl/busybox date +%s)
    then=$((now - 8640000))
    shift 2
    exec /musl/busybox date -d "@$then" "$@"
fi
exec /musl/busybox date "$@"
EOF
    "$BB" chmod +x /bin/date
    cd /glibc || return 1
    SCRIPT=/tmp/cagent_testcode.sh
    PATCHED_SCRIPT=/tmp/cagent_testcode.patched.sh
    "$BB" cp ./cagent_testcode.sh "$SCRIPT"
    "$BB" sed -i 's|date +%s%3N|date +%s000|g' "$SCRIPT"

    # The evaluator starts the server and all test cases in the background.
    # A bare `wait` is unreliable here: it may return while adopted children are
    # still running. Track the ten test PIDs explicitly, then stop and reap the
    # server before the next group starts.
    if "$BB" awk '
    /^[[:space:]]*SERVER_PID=\$![[:space:]]*$/ {
        print
        print "TEST_PIDS=\"\""
        server_seen++
        next
    }
    /^[[:space:]]*wait[[:space:]]*$/ {
        print "for pid in $TEST_PIDS; do"
        print "    wait \"$pid\""
        print "done"
        wait_replaced++
        next
    }
    {
        print
        if ($0 ~ /^[[:space:]]*[0-9]+[[:space:]]*&[[:space:]]*$/) {
            print "TEST_PIDS=\"$TEST_PIDS $!\""
            tracked++
        }
        if ($0 ~ /^[[:space:]]*kill[[:space:]]+\$SERVER_PID/) {
            print "wait \"$SERVER_PID\" 2>/dev/null"
            server_reaped++
        }
    }
    END {
        if (server_seen != 1 || tracked != 10 || wait_replaced != 1 || server_reaped != 1)
            exit 1
    }
    ' "$SCRIPT" > "$PATCHED_SCRIPT"; then
        "$BB" mv "$PATCHED_SCRIPT" "$SCRIPT"
    else
        "$BB" echo "FINAL_INIT_ERROR: failed to patch cagent process waits"
        "$BB" rm -f "$PATCHED_SCRIPT"
        return 1
    fi

    "$BB" sh "$SCRIPT"
}

run_buildstorm() {
    export PATH=/root/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/sbin:/usr/sbin
    export HOME=/root RUSTUP_HOME=/root/.rustup CARGO_HOME=/root/.cargo
    export RUSTUP_TOOLCHAIN=nightly-2026-05-28
    export CARGO_NET_OFFLINE=true

    mount -t proc proc /proc 2>/dev/null
    mount -t sysfs sysfs /sys 2>/dev/null
    mount -t devtmpfs devtmpfs /dev 2>/dev/null

    case "$(uname -m 2>/dev/null)" in
        loongarch64) AXARCH=loongarch64; AXTGT=loongarch64-unknown-linux-musl ;;
        *) AXARCH=riscv64; AXTGT=riscv64gc-unknown-linux-musl ;;
    esac

    echo "#### OS COMP TEST GROUP START buildstorm ####"
    if rustc --version && cargo --version; then
        echo "BUILDSTORM_TOOLCHAIN ok"
    else
        echo "BUILDSTORM_TOOLCHAIN fail"
    fi

    rm -rf /tmp/minibuild
    if cargo new --vcs none /tmp/minibuild >/dev/null 2>&1 \
        && (cd /tmp/minibuild && cargo build >/dev/null 2>&1) \
        && [ "$(/tmp/minibuild/target/debug/minibuild)" = "Hello, world!" ]; then
        echo "BUILDSTORM_MINIBUILD ok"
    else
        echo "BUILDSTORM_MINIBUILD fail"
    fi

    cd /work/tgoskits 2>/dev/null || {
        echo "BUILDSTORM_COMPILE mode=multi ok=false elapsed_s=0 cores=$(nproc) bytes=0 arch=$AXARCH"
        echo "#### OS COMP TEST GROUP END buildstorm ####"
        return 1
    }

    rm -rf "target/$AXTGT"
    echo "----- pre-build tg-xtask (untimed) -----"
    cargo build -p tg-xtask 2>&1 || true

    echo "----- build arceos-helloworld (timed, arch=$AXARCH) -----"
    echo "BUILDSTORM_BEGIN mode=multi"
    T0=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
    { timeout 14400 cargo xtask arceos build -p arceos-helloworld --arch "$AXARCH" 2>&1; \
        echo $? > /work/.build.rc; } | tee /work/buildstorm.build.out
    RC=$(cat /work/.build.rc 2>/dev/null || echo 1)
    rm -f /work/.build.rc
    T1=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
    ELAPSED=$(awk "BEGIN{printf \"%.2f\", (\"$T1\"+0)-(\"$T0\"+0)}" 2>/dev/null)
    [ -n "$ELAPSED" ] || ELAPSED=0

    ART=$(find target -type f \( -name arceos-helloworld -o -name helloworld \) 2>/dev/null | head -1)
    BYTES=0
    [ -z "$ART" ] || BYTES=$(wc -c < "$ART")

    if [ "$RC" -eq 0 ] && [ -n "$ART" ] && [ "$BYTES" -ge 500000 ]; then
        echo "BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=$ELAPSED cores=$(nproc) bytes=$BYTES arch=$AXARCH"
    else
        echo "BUILDSTORM_COMPILE mode=multi ok=false rc=$RC elapsed_s=$ELAPSED cores=$(nproc) bytes=$BYTES arch=$AXARCH"
        echo "----- buildstorm.build.out tail -----"
        tail -25 /work/buildstorm.build.out 2>/dev/null
    fi
    echo "#### OS COMP TEST GROUP END buildstorm ####"
}

skip_group() {
    echo "#### OS COMP TEST GROUP START $1 ####"
    echo "#### OS COMP TEST GROUP END $1 ####"
}

# Run the two final-test groups independently and always keep their output order.
if [ "$cagent" = 1 ] && [ "$RUN_ARCH" = 1 ] \
    && [ -x /glibc/agent_lite ] && [ -x /glibc/simple_llm_server ] \
    && [ -f /glibc/cagent_testcode.sh ]; then
    run_cagent
else
    skip_group cagent-glibc
fi

if [ "$buildstorm" = 1 ] && [ "$RUN_ARCH" = 1 ] \
    && [ -d /work/tgoskits ]; then
    run_buildstorm
else
    skip_group buildstorm
fi

# The score has already been emitted. Avoid hanging forever in sync/ext4 flush:
# -n skips BusyBox's sync and -f invokes the kernel reboot syscall directly.
exec "$BOOT_BB" poweroff -n -f
