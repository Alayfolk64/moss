#!/bin/busybox sh

set -eu

export PATH=/bin

mkdir -p /dev /proc /sys /newroot
mount -t devtmpfs devtmpfs /dev
mount -t proc proc /proc
mount -t sysfs sysfs /sys

insmod /modules/netfs.ko
insmod /modules/9pnet.ko
insmod /modules/9pnet_virtio.ko
insmod /modules/9p.ko

mount -t ext4 -o rw /dev/vda /newroot
mkdir -p /newroot/dev /newroot/proc /newroot/sys /newroot/mnt/hostlog
mount -t devtmpfs devtmpfs /newroot/dev
mount -t proc proc /newroot/proc
mount -t sysfs sysfs /newroot/sys
mount -t 9p -o trans=virtio,version=9p2000.L hostlog /newroot/mnt/hostlog

read -r boot_uptime _ </proc/uptime
echo "LINUX_ROOT_READY_UPTIME=$boot_uptime" | tee /newroot/mnt/hostlog/boot-time.txt

chroot /newroot /bin/bash /mnt/hostlog/benchmark.sh 2>&1 \
    | tee /newroot/mnt/hostlog/benchmark.log

sync
poweroff -f
