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
# Ensure shebang scripts like `./run-all.sh` can find `/bin/sh`.
"$BB" mkdir -p /bin >/dev/null 2>&1 || true
"$BB" cp "$BB" /bin/busybox >/dev/null 2>&1 || true
"$BB" chmod +x /bin/busybox >/dev/null 2>&1 || true
"$BB" cp "$BB" /bin/sh >/dev/null 2>&1 || true
"$BB" chmod +x /bin/sh >/dev/null 2>&1 || true
# Install busybox applets (e.g. sleep, kill, grep) to avoid missing-command failures.
"$BB" --install -s /bin >/dev/null 2>&1 || true

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

# Scan all mounted filesystems (exclude pseudo filesystems only).
if [ -r /proc/mounts ]; then
  while IFS=' ' read -r _dev m fs_type _rest; do
    [ -n "$m" ] || continue
    case "$fs_type" in
      proc|sysfs|devtmpfs|devpts|tmpfs|cgroup|cgroup2|mqueue|pstore|debugfs|tracefs|securityfs|ramfs|configfs|fusectl|hugetlbfs|bpf|overlay)
        continue
        ;;
    esac
    case "$m" in
      /proc|/proc/*|/sys|/sys/*|/dev|/dev/*|/run|/run/*)
        continue
        ;;
    esac
    append_root "$m"
  done < /proc/mounts
fi

echo "[TEST] scan roots:$SEARCH_ROOTS"

TEST_LIST=$(
  for d in $SEARCH_ROOTS; do
    [ -d "$d" ] || continue
    for p in \
      "$d"/*_testcode.sh \
      "$d"/*_test.sh \
      "$d"/test_*.sh \
      "$d"/*test*.sh
    do
      [ -f "$p" ] && echo "$p"
    done
  done | "$BB" sort -u
)

if [ -z "$TEST_LIST" ]; then
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
