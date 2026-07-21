#!/bin/bash

set -u

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
mkdir -p /tmp /var/tmp
chmod 1777 /tmp /var/tmp
/bin/busybox ip link set lo up

prepare_cagent_script() {
    local cagent_script=/tmp/cagent_testcode.sh

    cp /glibc/cagent_testcode.sh "$cagent_script"
    sed -i 's|^\./busybox echo |echo |' "$cagent_script"

    # The server is intentionally long-running. Wait only for the ten tests.
    sed -i \
        -e '/^[[:space:]]*[0-9][0-9]* &[[:space:]]*$/a TEST_PIDS="$TEST_PIDS $!"' \
        -e 's/^[[:space:]]*wait[[:space:]]*$/for pid in $TEST_PIDS; do wait "$pid"; done/' \
        -e '/kill \$SERVER_PID/a wait "$SERVER_PID" 2>\/dev\/null' \
        "$cagent_script"
}

read -r start_uptime _ </proc/uptime
start_ms=$(($(date +%s%N) / 1000000))

echo "LINUX_CAGENT_START_UPTIME=$start_uptime"
echo "LINUX_KERNEL=$(uname -srvm)"
echo "LINUX_CPUS=$(getconf _NPROCESSORS_ONLN)"

prepare_cagent_script
cd /glibc || exit 1
bash /tmp/cagent_testcode.sh
test_status=$?

end_ms=$(($(date +%s%N) / 1000000))
read -r end_uptime _ </proc/uptime
elapsed_ms=$((end_ms - start_ms))

echo "LINUX_CAGENT_END_UPTIME=$end_uptime"
echo "LINUX_CAGENT_ELAPSED_MS=$elapsed_ms"
echo "LINUX_CAGENT_STATUS=$test_status"

{
    echo "kernel=$(uname -r)"
    echo "cpus=$(getconf _NPROCESSORS_ONLN)"
    echo "start_uptime=$start_uptime"
    echo "end_uptime=$end_uptime"
    echo "elapsed_ms=$elapsed_ms"
    echo "status=$test_status"
} >/mnt/hostlog/summary.txt

exit "$test_status"
