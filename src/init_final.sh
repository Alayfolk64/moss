#!/bin/bash

# Final-round test switches.
cagent=1
buildstorm=1

# Architecture switches.
RUN_RV=1
RUN_LA=1

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
mkdir -p /tmp /var/tmp
chmod 1777 /tmp /var/tmp

select_architecture() {
    case "$(uname -m)" in
        riscv64) RUN_ARCH=$RUN_RV ;;
        loongarch64) RUN_ARCH=$RUN_LA ;;
    esac
}


prepare_cagent_script() {
    cagent_script=/tmp/cagent_testcode.sh
    cp /glibc/cagent_testcode.sh "$cagent_script"

    # The image already provides Bash and GNU Coreutils.
    sed -i 's|^\./busybox echo |echo |' "$cagent_script"

    # Wait for the ten tests, not for the long-running LLM server.
    # These variables must be expanded later, when the copied script runs.
    # shellcheck disable=SC2016
    sed -i \
        -e '/^[[:space:]]*[0-9][0-9]* &[[:space:]]*$/a TEST_PIDS="$TEST_PIDS $!"' \
        -e 's/^[[:space:]]*wait[[:space:]]*$/for pid in $TEST_PIDS; do wait "$pid"; done/' \
        -e '/kill \$SERVER_PID/a wait "$SERVER_PID" 2>\/dev\/null' \
        "$cagent_script"
}

run_cagent() {
    prepare_cagent_script
    cd /glibc || return 1
    bash "$cagent_script"
}

run_buildstorm() {
    buildstorm_script=/tmp/buildstorm_testcode.sh
    cp /glibc/buildstorm_testcode.sh "$buildstorm_script"
    # The official clean image omits the globally configured Cargo target
    # directory.  Cargo does not create its missing parent for the minibuild.
    mkdir -p /work/tgoskits/target

    if [ -w /proc/moss_profile ]; then
        sed -i \
            -e '/OS COMP TEST GROUP START buildstorm-glibc/a echo mode=full > /proc/moss_profile; echo reset > /proc/moss_profile; echo phase=setup > /proc/moss_profile' \
            -e '/----- pre-build tg-xtask/a echo phase=prebuild > /proc/moss_profile' \
            -e '/----- build arceos-helloworld/a echo phase=timed > /proc/moss_profile' \
            "$buildstorm_script"
    fi

    cd /glibc || return 1
    bash "$buildstorm_script"
    status=$?

    if [ -r /proc/moss_profile ]; then
        echo stop > /proc/moss_profile
        echo "#### MOSS GUEST PROFILE READY ####"
        # The host profiler freezes the VM at this marker and copies the
        # binary tables through QEMU's GDB stub.  Keep the text snapshot as a
        # fallback, but give the host a quiet interval with profiling stopped.
        sleep 30
        echo "#### MOSS GUEST PROFILE START ####"
        cat /proc/moss_profile
        echo "#### MOSS GUEST PROFILE END ####"
    fi

    return "$status"
}

skip_group() {
    echo "#### OS COMP TEST GROUP START $1 ####"
    echo "#### OS COMP TEST GROUP END $1 ####"
}

select_architecture

if [ "$cagent" = 1 ] && [ "$RUN_ARCH" = 1 ]; then
    run_cagent
else
    skip_group cagent-glibc
fi

if [ "$buildstorm" = 1 ] && [ "$RUN_ARCH" = 1 ]; then
    run_buildstorm
else
    skip_group buildstorm-glibc
fi

exit 0
