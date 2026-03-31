#!/bin/sh

export HOME=/root

echo "Welcome to Starry OS"
echo

BB=""
if [ -x /musl/busybox ]; then
  BB=/musl/busybox
elif [ -x /glibc/busybox ]; then
  BB=/glibc/busybox
fi

if [ -z "$BB" ]; then
  echo "[TEST] busybox not found under /musl or /glibc"
  echo "[TEST] fallback to interactive shell"
  cd /root || cd /
  sh --login
  exit 0
fi

echo "[TEST] using busybox: $BB"
export PATH="/musl:/glibc:/bin:/usr/bin"

run_test() {
  test_sh="$1"
  test_dir=$("$BB" dirname "$test_sh")
  test_name=$("$BB" basename "$test_sh")
  echo "[TEST] Running $test_sh"
  (
    cd "$test_dir" || exit 127
    "$BB" env HOME=/root PATH=/musl:/glibc:/bin:/usr/bin "$BB" sh -s -- "$BB" "$test_name" <<'EOF'
bb="$1"
script="$2"
for cmd in basename dirname rm mv grep cat mkdir cp ln chmod sed awk mount umount sync ls pwd cut tr sort uniq head tail wc xargs printf sleep kill ps env touch dd readlink find; do
  eval "$cmd() { \"\$bb\" $cmd \"\$@\"; }"
done
. "./$script"
EOF
  )
  ret=$?
  echo "[TEST] Exit code($ret): $test_sh"
  echo
  return "$ret"
}

append_root() {
  d="$1"
  [ -n "$d" ] || return 0
  [ -d "$d" ] || return 0
  case " $SEARCH_ROOTS " in
    *" $d "*) ;;
    *) SEARCH_ROOTS="$SEARCH_ROOTS $d" ;;
  esac
}

SEARCH_ROOTS=""
append_root "/musl"
append_root "/glibc"

echo "[TEST] scan roots:$SEARCH_ROOTS"

TEST_SUITES="basic busybox cyclictest iozone iperf libcbench libctest lmbench ltp lua netperf unixbench"
TEST_LIST=""

for d in $SEARCH_ROOTS; do
  [ -d "$d" ] || continue
  echo "[TEST] scanning $d"
  for suite in $TEST_SUITES; do
    TEST_LIST="${TEST_LIST}$d/${suite}_testcode.sh
"
  done
done

TEST_COUNT=0
while IFS= read -r t; do
  [ -n "$t" ] || continue
  TEST_COUNT=$((TEST_COUNT + 1))
done <<EOF
$TEST_LIST
EOF
echo "[TEST] discovered $TEST_COUNT candidate scripts"

if [ "$TEST_COUNT" -eq 0 ]; then
  echo "[TEST] no test shell scripts found in mounted filesystems"
else
  total=0
  passed=0
  failed=0
  while IFS= read -r t; do
    [ -n "$t" ] || continue
    total=$((total + 1))
    if run_test "$t"; then
      passed=$((passed + 1))
    else
      failed=$((failed + 1))
    fi
  done <<EOF
$TEST_LIST
EOF
  echo "[TEST] summary: total=$total passed=$passed failed=$failed"
fi

echo "[TEST] all discovered scripts completed"
"$BB" poweroff -f >/dev/null 2>&1 || "$BB" reboot -f >/dev/null 2>&1 || exit 0
