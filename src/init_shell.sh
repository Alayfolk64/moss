if [ ! -x /bin/sh ]; then
    echo @@@@@@@@@@ setup-shell @@@@@@@@@@

    if [ -x /musl/busybox ]; then
        BOOTSTRAP_BUSYBOX=/musl/busybox
    elif [ -x /glibc/busybox ]; then
        BOOTSTRAP_BUSYBOX=/glibc/busybox
    elif [ -x /bin/busybox ]; then
        BOOTSTRAP_BUSYBOX=/bin/busybox
    else
        echo "No /bin/sh or compatible busybox found"
        exit 127
    fi

    "$BOOTSTRAP_BUSYBOX" mkdir -vp /bin
    "$BOOTSTRAP_BUSYBOX" --install -s /bin
    export PATH=/bin

    mkdir -vp /lib /usr /var/tmp /var/log /etc

    if [ -e /glibc/lib/ld-linux-loongarch-lp64d.so.1 ]; then
        ln -vsnf /musl/lib/libc.so /lib/ld-musl-loongarch-lp64d.so.1
        ln -vsnf /glibc/lib/ld-linux-loongarch-lp64d.so.1 /lib/ld-linux-loongarch-lp64d.so.1
    elif [ -e /glibc/lib/ld-linux-riscv64-lp64d.so.1 ]; then
        ln -vsnf /musl/lib/libc.so /lib/ld-musl-riscv64.so.1
        ln -vsnf /musl/lib/libc.so /lib/ld-musl-riscv64-sf.so.1
        ln -vsnf /glibc/lib/ld-linux-riscv64-lp64d.so.1 /lib/ld-linux-riscv64-lp64d.so.1
    fi

    ln -vsnf /lib /lib64
    ln -vsnf /lib /usr/lib64

    {
        echo "root:x:0:0:root:/root:/bin/sh"
        echo "nobody:x:65534:65534:nobody:/:/bin/sh"
    } >/etc/passwd
fi

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:$PATH
export HOME=/root
export PS1='starry:~# '

cd /root 2>/dev/null || cd /

if [ -r /proc/net/resolv.conf ]; then
    rm -f /etc/resolv.conf
    ln -s /proc/net/resolv.conf /etc/resolv.conf
elif [ ! -s /etc/resolv.conf ]; then
    mkdir -p /etc
    printf 'nameserver 223.5.5.5\nnameserver 1.1.1.1\n' >/etc/resolv.conf
fi

if [ "${MOSS_NTP_SYNC:-0}" = 1 ] && command -v ntpd >/dev/null 2>&1; then
    # The RTC supplies boot time. NTP is opt-in because BusyBox ntpd exercises
    # SIGALRM heavily and network availability must not affect the shell.
    # Set MOSS_NTP_SYNC=1 before running this script to request a one-shot sync.
    mkdir -p /var/log 2>/dev/null || true
    echo "Synchronizing system time with NTP in background (log: /var/log/moss-ntp.log)..."
    (
        NTP_OK=0
        NTP_ATTEMPT=1
        while [ "$NTP_ATTEMPT" -le 3 ]; do
            if ntpd -nq -p 203.107.6.88 \
                || ntpd -nq -p 192.168.88.1; then
                NTP_OK=1
                break
            fi
            NTP_ATTEMPT=$((NTP_ATTEMPT + 1))
            sleep 1
        done
        if [ "$NTP_OK" -ne 1 ]; then
            echo "Warning: NTP time synchronization failed"
        fi
    ) >/var/log/moss-ntp.log 2>&1 &
fi

echo "Entering interactive shell..."
exec /bin/sh
