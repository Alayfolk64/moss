export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export HOME=/root
export PS1='starry:~# '

configure_dns() {
    printf 'nameserver 223.5.5.5\nnameserver 1.1.1.1\n' > /etc/resolv.conf
}

configure_dns
cd /root 2>/dev/null || cd /
exec /bin/sh
