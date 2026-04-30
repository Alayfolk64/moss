#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INIT="${1:-$ROOT/init.sh}"

if [[ ! -f "$INIT" ]]; then
  echo "missing init script: $INIT" >&2
  echo "usage: $0 [init.sh]" >&2
  exit 1
fi

for kernel in "$ROOT/kernel-rv" "$ROOT/kernel-la"; do
  [[ -f "$kernel" ]] || { echo "missing kernel: $kernel" >&2; exit 1; }
done

stamp="$(date +%Y%m%d-%H%M%S)"

for kernel in "$ROOT/kernel-rv" "$ROOT/kernel-la"; do
  backup="$kernel.bak.$stamp"
  tmp="$kernel.tmp.$stamp"
  cp -p "$kernel" "$backup"

  SRC_INIT="$INIT" KERNEL="$kernel" OUT="$tmp" perl -e '
    BEGIN {
      $src = $ENV{"SRC_INIT"};
      $kernel = $ENV{"KERNEL"};
      $out = $ENV{"OUT"};

      open my $fh, "<", $src or die "open $src: $!";
      binmode $fh;
      local $/;
      $new = <$fh>;
      close $fh;

      # Keep only active lines for injection. In init.sh, disabled LTP cases
      # and explanatory notes are comment lines, so dropping them makes the
      # embedded script smaller without changing which cases run.
      $new = join "", grep { $_ !~ /^[ \t]*#/ } split /(?<=\n)/, $new;

      $start = quotemeta("if ! /glibc/busybox test -d /bin; then\n");
      $end = quotemeta("run_ltp glibc\n");
      $patched = 0;
    }

    open my $in, "<", $kernel or die "open $kernel: $!";
    binmode $in;
    local $/;
    $_ = <$in>;
    close $in;

    s/($start.*?$end)/
      my $old = $1;
      my $room = length($old);
      my $need = length($new);
      die "new init is too large: $need bytes > embedded init $room bytes\n"
        if $need > $room;

      my $pad = $room - $need;
      my $replacement = $new;
      if ($pad == 1) {
        $replacement .= "\n";
      } elsif ($pad > 1) {
        $replacement .= "\n#" . (" " x ($pad - 2));
      }
      $patched++;
      $replacement;
    /se;

    die "embedded init script not found\n" unless $patched;

    open my $ofh, ">", $out or die "open $out: $!";
    binmode $ofh;
    print {$ofh} $_;
    close $ofh;
  '

  mv "$tmp" "$kernel"
  chmod --reference="$backup" "$kernel" 2>/dev/null || chmod 0755 "$kernel"

  echo "patched $kernel using $INIT"
  echo "backup:  $backup"
done
