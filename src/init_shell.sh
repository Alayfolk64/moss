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
    echo "nameserver 8.8.8.8" >/etc/resolv.conf
fi

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:$PATH
export HOME=/root
export PS1='starry:~# '

cd /root 2>/dev/null || cd /

if [ ! -s /etc/resolv.conf ]; then
    mkdir -p /etc
    printf 'nameserver 223.5.5.5\nnameserver 1.1.1.1\n' >/etc/resolv.conf
fi

if command -v ntpd >/dev/null 2>&1; then
    echo "Synchronizing system time with NTP..."
    ntpd -nq -p ntp.aliyun.com || echo "Warning: NTP time synchronization failed"
fi

echo "Entering interactive shell..."
exec /bin/sh
