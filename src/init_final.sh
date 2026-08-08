#!/bin/bash

# Final-round test switches.
cagent=1
buildstorm=0

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

run_cagent() {
    cd /glibc || return 1
    bash /glibc/cagent_testcode.sh
}

run_buildstorm() {
    cd /glibc || return 1
    bash /glibc/buildstorm_testcode.sh
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
