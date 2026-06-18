#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

input="${1:-$ROOT_DIR/docs/summary_rv.txt}"
output="${2:-$ROOT_DIR/docs/total.txt}"

LC_ALL=C awk '
function reset_case() {
  in_case = 0
  in_summary = 0
  has_summary = 0
  case_name = ""
  case_passed = case_failed = case_broken = case_skipped = case_warnings = 0
}

function add_case() {
  if (!in_case || !has_summary) {
    reset_case()
    return
  }

  passed += case_passed
  failed += case_failed
  broken += case_broken
  skipped += case_skipped
  warnings += case_warnings
  reset_case()
}

BEGIN {
  reset_case()
}

{
  line = $0
  sub(/\r$/, "", line)
}

line ~ /^RUN LTP CASE[[:space:]]+/ {
  add_case()
  case_name = line
  sub(/^RUN LTP CASE[[:space:]]+/, "", case_name)
  in_case = 1
  next
}

!in_case {
  next
}

line ~ /^Summary:?[[:space:]]*$/ {
  in_summary = 1
  has_summary = 1
  next
}

in_summary && line ~ /^(passed|failed|broken|skipped|warnings)[[:space:]]+[0-9]+/ {
  key = line
  sub(/[[:space:]].*$/, "", key)

  val = line
  sub(/^[^[:space:]]+[[:space:]]+/, "", val)
  sub(/[^0-9].*$/, "", val)
  val += 0

  if (key == "passed") case_passed = val
  else if (key == "failed") case_failed = val
  else if (key == "broken") case_broken = val
  else if (key == "skipped") case_skipped = val
  else if (key == "warnings") case_warnings = val
  next
}

line ~ /^FAIL LTP CASE[[:space:]]+.+:[[:space:]]+[0-9]+[[:space:]]*$/ {
  add_case()
  next
}

END {
  add_case()
  print "passed   " passed
  print "failed   " failed
  print "broken   " broken
  print "skipped  " skipped
  print "warnings " warnings
}
' "$input" > "$output"

echo "generated: $output"
