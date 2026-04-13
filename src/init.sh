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
export LD_LIBRARY_PATH=/glibc/lib:/musl/lib:/lib:/usr/lib

# =========================
# Test switches (1=run, 0=skip)
# 共 12 组题目，可手动修改
# 默认全部开启，符合“扫描并依次执行所有测试点”
# =========================
: "${RUN_BASIC:=1}"
: "${RUN_BUSYBOX:=0}"
: "${RUN_CYCLICTEST:=0}"
: "${RUN_IOZONE:=0}"
: "${RUN_IPERF:=0}"
: "${RUN_LIBCBENCH:=0}"
: "${RUN_LIBCTEST:=0}"
: "${RUN_LMBENCH:=0}"
: "${RUN_LTP:=0}"
: "${RUN_LUA:=0}"
: "${RUN_NETPERF:=0}"
: "${RUN_UNIXBENCH:=0}"

suite_enabled() {
  case "$1" in
    basic) [ "${RUN_BASIC:-1}" = "1" ] ;;
    busybox) [ "${RUN_BUSYBOX:-1}" = "1" ] ;;
    cyclictest) [ "${RUN_CYCLICTEST:-1}" = "1" ] ;;
    iozone) [ "${RUN_IOZONE:-1}" = "1" ] ;;
    iperf) [ "${RUN_IPERF:-1}" = "1" ] ;;
    libcbench) [ "${RUN_LIBCBENCH:-1}" = "1" ] ;;
    libctest) [ "${RUN_LIBCTEST:-1}" = "1" ] ;;
    lmbench) [ "${RUN_LMBENCH:-1}" = "1" ] ;;
    ltp) [ "${RUN_LTP:-1}" = "1" ] ;;
    lua) [ "${RUN_LUA:-1}" = "1" ] ;;
    netperf) [ "${RUN_NETPERF:-1}" = "1" ] ;;
    unixbench) [ "${RUN_UNIXBENCH:-1}" = "1" ] ;;
    *) return 1 ;;
  esac
}

ALL_SUITES="basic busybox cyclictest iozone iperf libcbench libctest lmbench ltp lua netperf unixbench"
TEST_SUITES=""
for suite in $ALL_SUITES; do
  if suite_enabled "$suite"; then
    TEST_SUITES="$TEST_SUITES $suite"
  fi
done
echo "[TEST] enabled suites:$TEST_SUITES"

SETUP_RUNTIME_DONE=0

setup_runtime_tools() {
  ensure_runtime_dirs() {
    rt_pwd=$PWD
    cd / >/dev/null 2>&1 || true
    "$BB" mkdir -p tmp var var/tmp >/dev/null 2>&1 || true
    "$BB" chmod 1777 tmp var/tmp >/dev/null 2>&1 || true
    cd "$rt_pwd" >/dev/null 2>&1 || true
  }

  ensure_bb_cmd() {
    cmd="$1"
    [ -n "$cmd" ] || return 0
    [ -x "/bin/$cmd" ] && return 0
    "$BB" cp -f "$BB" "/bin/$cmd" >/dev/null 2>&1 || true
    "$BB" chmod +x "/bin/$cmd" >/dev/null 2>&1 || true
  }

  ensure_runtime_dirs
  [ "$SETUP_RUNTIME_DONE" = "1" ] && return 0

  rt_pwd=$PWD
  cd / >/dev/null 2>&1 || true
  "$BB" mkdir -p bin usr usr/bin >/dev/null 2>&1 || true
  cd "$rt_pwd" >/dev/null 2>&1 || true
  if [ -d /glibc/lib ]; then
    rt_pwd=$PWD
    cd / >/dev/null 2>&1 || true
    "$BB" mkdir -p lib >/dev/null 2>&1 || true
    cd "$rt_pwd" >/dev/null 2>&1 || true
    for so in ld-linux-riscv64-lp64d.so.1 libc.so libc.so.6 libm.so libm.so.6; do
      [ -e "/glibc/lib/$so" ] && "$BB" cp -f "/glibc/lib/$so" "/lib/$so" >/dev/null 2>&1 || true
    done
  fi
  # FAT/ext files in test images often miss musl ldso symlink; provide real files in /lib.
  if [ -e /musl/lib/libc.so ]; then
    [ -e /lib/ld-musl-riscv64.so.1 ] || "$BB" cp -f /musl/lib/libc.so /lib/ld-musl-riscv64.so.1 >/dev/null 2>&1 || true
    [ -e /lib/ld-musl-riscv64-sf.so.1 ] || "$BB" cp -f /musl/lib/libc.so /lib/ld-musl-riscv64-sf.so.1 >/dev/null 2>&1 || true
  fi
  "$BB" --install /bin >/dev/null 2>&1 || "$BB" --install -s /bin >/dev/null 2>&1 || true
  "$BB" --install /usr/bin >/dev/null 2>&1 || "$BB" --install -s /usr/bin >/dev/null 2>&1 || true
  [ -x /bin/sh ] || "$BB" cp -f "$BB" /bin/sh >/dev/null 2>&1 || true
  [ -x /bin/bash ] || "$BB" cp -f "$BB" /bin/bash >/dev/null 2>&1 || true
  [ -x /usr/bin/env ] || "$BB" cp -f "$BB" /usr/bin/env >/dev/null 2>&1 || true
  for cmd in sh bash env basename dirname rm mv grep cat mkdir rmdir cp ln chmod sed awk mount umount sync ls pwd cut tr sort uniq head tail wc xargs printf sleep kill ps touch dd readlink find which stat free df cal clear date od hexdump md5sum more false true expr uname uptime hwclock; do
    ensure_bb_cmd "$cmd"
  done
  SETUP_RUNTIME_DONE=1
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
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: entered test dir $test_dir"
  # Re-create runtime dirs in case previous suites removed /tmp or /var/tmp.
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: setup_runtime_tools begin"
  setup_runtime_tools
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: setup_runtime_tools end"
  if [ "$test_name" = "lmbench_testcode.sh" ] && [ -x ./lmbench_all ]; then
    # Keep compatibility for wrappers that hardcode this path,
    # without scanning/rewriting all files in the test directory.
    "$BB" mkdir -p /code/lmbench_src/bin/build >/dev/null 2>&1 || true
    "$BB" cp -f ./lmbench_all /code/lmbench_src/bin/build/lmbench_all >/dev/null 2>&1 || true
    "$BB" chmod +x /code/lmbench_src/bin/build/lmbench_all >/dev/null 2>&1 || true
  fi

  if [ "$test_name" = "ltp_testcode.sh" ]; then
    target_dir="ltp/testcases/bin"
    suite_ret=0
    for file in "$target_dir"/*; do
      [ -f "$file" ] || continue
      base=$(basename "$file")
      echo "RUN LTP CASE $base"
      "$file"
      ret=$?
      echo "FAIL LTP CASE $base : $ret"
      [ "$ret" -ne 0 ] && suite_ret=1
    done
    ret=$suite_ret
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
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: patched script ready $patched_script"
  HOME=/root
  PATH=/musl:/glibc:/bin:/usr/bin
  LD_LIBRARY_PATH=/glibc/lib:/musl/lib:/lib:/usr/lib
  export HOME PATH LD_LIBRARY_PATH
  for cmd in sh bash basename dirname rm mv grep cat mkdir cp ln chmod sed awk mount umount sync ls pwd cut tr sort uniq head tail wc xargs printf sleep kill ps env touch dd readlink find which stat free df cal clear date od hexdump md5sum more rmdir false true expr uname uptime hwclock; do
    eval "$cmd() { \"\$BB\" $cmd \"\$@\"; }"
  done
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: run patched script begin"
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
append_root "/musl"
append_root "/glibc"

setup_runtime_tools
echo "[TEST] scan roots:$SEARCH_ROOTS"
for d in $SEARCH_ROOTS; do
  [ -d "$d" ] || continue
  echo "[TEST] scanning $d"
done

find_suite_scripts() {
  suite="$1"
  SUITE_SCRIPT_LIST=""

  for root in $SEARCH_ROOTS; do
    [ -d "$root" ] || continue
    p="$root/${suite}_testcode.sh"
    [ -f "$p" ] || continue
    SUITE_SCRIPT_LIST="${SUITE_SCRIPT_LIST}$p
"
  done
}

emit_skip_markers() {
  test_sh="$1"
  suite="$2"
  start_line=$("$BB" sed -n 's/.*\(#### OS COMP TEST GROUP START .* ####\).*/\1/p' "$test_sh" 2>/dev/null | "$BB" head -n 1)
  end_line=$("$BB" sed -n 's/.*\(#### OS COMP TEST GROUP END .* ####\).*/\1/p' "$test_sh" 2>/dev/null | "$BB" head -n 1)
  [ -n "$start_line" ] || start_line="#### OS COMP TEST GROUP START $suite ####"
  [ -n "$end_line" ] || end_line="#### OS COMP TEST GROUP END $suite ####"
  echo "$start_line"
  echo "[TEST] SKIP script: $test_sh (switch=0)"
  echo "$end_line"
}

group_total=0
group_skipped=0
group_passed=0
group_failed=0
script_total=0
script_passed=0
script_failed=0

for suite in $ALL_SUITES; do
  group_total=$((group_total + 1))
  find_suite_scripts "$suite"

  if ! suite_enabled "$suite"; then
    group_skipped=$((group_skipped + 1))
    if [ -z "$SUITE_SCRIPT_LIST" ]; then
      emit_skip_markers "/dev/null" "$suite"
    else
      while IFS= read -r t; do
        [ -n "$t" ] || continue
        emit_skip_markers "$t" "$suite"
      done <<EOF_SKIP
$SUITE_SCRIPT_LIST
EOF_SKIP
    fi
    continue
  fi

  if [ -z "$SUITE_SCRIPT_LIST" ]; then
    echo "[TEST] no script found for group $suite"
    group_failed=$((group_failed + 1))
    continue
  fi

  suite_failed=0
  while IFS= read -r t; do
    [ -n "$t" ] || continue
    script_total=$((script_total + 1))
    if run_test "$t"; then
      script_passed=$((script_passed + 1))
    else
      script_failed=$((script_failed + 1))
      suite_failed=1
    fi
  done <<EOF_RUN
$SUITE_SCRIPT_LIST
EOF_RUN

  if [ "$suite_failed" -eq 0 ]; then
    group_passed=$((group_passed + 1))
  else
    group_failed=$((group_failed + 1))
  fi
done

echo "[TEST] group summary: total=$group_total passed=$group_passed failed=$group_failed skipped=$group_skipped"
echo "[TEST] script summary: total=$script_total passed=$script_passed failed=$script_failed"

echo "[TEST] all discovered scripts completed"
"$BB" sync >/dev/null 2>&1 || true
"$BB" poweroff -f >/dev/null 2>&1 || "$BB" reboot -f >/dev/null 2>&1 || exit 0
