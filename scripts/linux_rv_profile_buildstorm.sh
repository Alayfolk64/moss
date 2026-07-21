#!/bin/bash

set -u

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export HOSTLOG=/mnt/hostlog
export BPFTRACE_MAX_MAP_KEYS=65536

for package in "$HOSTLOG"/debs/*.deb; do
    dpkg-deb -x "$package" /
done

mkdir -p /sys/kernel/tracing /tmp /var/tmp
mountpoint -q /sys/kernel/tracing || mount -t tracefs tracefs /sys/kernel/tracing
echo 0 >/proc/sys/kernel/kptr_restrict
echo -1 >/proc/sys/kernel/perf_event_paranoid
chmod 1777 /tmp /var/tmp

profile_start() {
    profile_phase=$1
    profile_raw=/tmp/linux-profile-$profile_phase.raw
    profile_ready=/tmp/linux-profile-$profile_phase.ready
    : >"$profile_raw"

    mkdir -p /sys/kernel/tracing
    if [ ! -r /sys/kernel/tracing/events/sched/sched_switch/id ]; then
        mount -t tracefs tracefs /sys/kernel/tracing
    fi
    if [ ! -r /sys/kernel/tracing/events/sched/sched_switch/id ]; then
        echo "LINUX_PROFILE_TRACEFS_MISSING phase=$profile_phase"
        return 1
    fi

    bpftrace -B none "$HOSTLOG/linux_rv_profile.bt" >"$profile_raw" 2>&1 &
    profile_pid=$!

    profile_wait=0
    while ! grep -q '^Attaching [0-9][0-9]* probes' "$profile_raw"; do
        if ! kill -0 "$profile_pid" 2>/dev/null; then
            cp "$profile_raw" "$HOSTLOG/$profile_phase.raw"
            echo "LINUX_PROFILE_ATTACH_FAILED phase=$profile_phase"
            return 1
        fi
        sleep 0.25
        profile_wait=$((profile_wait + 1))
        if [ "$profile_wait" -ge 480 ]; then
            kill -INT "$profile_pid" 2>/dev/null || true
            wait "$profile_pid" 2>/dev/null || true
            cp "$profile_raw" "$HOSTLOG/$profile_phase.raw"
            echo "LINUX_PROFILE_ATTACH_TIMEOUT phase=$profile_phase"
            return 1
        fi
    done

    # bpftrace prints "Attaching" before perf_event_open has attached every
    # probe. Give the final attach pass time to fail before starting the phase.
    sleep 2
    if ! kill -0 "$profile_pid" 2>/dev/null; then
        wait "$profile_pid" 2>/dev/null || true
        cp "$profile_raw" "$HOSTLOG/$profile_phase.raw"
        echo "LINUX_PROFILE_ATTACH_FAILED phase=$profile_phase"
        return 1
    fi

    read -r profile_started _ </proc/uptime
    echo "$profile_phase $profile_started" >>"$HOSTLOG/profile-times.txt"
    echo "LINUX_PROFILE_START phase=$profile_phase uptime=$profile_started pid=$profile_pid"
    : >"$profile_ready"
}

profile_stop() {
    expected_phase=$1
    if [ "${profile_phase:-}" != "$expected_phase" ] || [ -z "${profile_pid:-}" ]; then
        echo "LINUX_PROFILE_STOP_MISMATCH expected=$expected_phase actual=${profile_phase:-none}"
        return 1
    fi

    read -r profile_stopped _ </proc/uptime
    echo "$expected_phase $profile_started $profile_stopped" >>"$HOSTLOG/profile-times.txt"
    echo "LINUX_PROFILE_STOP phase=$expected_phase uptime=$profile_stopped"

    kill -INT "$profile_pid" 2>/dev/null || true
    wait "$profile_pid" 2>/dev/null || true
    cp "$profile_raw" "$HOSTLOG/$expected_phase.raw"

    unset profile_phase profile_pid profile_raw profile_ready profile_started
}

export -f profile_start profile_stop
rm -f "$HOSTLOG/profile-times.txt"

buildstorm_script=/tmp/linux-profile-buildstorm.sh
cp /glibc/buildstorm_testcode.sh "$buildstorm_script"
sed -i \
    -e '/^mount -t devtmpfs devtmpfs \/dev 2>\/dev\/null$/a profile_start setup || exit 1' \
    -e '/^echo "----- pre-build tg-xtask (untimed) -----"$/a profile_stop setup; profile_start prebuild || exit 1' \
    -e '/^echo "----- build arceos-helloworld (timed, arch=\$AXARCH) -----"$/a profile_stop prebuild; profile_start timed || exit 1' \
    -e '/^echo "#### OS COMP TEST GROUP END buildstorm-glibc ####"$/i profile_stop timed' \
    "$buildstorm_script"

read -r run_started _ </proc/uptime
echo "LINUX_PROFILE_RUN_START_UPTIME=$run_started"
echo "LINUX_PROFILE_KERNEL=$(uname -srvm)"
echo "LINUX_PROFILE_CPUS=$(nproc)"
echo "LINUX_PROFILE_BPFTRACE=$(bpftrace --version)"

bash "$buildstorm_script"
test_status=$?

read -r run_stopped _ </proc/uptime
echo "LINUX_PROFILE_RUN_END_UPTIME=$run_stopped"
echo "LINUX_PROFILE_STATUS=$test_status"

{
    echo "kernel=$(uname -r)"
    echo "cpus=$(nproc)"
    echo "start_uptime=$run_started"
    echo "end_uptime=$run_stopped"
    echo "status=$test_status"
} >"$HOSTLOG/summary.txt"

exit "$test_status"
