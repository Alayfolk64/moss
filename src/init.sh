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

run_test() {
  test_sh="$1"
  test_dir=$("$BB" dirname "$test_sh")
  test_name=$("$BB" basename "$test_sh")
  echo "[TEST] Running $test_sh"
  (
    cd "$test_dir" || exit 127
    "$BB" env HOME=/root PATH=/musl:/glibc:/bin:/usr/bin "$BB" sh "./$test_name"
  )
  ret=$?
  echo "[TEST] Exit code($ret): $test_sh"
  echo
}

TEST_LIST=$("$BB" find /musl /glibc -type f -name '*_testcode.sh' 2>/dev/null | "$BB" sort)

if [ -z "$TEST_LIST" ]; then
  echo "[TEST] no *_testcode.sh found under /musl or /glibc"
else
  printf '%s\n' "$TEST_LIST" | while IFS= read -r t; do
    [ -n "$t" ] || continue
    run_test "$t"
  done
fi

echo "[TEST] all discovered scripts completed"
