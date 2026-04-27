if ! /musl/busybox test -d /bin; then
    echo @@@@@@@@@@ setup-shell @@@@@@@@@@

    /musl/busybox mkdir -v /bin
    /musl/busybox --install -s /bin
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

export PATH=/bin:$PATH
export HOME=/root
export PS1='starry:~# '

cd /root 2>/dev/null || cd /

echo "Entering interactive shell..."
exec /bin/sh
