#!/bin/bash

set -u

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
mkdir -p /tmp /var/tmp
chmod 1777 /tmp /var/tmp

read -r start_uptime _ </proc/uptime
start_ms=$(($(date +%s%N) / 1000000))

echo "LINUX_BUILDSTORM_START_UPTIME=$start_uptime"
echo "LINUX_KERNEL=$(uname -srvm)"
echo "LINUX_CPUS=$(getconf _NPROCESSORS_ONLN)"

cd /glibc || exit 1
bash ./buildstorm_testcode.sh
test_status=$?

end_ms=$(($(date +%s%N) / 1000000))
read -r end_uptime _ </proc/uptime
elapsed_ms=$((end_ms - start_ms))

echo "LINUX_BUILDSTORM_END_UPTIME=$end_uptime"
echo "LINUX_BUILDSTORM_ELAPSED_MS=$elapsed_ms"
echo "LINUX_BUILDSTORM_STATUS=$test_status"

{
    echo "kernel=$(uname -r)"
    echo "cpus=$(getconf _NPROCESSORS_ONLN)"
    echo "start_uptime=$start_uptime"
    echo "end_uptime=$end_uptime"
    echo "elapsed_ms=$elapsed_ms"
    echo "status=$test_status"
} >/mnt/hostlog/summary.txt

exit "$test_status"
