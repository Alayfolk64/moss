#!/bin/sh
set -eu

cd "$(dirname "$0")"

fail()
{
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

printf '%s\n' '[1/4] make --help'
make --help >/dev/null || fail 'make --help'

printf '%s\n' '[2/4] single-threaded build'
make clean
make
result=$(./calc)
[ "$result" = 'result = 42' ] || fail "unexpected output: $result"

printf '%s\n' '[3/4] four-job parallel build'
make clean
make -j4
result=$(./calc)
[ "$result" = 'result = 42' ] || fail "unexpected output: $result"

printf '%s\n' '[4/4] incremental build after touching add.c'
# Sleeping first keeps this check valid on filesystems with coarse mtimes.
sleep 1
touch add.c
log=$(make -j4)
printf '%s\n' "$log"

case "$log" in
    *'[CC] add.c'*) ;;
    *) fail 'add.c was not recompiled' ;;
esac
case "$log" in
    *'[LD] calc'*) ;;
    *) fail 'calc was not relinked' ;;
esac
case "$log" in
    *'[CC] main.c'*|*'[CC] mul.c'*)
        fail 'an unrelated source file was recompiled'
        ;;
esac

result=$(./calc)
[ "$result" = 'result = 42' ] || fail "unexpected output: $result"

printf '%s\n' 'PASS: all GNU Make tests passed'
