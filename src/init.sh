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

bb_try() {
  "$BB" "$@" >/dev/null 2>&1 || true
}

copy_if_exists() {
  src="$1"
  dst="$2"
  [ -e "$src" ] || return 0
  bb_try cp -f "$src" "$dst"
}

copy_if_missing() {
  src="$1"
  dst="$2"
  [ -e "$dst" ] && return 0
  copy_if_exists "$src" "$dst"
}

RUNTIME_EXTRA_LD_LIBRARY_PATH=""
RUNTIME_DISK_MOUNT_DIR=/dev/shm/.starry-runtime-disk
RUNTIME_DISK_LIBGCC_SRC=""
RUNTIME_TMP_BASE=/tmp

append_extra_ld_library_path() {
  d="$1"
  [ -n "$d" ] || return 0
  case ":$RUNTIME_EXTRA_LD_LIBRARY_PATH:" in
    *":$d:"*) ;;
    "")
      RUNTIME_EXTRA_LD_LIBRARY_PATH="$d"
      ;;
    *)
      RUNTIME_EXTRA_LD_LIBRARY_PATH="$RUNTIME_EXTRA_LD_LIBRARY_PATH:$d"
      ;;
  esac
}

detect_runtime_tmp_base() {
  for d in /dev/shm /tmp /var/tmp /root; do
    [ -d "$d" ] || continue
    probe="$d/.starry-writable.$$"
    "$BB" touch "$probe" >/dev/null 2>&1 || continue
    "$BB" rm -f "$probe" >/dev/null 2>&1 || true
    RUNTIME_TMP_BASE="$d"
    break
  done
}

find_libgcc_from_runtime_disk() {
  [ -n "$ARCH_NAME" ] || return 1
  [ -n "$RUNTIME_DISK_LIBGCC_SRC" ] && return 0

  echo "[TEST][runtime] probing runtime disk for libgcc..."
  for mp in \
    /dev/shm/.starry-runtime-disk \
    /tmp/.starry-runtime-disk \
    /var/tmp/.starry-runtime-disk \
    /dev/shm \
    /tmp \
    /var/tmp \
    /root \
    /mnt
  do
    case "$mp" in
      */.starry-runtime-disk)
        bb_try mkdir -p "$mp"
        ;;
    esac
    [ -d "$mp" ] || continue
    RUNTIME_DISK_MOUNT_DIR="$mp"
    break
  done
  if [ ! -d "$RUNTIME_DISK_MOUNT_DIR" ]; then
    echo "[TEST][runtime] WARN: no usable mount dir for runtime disk"
    return 1
  fi

  found_dev=0
  for dev in /dev/vd[a-z] /dev/xvd[a-z] /dev/sd[a-z] /dev/nvme*n1; do
    [ -b "$dev" ] || continue
    found_dev=1
    echo "[TEST][runtime] try mount $dev -> $RUNTIME_DISK_MOUNT_DIR"
    # Use `-n` to avoid /etc/mtab writes on read-only /etc images.
    mounted_ok=0
    "$BB" mount -n -t ext4 -o ro "$dev" "$RUNTIME_DISK_MOUNT_DIR" >/dev/null 2>&1 && mounted_ok=1
    if [ "$mounted_ok" -ne 1 ] && [ -f "$RUNTIME_DISK_MOUNT_DIR/.starry-runtime-disk" ]; then
      # Some busybox builds may return non-zero even when mount(2) succeeded.
      mounted_ok=1
    fi
    [ "$mounted_ok" -eq 1 ] || continue
    echo "[TEST][runtime] mounted $dev"

    for cand in \
      "$RUNTIME_DISK_MOUNT_DIR/libgcc/$ARCH_NAME/libgcc_s.so.1" \
      "$RUNTIME_DISK_MOUNT_DIR/$ARCH_NAME/libgcc_s.so.1" \
      "$RUNTIME_DISK_MOUNT_DIR/libgcc_s.so.1"
    do
      [ -f "$cand" ] || continue
      RUNTIME_DISK_LIBGCC_SRC="$cand"
      append_extra_ld_library_path "${cand%/*}"
      echo "[TEST][runtime] libgcc source from runtime disk: $cand"
      return 0
    done

    echo "[TEST][runtime] libgcc not found under mounted $dev"
    "$BB" umount -n "$RUNTIME_DISK_MOUNT_DIR" >/dev/null 2>&1 || true
  done

  [ "$found_dev" -eq 1 ] || echo "[TEST][runtime] no block node matched: /dev/vd[a-z] /dev/xvd[a-z] /dev/sd[a-z] /dev/nvme*n1"
  return 1
}

ensure_bb_cmd() {
  cmd="$1"
  [ -n "$cmd" ] || return 0
  [ -x "/bin/$cmd" ] && return 0
  bb_try cp -f "$BB" "/bin/$cmd"
  bb_try chmod +x "/bin/$cmd"
}

ensure_runtime_dirs() {
  bb_try mkdir -p /tmp /var /var/tmp /var/tmp/lmbench
  bb_try chmod 1777 /tmp /var/tmp
}

etc_is_writable() {
  "$BB" sh -c 'printf "x\n" > /etc/.starry-rw-test' >/dev/null 2>&1 || return 1
  "$BB" rm -f /etc/.starry-rw-test >/dev/null 2>&1 || true
  return 0
}

ensure_writable_etc() {
  bb_try mkdir -p /etc
  etc_is_writable && return 0

  # Some images mount root read-only. Try remounting root first.
  "$BB" mount -o remount,rw / >/dev/null 2>&1 || true
  "$BB" mount -o remount,rw rootfs / >/dev/null 2>&1 || true
  bb_try mkdir -p /etc
  etc_is_writable && return 0

  # Fallback: mount tmpfs on /etc if mountpoint exists.
  if [ -d /etc ] || "$BB" mkdir -p /etc >/dev/null 2>&1; then
    "$BB" mount -t tmpfs -o mode=755,size=1m tmpfs /etc >/dev/null 2>&1 || true
  fi
  etc_is_writable && return 0

  return 1
}

setup_runtime_users() {
  bb_try mkdir -p /home/nobody
  if ! ensure_writable_etc; then
    echo "[TEST][runtime] WARN: /etc is not writable; cannot prepare passwd/group"
    return 0
  fi

  if [ ! -f /etc/passwd ]; then
    "$BB" sh -c 'printf "%s\n" "root:x:0:0:root:/root:/bin/sh" "nobody:x:65534:65534:nobody:/home/nobody:/bin/sh" > /etc/passwd' >/dev/null 2>&1 || true
  elif ! "$BB" grep -q '^nobody:' /etc/passwd >/dev/null 2>&1; then
    "$BB" sh -c 'printf "%s\n" "nobody:x:65534:65534:nobody:/home/nobody:/bin/sh" >> /etc/passwd' >/dev/null 2>&1 || true
  fi

  if [ ! -f /etc/group ]; then
    "$BB" sh -c 'printf "%s\n" "root:x:0:" "nobody:x:65534:" > /etc/group' >/dev/null 2>&1 || true
  elif ! "$BB" grep -q '^nobody:' /etc/group >/dev/null 2>&1; then
    "$BB" sh -c 'printf "%s\n" "nobody:x:65534:" >> /etc/group' >/dev/null 2>&1 || true
  fi

  if [ ! -f /etc/nsswitch.conf ]; then
    "$BB" sh -c 'printf "%s\n" "passwd: files" "group: files" "shadow: files" "hosts: files dns" > /etc/nsswitch.conf' >/dev/null 2>&1 || true
  fi

}

setup_glibc_runtime() {
  [ -d /glibc/lib ] || return 0

  bb_try mkdir -p /lib /lib64
  for so in libc.so libc.so.6 libm.so libm.so.6 libnss_files.so.2 libresolv.so.2 libnss_dns.so.2; do
    copy_if_exists "/glibc/lib/$so" "/lib/$so"
    copy_if_exists "/glibc/lib/$so" "/lib64/$so"
  done

  # Do not rely on arch detection here; just populate all known loaders.
  # Missing files are ignored by copy_if_exists.
  for ldso in \
    ld-linux-loongarch-lp64d.so.1 \
    ld-linux-riscv64-lp64d.so.1
  do
    copy_if_exists "/glibc/lib/$ldso" "/lib/$ldso"
    copy_if_exists "/glibc/lib/$ldso" "/lib64/$ldso"
  done
}

setup_musl_runtime() {
  # FAT/ext files in test images often miss musl ldso symlink; provide real files in /lib.
  [ -e /musl/lib/libc.so ] || return 0

  bb_try mkdir -p /lib /lib64 /musl/lib
  # Do not rely on arch detection here; just populate all known loader names.
  # Existing files are kept and copy_if_missing suppresses failures.
  for musl_ldso in \
    /lib/ld-musl-loongarch64.so.1 \
    /lib/ld-musl-loongarch-lp64d.so.1 \
    /lib64/ld-musl-loongarch-lp64d.so.1 \
    /lib/ld-musl-riscv64.so.1 \
    /lib/ld-musl-riscv64-sf.so.1
  do
    copy_if_missing /musl/lib/libc.so "$musl_ldso"
  done
}

setup_libgcc_runtime() {
  bb_try mkdir -p /lib /lib64 /usr/lib /usr/lib64 /glibc/lib64 /musl/lib

  libgcc_src=""
  for cand in /musl/lib/libgcc_s.so.1 /glibc/lib/libgcc_s.so.1; do
    [ -f "$cand" ] || continue
    libgcc_src="$cand"
    break
  done

  if [ -z "$libgcc_src" ]; then
    find_libgcc_from_runtime_disk || true
    [ -n "$RUNTIME_DISK_LIBGCC_SRC" ] && libgcc_src="$RUNTIME_DISK_LIBGCC_SRC"
  fi

  if [ -z "$libgcc_src" ]; then
    echo "[TEST][runtime] WARN: libgcc_s.so.1 not found in rootfs/runtime disk"
    return 0
  fi

  append_extra_ld_library_path "${libgcc_src%/*}"
  copy_if_missing "$libgcc_src" /musl/lib/libgcc_s.so.1
  copy_if_missing "$libgcc_src" /lib/libgcc_s.so.1
  copy_if_missing "$libgcc_src" /lib64/libgcc_s.so.1
  copy_if_missing "$libgcc_src" /usr/lib/libgcc_s.so.1
  copy_if_missing "$libgcc_src" /usr/lib64/libgcc_s.so.1
  copy_if_missing "$libgcc_src" /glibc/lib/libgcc_s.so.1
  copy_if_missing "$libgcc_src" /glibc/lib64/libgcc_s.so.1
  echo "[TEST][runtime] libgcc bound: arch=$ARCH_NAME source=$libgcc_src target=/musl/lib,/lib,/lib64,/usr/lib,/usr/lib64,/glibc/lib"
}

setup_locale_runtime() {
  bb_try mkdir -p /usr/lib/locale /usr/share/locale

  [ -d /usr/lib/locale/C.utf8 ] || "$BB" cp -a /glibc/usr/lib/locale/C.utf8 /usr/lib/locale/C.utf8 >/dev/null 2>&1 || true
  [ -d /usr/lib/locale/C.utf8 ] || "$BB" cp -a /glibc/lib/locale/C.utf8 /usr/lib/locale/C.utf8 >/dev/null 2>&1 || true
  [ -d /usr/lib/locale/C.utf8 ] || "$BB" cp -a /musl/usr/lib/locale/C.utf8 /usr/lib/locale/C.utf8 >/dev/null 2>&1 || true
  [ -d /usr/lib/locale/C.utf8 ] || "$BB" cp -a /musl/lib/locale/C.utf8 /usr/lib/locale/C.utf8 >/dev/null 2>&1 || true
  [ -d /usr/lib/locale/C.utf8 ] || "$BB" cp -a /usr/lib/locale/C.UTF-8 /usr/lib/locale/C.utf8 >/dev/null 2>&1 || true
  [ -d /usr/lib/locale/C.UTF-8 ] || "$BB" cp -a /usr/lib/locale/C.utf8 /usr/lib/locale/C.UTF-8 >/dev/null 2>&1 || true
  [ -f /usr/share/locale/locale.alias ] || copy_if_exists /glibc/usr/share/locale/locale.alias /usr/share/locale/locale.alias
  [ -f /usr/share/locale/locale.alias ] || copy_if_exists /glibc/lib/locale/locale.alias /usr/share/locale/locale.alias
  [ -f /usr/share/locale/locale.alias ] || copy_if_exists /musl/usr/share/locale/locale.alias /usr/share/locale/locale.alias
  [ -f /usr/share/locale/locale.alias ] || copy_if_exists /musl/lib/locale/locale.alias /usr/share/locale/locale.alias
  [ -f /usr/share/locale/locale.alias ] || copy_if_exists /usr/lib/locale/locale.alias /usr/share/locale/locale.alias
  echo "[TEST][runtime] locale bound: /usr/lib/locale/C.utf8,/usr/lib/locale/C.UTF-8 /usr/share/locale/locale.alias"
}

install_busybox_commands() {
  bb_try --install /bin
  bb_try --install -s /bin
  bb_try --install /usr/bin
  bb_try --install -s /usr/bin

  copy_if_missing "$BB" /bin/sh
  copy_if_missing "$BB" /bin/bash
  copy_if_missing "$BB" /usr/bin/env
  bb_try chmod +x /bin/sh
  bb_try chmod +x /bin/bash
  bb_try chmod +x /usr/bin/env

  for cmd in sh bash env basename dirname rm mv grep cat mkdir rmdir cp ln chmod sed awk mount umount sync ls pwd cut tr sort uniq head tail wc xargs printf sleep kill ps touch dd readlink find which stat free df cal clear date od hexdump md5sum more false true expr uname uptime hwclock ar locale timeout; do
    ensure_bb_cmd "$cmd"
  done
}

is_gnu_ar() {
  tool="$1"
  [ -x "$tool" ] || return 1
  "$tool" --version 2>/dev/null | grep -qi "GNU ar"
}

prefer_gnu_ar() {
  if command -v ar >/dev/null 2>&1; then
    current_ar=$(command -v ar 2>/dev/null)
    if [ -n "$current_ar" ] && is_gnu_ar "$current_ar"; then
      return 0
    fi
  fi

  for tool in /musl/bin/ar /glibc/bin/ar /usr/local/bin/ar /musl/ar /glibc/ar; do
    if is_gnu_ar "$tool"; then
      copy_if_exists "$tool" /usr/bin/ar
      copy_if_exists "$tool" /bin/ar
      bb_try chmod +x /usr/bin/ar /bin/ar
      return 0
    fi
  done

  # Fallback: keep busybox ar if GNU ar is unavailable in this rootfs.
  ensure_bb_cmd ar
}

ensure_locale_fallback() {
  # Some LTP scripts shell out to `locale`; provide a tiny fallback when the
  # command is unavailable in busybox/rootfs.
  if ! command -v locale >/dev/null 2>&1; then
    bb_try mkdir -p /bin
    "$BB" sh -c 'printf "%s\n" "#!/bin/sh" "echo LANG=C" > /bin/locale' >/dev/null 2>&1 || true
    bb_try chmod +x /bin/locale
  fi
}

# Ensure shebang scripts like `./run-all.sh` can find `/bin/sh`.
bb_try mkdir -p /bin
bb_try cp -f "$BB" /bin/busybox
bb_try chmod +x /bin/busybox
copy_if_missing "$BB" /bin/sh
bb_try chmod +x /bin/sh

RUNTIME_PATH=/musl:/glibc:/bin:/usr/bin
RUNTIME_LD_LIBRARY_PATH=/glibc/lib:/glibc/lib64:/musl/lib:/lib:/lib64:/usr/lib:/usr/lib64
RUNTIME_LOCALE=C.UTF-8
RUNTIME_LOCPATH=/usr/lib/locale:/glibc/usr/lib/locale:/glibc/lib/locale:/musl/usr/lib/locale:/musl/lib/locale
PATH=$RUNTIME_PATH
LD_LIBRARY_PATH=$RUNTIME_LD_LIBRARY_PATH
LANG=$RUNTIME_LOCALE
LC_ALL=$RUNTIME_LOCALE
LC_CTYPE=$RUNTIME_LOCALE
LOCPATH=$RUNTIME_LOCPATH
export PATH LD_LIBRARY_PATH LANG LC_ALL LC_CTYPE LOCPATH

detect_arch() {
  detected=$("$BB" uname -m 2>/dev/null)
  [ -n "$detected" ] || detected=$(uname -m 2>/dev/null)
  case "$detected" in
    loongarch64|riscv64)
      echo "$detected"
      return 0
      ;;
  esac
  if [ -e /glibc/lib/ld-linux-loongarch-lp64d.so.1 ]; then
    echo "loongarch64"
    return 0
  elif [ -e /glibc/lib/ld-linux-riscv64-lp64d.so.1 ]; then
    echo "riscv64"
    return 0
  fi
  echo "[TEST] unsupported arch: ${detected:-unknown} (only riscv64/loongarch64 supported)" >&2
  return 1
}

runtime_ld_library_path_for_test() {
  test_sh="$1"
  base=""
  case "$test_sh" in
    /musl/*)
      base="/musl/lib:/lib:/lib64:/usr/lib:/usr/lib64"
      ;;
    /glibc/*)
      base="/glibc/lib:/glibc/lib64:/lib:/lib64:/usr/lib:/usr/lib64"
      ;;
    *)
      base="$RUNTIME_LD_LIBRARY_PATH"
      ;;
  esac
  if [ -n "$RUNTIME_EXTRA_LD_LIBRARY_PATH" ]; then
    echo "$base:$RUNTIME_EXTRA_LD_LIBRARY_PATH"
  else
    echo "$base"
  fi
}

runtime_locale_for_test() {
  _test_name="$1"
  case "$_test_name" in
    libctest_testcode.sh)
      # libc-test clocale_mbfuncs expects plain C locale semantics.
      echo "C"
      ;;
    *)
      echo "$RUNTIME_LOCALE"
      ;;
  esac
}

ARCH_NAME=$(detect_arch) || exit 1
echo "[TEST] detected arch: $ARCH_NAME"

# =========================
# Test switches (1=run, 0=skip)
# 12 suites; defaults can be overridden by environment variables.
# =========================
: "${RUN_BASIC:=0}"
: "${RUN_BUSYBOX:=0}"
: "${RUN_CYCLICTEST:=0}"
: "${RUN_IOZONE:=0}"
: "${RUN_IPERF:=0}"
: "${RUN_LIBCBENCH:=0}"
: "${RUN_LIBCTEST:=1}"
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
  ensure_runtime_dirs
  [ "$SETUP_RUNTIME_DONE" = "1" ] && return 0

  bb_try mkdir -p /bin /usr/bin
  detect_runtime_tmp_base
  setup_runtime_users
  setup_glibc_runtime
  setup_musl_runtime
  setup_libgcc_runtime
  setup_locale_runtime
  install_busybox_commands
  prefer_gnu_ar
  ensure_locale_fallback
  SETUP_RUNTIME_DONE=1
}

suite_group_name() {
  test_sh="$1"
  suite="$2"
  case "$test_sh" in
    /musl/*) echo "$suite-musl" ;;
    /glibc/*) echo "$suite-glibc" ;;
    *) echo "$suite" ;;
  esac
}

group_marker_line() {
  kind="$1"
  test_sh="$2"
  suite="$3"
  echo "#### OS COMP TEST GROUP $kind $(suite_group_name "$test_sh" "$suite") ####"
}

run_test() {
  test_sh="$1"
  test_dir=${test_sh%/*}
  test_name=${test_sh##*/}
  is_ltp_script=0
  [ "$test_name" = "ltp_testcode.sh" ] && is_ltp_script=1
  old_pwd=$PWD
  [ "$test_dir" = "$test_sh" ] && test_dir=.
  [ "$is_ltp_script" -eq 1 ] || echo "[TEST] Running $test_sh"
  cd "$test_dir" || {
    ret=127
    if [ "$is_ltp_script" -ne 1 ]; then
      echo "[TEST] Exit code($ret): $test_sh"
      echo
    fi
    return "$ret"
  }
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: entered test dir $test_dir"
  # Re-create runtime dirs in case previous suites removed /tmp or /var/tmp.
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: setup_runtime_tools begin"
  setup_runtime_tools
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: setup_runtime_tools end"

  # Keep libctest in-place: recursive copy to temp dir can trigger severe memory
  # pressure on constrained images and causes unrelated failures.

  run_dynamic_cmd='sh ./run-dynamic.sh'
  libctest_dynamic_temp=""
  libctest_dynamic_wrapper=""
  if [ "$test_name" = "libctest_testcode.sh" ] && [ -f ./run-dynamic.sh ]; then
    libctest_dynamic_temp="$RUNTIME_TMP_BASE/.starry-run-dynamic.$$"
    case "$test_sh" in
      /musl/*)
        musl_loader=""
        case "$ARCH_NAME" in
          riscv64)
            for cand in \
              /musl/lib/ld-musl-riscv64-sf.so.1 \
              /musl/lib/ld-musl-riscv64.so.1 \
              /musl/lib/libc.so \
              /lib/ld-musl-riscv64-sf.so.1 \
              /lib/ld-musl-riscv64.so.1
            do
              [ -e "$cand" ] || continue
              musl_loader="$cand"
              break
            done
            ;;
          loongarch64)
            for cand in \
              /musl/lib/ld-musl-loongarch-lp64d.so.1 \
              /musl/lib/ld-musl-loongarch64.so.1 \
              /musl/lib/libc.so \
              /lib/ld-musl-loongarch-lp64d.so.1 \
              /lib/ld-musl-loongarch64.so.1
            do
              [ -e "$cand" ] || continue
              musl_loader="$cand"
              break
            done
            ;;
        esac

        if [ -n "$musl_loader" ]; then
          loader_library_path="/musl/lib:/lib:/lib64:/usr/lib:/usr/lib64"
          if [ -n "$RUNTIME_EXTRA_LD_LIBRARY_PATH" ]; then
            loader_library_path="$loader_library_path:$RUNTIME_EXTRA_LD_LIBRARY_PATH"
          fi
          libctest_dynamic_wrapper="$RUNTIME_TMP_BASE/.starry-entry-dynamic.$$"
          entry_dynamic_path="$PWD/entry-dynamic.exe"
          {
            printf '%s\n' '#!/bin/sh'
            printf 'exec "%s" --library-path "%s" "%s" "$@"\n' \
              "$musl_loader" "$loader_library_path" "$entry_dynamic_path"
          } >"$libctest_dynamic_wrapper" 2>/dev/null || libctest_dynamic_wrapper=""
          [ -n "$libctest_dynamic_wrapper" ] && "$BB" chmod +x "$libctest_dynamic_wrapper" >/dev/null 2>&1 || true
          if [ -n "$libctest_dynamic_wrapper" ]; then
            echo "[TEST][libctest] dynamic wrapper: $libctest_dynamic_wrapper (loader=$musl_loader)"
          else
            echo "[TEST][libctest] WARN: failed to create dynamic wrapper under $RUNTIME_TMP_BASE"
          fi
        else
          echo "[TEST][libctest] WARN: musl loader not found, keep original run-dynamic.sh"
        fi
        ;;
      /glibc/*)
        glibc_loader=""
        case "$ARCH_NAME" in
          riscv64)
            for cand in \
              /glibc/lib/ld-linux-riscv64-lp64d.so.1 \
              /lib/ld-linux-riscv64-lp64d.so.1 \
              /lib64/ld-linux-riscv64-lp64d.so.1
            do
              [ -e "$cand" ] || continue
              glibc_loader="$cand"
              break
            done
            ;;
          loongarch64)
            for cand in \
              /glibc/lib/ld-linux-loongarch-lp64d.so.1 \
              /lib/ld-linux-loongarch-lp64d.so.1 \
              /lib64/ld-linux-loongarch-lp64d.so.1
            do
              [ -e "$cand" ] || continue
              glibc_loader="$cand"
              break
            done
            ;;
        esac

        if [ -n "$glibc_loader" ]; then
          loader_library_path="/glibc/lib:/glibc/lib64:/lib:/lib64:/usr/lib:/usr/lib64"
          if [ -n "$RUNTIME_EXTRA_LD_LIBRARY_PATH" ]; then
            loader_library_path="$loader_library_path:$RUNTIME_EXTRA_LD_LIBRARY_PATH"
          fi
          libctest_dynamic_wrapper="$RUNTIME_TMP_BASE/.starry-entry-dynamic.$$"
          entry_dynamic_path="$PWD/entry-dynamic.exe"
          {
            printf '%s\n' '#!/bin/sh'
            printf 'exec "%s" --library-path "%s" "%s" "$@"\n' \
              "$glibc_loader" "$loader_library_path" "$entry_dynamic_path"
          } >"$libctest_dynamic_wrapper" 2>/dev/null || libctest_dynamic_wrapper=""
          [ -n "$libctest_dynamic_wrapper" ] && "$BB" chmod +x "$libctest_dynamic_wrapper" >/dev/null 2>&1 || true
          if [ -n "$libctest_dynamic_wrapper" ]; then
            echo "[TEST][libctest] dynamic wrapper: $libctest_dynamic_wrapper (loader=$glibc_loader)"
          else
            echo "[TEST][libctest] WARN: failed to create dynamic wrapper under $RUNTIME_TMP_BASE"
          fi
        else
          echo "[TEST][libctest] WARN: glibc loader not found, keep original run-dynamic.sh"
        fi
        ;;
    esac

    if [ -n "$libctest_dynamic_wrapper" ]; then
      "$BB" sed -e "s#entry-dynamic\\.exe#$libctest_dynamic_wrapper#g" \
        ./run-dynamic.sh > "$libctest_dynamic_temp" || libctest_dynamic_temp=""
    else
      "$BB" cp -f ./run-dynamic.sh "$libctest_dynamic_temp" >/dev/null 2>&1 || libctest_dynamic_temp=""
    fi

    if [ -n "$libctest_dynamic_temp" ]; then
      "$BB" chmod +x "$libctest_dynamic_temp" >/dev/null 2>&1 || true
      run_dynamic_cmd="sh $libctest_dynamic_temp"
      echo "[TEST][libctest] run dynamic via: $libctest_dynamic_temp"
    else
      echo "[TEST][libctest] WARN: failed to prepare patched run-dynamic script"
    fi
  fi

  if [ "$test_name" = "lmbench_testcode.sh" ] && [ -x ./lmbench_all ]; then
    # Keep compatibility for wrappers that hardcode this path,
    # without scanning/rewriting all files in the test directory.
    "$BB" mkdir -p /code/lmbench_src/bin/build >/dev/null 2>&1 || true
    "$BB" cp -f ./lmbench_all /code/lmbench_src/bin/build/lmbench_all >/dev/null 2>&1 || true
    "$BB" chmod +x /code/lmbench_src/bin/build/lmbench_all >/dev/null 2>&1 || true
  fi

  if [ "$test_name" = "ltp_testcode.sh" ]; then
    ltp_root="$PWD/ltp"
    target_dir="$ltp_root/testcases/bin"
    ltp_lib_dir="$ltp_root/testcases/lib"
    ltp_testscripts_dir="$ltp_root/testscripts"
    ltp_group_name=$(suite_group_name "$test_sh" "ltp")
    # Hardcode to embedded src/ltp.txt content only (via LTP_CASES from main.rs).
    # Do not consume in-image /ltp.txt to avoid stale case lists.
    ltp_case_list=/tmp/.ltp_cases_from_src.txt

    # LTP shell tests often source helper scripts with `. helper.sh` and run
    # helper commands (e.g. tst_brk) that are resolved via PATH.
    # Export LTP paths here to avoid "not found" in those cases.
    PATH="$target_dir:$ltp_lib_dir:$ltp_testscripts_dir:$PATH"
    LTPROOT="$ltp_root"
    export PATH LTPROOT

    if [ -n "${LTP_CASES:-}" ] && : > "$ltp_case_list" 2>/dev/null; then
      for ltp_case in $LTP_CASES; do
        printf "%s\n" "$ltp_case" >> "$ltp_case_list"
      done
      echo "[TEST][ltp] loaded case list from embedded src/ltp.txt: $ltp_case_list"
    else
      echo "[TEST][ltp] missing embedded case list (LTP_CASES) or cannot write: $ltp_case_list"
    fi
    echo "#### OS COMP TEST GROUP START $ltp_group_name ####"
    ret=0
    if cd "$target_dir" 2>/dev/null; then
      if [ -f /etc/passwd ]; then
        echo "[TEST][ltp] /etc/passwd present"
        if "$BB" grep -q '^nobody:' /etc/passwd >/dev/null 2>&1; then
          echo "[TEST][ltp] /etc/passwd has nobody"
        else
          echo "[TEST][ltp] /etc/passwd missing nobody"
        fi
      else
        echo "[TEST][ltp] /etc/passwd absent"
      fi
      if [ -r "$ltp_case_list" ]; then
        # Avoid depending on external `tr` on minimal judge images.
        # Strip one trailing CR (Windows line ending) with POSIX shell expansion.
        ltp_cr=$(printf '\r')
        while IFS= read -r ltp_case_raw || [ -n "$ltp_case_raw" ]; do
          ltp_case=${ltp_case_raw%"$ltp_cr"}
          [ -n "$ltp_case" ] || continue
          case "$ltp_case" in
            \#*) continue ;;
          esac

          echo "RUN LTP CASE $ltp_case"
          if [ -x "./$ltp_case" ]; then
            "./$ltp_case"
            case_ret=$?
          elif [ -f "./$ltp_case" ]; then
            sh "./$ltp_case"
            case_ret=$?
          else
            case_ret=127
            echo "[TEST][ltp] missing case: $target_dir/$ltp_case"
          fi
          echo "FAIL LTP CASE $ltp_case : $case_ret"
        done < "$ltp_case_list"
      else
        ret=127
        echo "[TEST][ltp] case list not readable: $ltp_case_list"
      fi
    else
      ret=127
      echo "[TEST][ltp] missing target dir: $target_dir"
    fi
    echo "#### OS COMP TEST GROUP END $ltp_group_name ####"

    cd "$old_pwd" || true
    return "$ret"
  fi

  patched_script="./.starry-${test_name}.$$"
  "$BB" sed \
    -e 's#\./run-all\.sh#sh ./run-all.sh#g' \
    -e 's#\./run-static\.sh#sh ./run-static.sh#g' \
    -e "s#\\./run-dynamic\\.sh#$run_dynamic_cmd#g" \
    -e 's#\./busybox mkdir -p /var/tmp#true#g' \
    -e 's#\./lmbench_all lat_fs /var/tmp#echo "[TEST] SKIP lmbench lat_fs /var/tmp (known hang)"#g' \
    "./$test_name" > "$patched_script" || {
      ret=1
      [ -n "$libctest_dynamic_temp" ] && "$BB" rm -f "$libctest_dynamic_temp" >/dev/null 2>&1 || true
      [ -n "$libctest_dynamic_wrapper" ] && "$BB" rm -f "$libctest_dynamic_wrapper" >/dev/null 2>&1 || true
      [ -n "$libctest_work_dir" ] && "$BB" rm -rf "$libctest_work_dir" >/dev/null 2>&1 || true
      cd "$old_pwd" || true
      echo "[TEST] Exit code($ret): $test_sh"
      echo
      return "$ret"
    }
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: patched script ready $patched_script"
  test_locale=$(runtime_locale_for_test "$test_name")
  test_locpath="$RUNTIME_LOCPATH"
  [ "$test_locale" = "C" ] && test_locpath=""
  HOME=/root
  PATH=$RUNTIME_PATH
  LD_LIBRARY_PATH=$(runtime_ld_library_path_for_test "$test_sh")
  LANG=$test_locale
  LC_ALL=$test_locale
  LC_CTYPE=$test_locale
  LOCPATH=$test_locpath
  export HOME PATH LD_LIBRARY_PATH LANG LC_ALL LC_CTYPE LOCPATH
  for cmd in sh bash basename dirname rm mv grep cat mkdir cp ln chmod sed awk mount umount sync ls pwd cut tr sort uniq head tail wc xargs printf sleep kill ps env touch dd readlink find which stat free df cal clear date od hexdump md5sum more rmdir false true expr uname uptime hwclock ar locale timeout; do
    eval "$cmd() { \"\$BB\" $cmd \"\$@\"; }"
  done
  [ "$test_name" = "lmbench_testcode.sh" ] && echo "[TEST][lmbench] stage: run patched script begin"
  . "$patched_script"
  ret=$?
  "$BB" rm -f "$patched_script" >/dev/null 2>&1 || true
  [ -n "$libctest_dynamic_temp" ] && "$BB" rm -f "$libctest_dynamic_temp" >/dev/null 2>&1 || true
  [ -n "$libctest_dynamic_wrapper" ] && "$BB" rm -f "$libctest_dynamic_wrapper" >/dev/null 2>&1 || true
  [ -n "$libctest_work_dir" ] && "$BB" rm -rf "$libctest_work_dir" >/dev/null 2>&1 || true
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
  start_line=$(group_marker_line START "$test_sh" "$suite")
  end_line=$(group_marker_line END "$test_sh" "$suite")
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
