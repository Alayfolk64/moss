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
  cd / || true
  sh
  exit 0
fi

echo "[TEST] using busybox: $BB"
export PATH=/musl:/glibc:/bin:/usr/bin
TEST_SUITES="basic busybox cyclictest iozone iperf libcbench libctest lmbench ltp lua netperf unixbench"

setup_runtime_tools() {
  "$BB" mkdir -p /bin /usr/bin >/dev/null 2>&1 || true
  "$BB" --install /bin >/dev/null 2>&1 || "$BB" --install -s /bin >/dev/null 2>&1 || true
  "$BB" --install /usr/bin >/dev/null 2>&1 || "$BB" --install -s /usr/bin >/dev/null 2>&1 || true
  [ -x /bin/sh ] || "$BB" cp -f "$BB" /bin/sh >/dev/null 2>&1 || true
  [ -x /bin/bash ] || "$BB" cp -f "$BB" /bin/bash >/dev/null 2>&1 || true
  [ -x /usr/bin/env ] || "$BB" cp -f "$BB" /usr/bin/env >/dev/null 2>&1 || true
}

run_test() {
  test_sh="$1"
  test_dir=${test_sh%/*}
  test_name=${test_sh##*/}
  old_pwd=$PWD
  [ "$test_dir" = "$test_sh" ] && test_dir=.
  echo "[TEST] Running $test_sh"
  cd "$test_dir" || {
    ret=127
    echo "[TEST] Exit code($ret): $test_sh"
    echo
    return "$ret"
  }
  patched_script="/tmp/.starry-${test_name}.$$"
  "$BB" sed \
    -e 's#\./run-all\.sh#sh ./run-all.sh#g' \
    -e 's#\./run-static\.sh#sh ./run-static.sh#g' \
    -e 's#\./run-dynamic\.sh#sh ./run-dynamic.sh#g' \
    "./$test_name" > "$patched_script" || {
      ret=1
      cd "$old_pwd" || true
      echo "[TEST] Exit code($ret): $test_sh"
      echo
      return "$ret"
    }
  HOME=/root
  PATH=/musl:/glibc:/bin:/usr/bin
  export HOME PATH
  for cmd in sh bash basename dirname rm mv grep cat mkdir cp ln chmod sed awk mount umount sync ls pwd cut tr sort uniq head tail wc xargs printf sleep kill ps env touch dd readlink find which stat free df cal clear date od hexdump md5sum more rmdir false true expr uname uptime hwclock; do
    eval "$cmd() { \"\$BB\" $cmd \"\$@\"; }"
  done
  . "$patched_script"
  ret=$?
  "$BB" rm -f "$patched_script" >/dev/null 2>&1 || true
  cd "$old_pwd" || true
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
append_root "/"
append_root "/musl"
append_root "/glibc"

setup_runtime_tools
echo "[TEST] scan roots:$SEARCH_ROOTS"

TEST_LIST=""
for d in $SEARCH_ROOTS; do
  [ -d "$d" ] || continue
  echo "[TEST] scanning $d"
  for suite in $TEST_SUITES; do
    p="$d/${suite}_testcode.sh"
    [ -f "$p" ] && TEST_LIST="${TEST_LIST}$p
"
  done
done

if [ -z "$TEST_LIST" ]; then
  echo "[TEST] no test shell scripts found in mounted filesystems"
else
  TEST_COUNT=0
  while IFS= read -r t; do
    [ -n "$t" ] || continue
    TEST_COUNT=$((TEST_COUNT + 1))
  done <<EOF_COUNT
$TEST_LIST
EOF_COUNT
  echo "[TEST] discovered $TEST_COUNT candidate scripts"

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
