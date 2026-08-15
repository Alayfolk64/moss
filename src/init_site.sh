#!/bin/sh

# The on-site final is judged interactively. Keep this profile limited to
# preparing a predictable userspace and handing the console to the operator.
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export HOME=/root
export PS1='starry:~# '

mkdir -p /root /tmp /var/tmp /etc
chmod 1777 /tmp /var/tmp

if [ ! -s /etc/resolv.conf ]; then
    printf 'nameserver 10.0.2.3\nnameserver 1.1.1.1\n' >/etc/resolv.conf
fi

cd /root 2>/dev/null || cd /

echo "Entering on-site final interactive shell..."
exec /bin/sh
