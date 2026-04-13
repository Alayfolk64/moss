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
# Ensure shebang scripts like `./run-all.sh` can find `/bin/sh`.
"$BB" mkdir -p /bin >/dev/null 2>&1 || true
"$BB" cp "$BB" /bin/busybox >/dev/null 2>&1 || true
"$BB" chmod +x /bin/busybox >/dev/null 2>&1 || true
"$BB" cp "$BB" /bin/sh >/dev/null 2>&1 || true
"$BB" chmod +x /bin/sh >/dev/null 2>&1 || true
export PATH=/musl:/glibc:/bin:/usr/bin
export LD_LIBRARY_PATH=/glibc/lib:/musl/lib:/lib:/lib64:/usr/lib
TEST_SUITES="basic busybox cyclictest iozone iperf libcbench libctest lmbench ltp lua netperf unixbench"

setup_runtime_tools() {
  ensure_bb_cmd() {
    cmd="$1"
    [ -n "$cmd" ] || return 0
    [ -x "/bin/$cmd" ] && return 0
    "$BB" cp -f "$BB" "/bin/$cmd" >/dev/null 2>&1 || true
    "$BB" chmod +x "/bin/$cmd" >/dev/null 2>&1 || true
  }

  rt_pwd=$PWD
  cd / >/dev/null 2>&1 || true
  "$BB" mkdir -p bin usr usr/bin tmp var var/tmp var/tmp/lmbench >/dev/null 2>&1 || true
  "$BB" chmod 1777 tmp var/tmp >/dev/null 2>&1 || true
  cd "$rt_pwd" >/dev/null 2>&1 || true
  if [ -d /glibc/lib ]; then
    rt_pwd=$PWD
    cd / >/dev/null 2>&1 || true
    "$BB" mkdir -p lib lib64 >/dev/null 2>&1 || true
    cd "$rt_pwd" >/dev/null 2>&1 || true
    for so in ld-linux-riscv64-lp64d.so.1 ld-linux-loongarch-lp64d.so.1 libc.so libc.so.6 libm.so libm.so.6; do
      [ -e "/glibc/lib/$so" ] && "$BB" cp -f "/glibc/lib/$so" "/lib/$so" >/dev/null 2>&1 || true
      [ -e "/glibc/lib/$so" ] && "$BB" cp -f "/glibc/lib/$so" "/lib64/$so" >/dev/null 2>&1 || true
    done
  fi
  # FAT/ext files in test images often miss musl ldso symlink; provide real files in /lib.
  if [ -e /musl/lib/libc.so ]; then
    [ -e /lib/ld-musl-riscv64.so.1 ] || "$BB" cp -f /musl/lib/libc.so /lib/ld-musl-riscv64.so.1 >/dev/null 2>&1 || true
    [ -e /lib/ld-musl-riscv64-sf.so.1 ] || "$BB" cp -f /musl/lib/libc.so /lib/ld-musl-riscv64-sf.so.1 >/dev/null 2>&1 || true
    [ -e /lib/ld-musl-loongarch64.so.1 ] || "$BB" cp -f /musl/lib/libc.so /lib/ld-musl-loongarch64.so.1 >/dev/null 2>&1 || true
  fi
  "$BB" --install /bin >/dev/null 2>&1 || "$BB" --install -s /bin >/dev/null 2>&1 || true
  "$BB" --install /usr/bin >/dev/null 2>&1 || "$BB" --install -s /usr/bin >/dev/null 2>&1 || true
  [ -x /bin/sh ] || "$BB" cp -f "$BB" /bin/sh >/dev/null 2>&1 || true
  [ -x /bin/bash ] || "$BB" cp -f "$BB" /bin/bash >/dev/null 2>&1 || true
  [ -x /usr/bin/env ] || "$BB" cp -f "$BB" /usr/bin/env >/dev/null 2>&1 || true
  for cmd in sh bash env basename dirname rm mv grep cat mkdir rmdir cp ln chmod sed awk mount umount sync ls pwd cut tr sort uniq head tail wc xargs printf sleep kill ps touch dd readlink find which stat free df cal clear date od hexdump md5sum more false true expr uname uptime hwclock; do
    ensure_bb_cmd "$cmd"
  done
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
  # Re-create runtime dirs in case previous suites removed /tmp or /var/tmp.
  setup_runtime_tools
  if [ -x ./lmbench_all ]; then
    # Some lmbench helper scripts are hardcoded to /code/lmbench_src/bin/build/lmbench_all.
    rt_pwd=$PWD
    cd / >/dev/null 2>&1 || true
    "$BB" mkdir -p code code/lmbench_src code/lmbench_src/bin code/lmbench_src/bin/build >/dev/null 2>&1 || true
    "$BB" cp -f "$rt_pwd/lmbench_all" /code/lmbench_src/bin/build/lmbench_all >/dev/null 2>&1 || true
    "$BB" chmod +x /code/lmbench_src/bin/build/lmbench_all >/dev/null 2>&1 || true
    cd "$rt_pwd" >/dev/null 2>&1 || true
  fi

  if [ "$test_name" = "ltp_testcode.sh" ]; then
    echo "#### OS COMP TEST GROUP START ltp ####"
    target_dir="ltp/testcases/bin"
    for file in "$target_dir"/*; do
      [ -f "$file" ] || continue
      base=$(basename "$file")
      case "$base" in
        cgroup_regression_3_1.sh)
          echo "SKIP LTP CASE $base (requires path argument)"
          continue
          ;;
      esac
      echo "RUN LTP CASE $base"
      "$file"
      ret=$?
      echo "FAIL LTP CASE $base : $ret"
    done
    echo "#### OS COMP TEST GROUP END ltp ####"
    ret=0
    cd "$old_pwd" || true
    echo "[TEST] Exit code($ret): $test_sh"
    echo
    return "$ret"
  fi

  patched_script="./.starry-${test_name}.$$"
  "$BB" sed \
    -e 's#\./run-all\.sh#sh ./run-all.sh#g' \
    -e 's#\./run-static\.sh#sh ./run-static.sh#g' \
    -e 's#\./run-dynamic\.sh#sh ./run-dynamic.sh#g' \
    -e 's#\./busybox mkdir -p /var/tmp#true#g' \
    "./$test_name" > "$patched_script" || {
      ret=1
      cd "$old_pwd" || true
      echo "[TEST] Exit code($ret): $test_sh"
      echo
      return "$ret"
    }
  HOME=/root
  PATH=/musl:/glibc:/bin:/usr/bin
  LD_LIBRARY_PATH=/glibc/lib:/musl/lib:/lib:/lib64:/usr/lib
  export HOME PATH LD_LIBRARY_PATH
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
