#!/bin/bash

set -eu

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

for package in /mnt/hostlog/debs/*.deb; do
    dpkg-deb -x "$package" /
done

mkdir -p /sys/kernel/tracing
mountpoint -q /sys/kernel/tracing || mount -t tracefs tracefs /sys/kernel/tracing
echo 0 >/proc/sys/kernel/kptr_restrict
echo -1 >/proc/sys/kernel/perf_event_paranoid

echo "LINUX_PROFILE_PROBE_KERNEL=$(uname -srvm)"
echo "LINUX_PROFILE_PROBE_CPUS=$(nproc)"
bpftrace --version
bpftrace --info

for probe in \
    'profile:hz:19' \
    'tracepoint:sched:sched_switch' \
    'kprobe:__mutex_lock_slowpath' \
    'kprobe:rwsem_down_read_slowpath' \
    'kprobe:rwsem_down_write_slowpath' \
    'kprobe:filemap_fault' \
    'kprobe:filemap_read' \
    'kprobe:ext4_file_read_iter' \
    'kprobe:submit_bio_noacct' \
    'kprobe:bio_endio'; do
    if bpftrace -l "$probe" | grep -q .; then
        echo "LINUX_PROFILE_PROBE_OK=$probe"
    else
        echo "LINUX_PROFILE_PROBE_MISSING=$probe"
    fi
done

BPFTRACE_MAX_MAP_KEYS=65536 timeout --signal=INT 60 \
    bpftrace -B none /mnt/hostlog/linux_rv_profile.bt

echo "LINUX_PROFILE_PROBE_STATUS=OK"
