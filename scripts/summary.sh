#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

INPUT_RV="$ROOT_DIR/docs/output_rv.txt"
INPUT_LA="$ROOT_DIR/docs/output_la.txt"
OUTPUT_RV="$ROOT_DIR/docs/summary_rv.txt"
OUTPUT_LA="$ROOT_DIR/docs/summary_la.txt"

for f in "$INPUT_RV" "$INPUT_LA"; do
  if [ ! -f "$f" ]; then
    echo "error: file not found: $f" >&2
    exit 1
  fi
done

extract_summary() {
  local input_file="$1"
  local output_file="$2"
  local tab
  tab="$(printf '\t')"

  awk '
function reset_case() {
  in_case = 0
  in_summary = 0
  case_name = ""
  has_summary = 0
  passed = failed = broken = skipped = warnings = 0
}

function start_case(name) {
  in_case = 1
  in_summary = 0
  case_name = name
  has_summary = 0
  passed = failed = broken = skipped = warnings = 0
}

function print_case_record() {
  case_seq += 1
  printf "%d\t%d\t%s\t%d\t%d\t%d\t%d\t%d\n", failed, case_seq, case_name, passed, failed, broken, skipped, warnings
}

function end_case() {
  if (!in_case) return
  if (has_summary) {
    print_case_record()
  }
  reset_case()
}

BEGIN {
  reset_case()
  case_seq = 0
}

{
  line = $0
  sub(/\r$/, "", line)

  if (line ~ /^RUN LTP CASE[[:space:]]+/) {
    end_case()
    case_name_tmp = line
    sub(/^RUN LTP CASE[[:space:]]+/, "", case_name_tmp)
    start_case(case_name_tmp)
    next
  }

  if (!in_case) next

  if (line ~ /^Summary:?[[:space:]]*$/) {
    in_summary = 1
    has_summary = 1
    next
  }

  if (in_summary && line ~ /^(passed|failed|broken|skipped|warnings)[[:space:]]+[0-9]+/) {
    key = line
    sub(/[[:space:]].*$/, "", key)

    val_str = line
    sub(/^[^[:space:]]+[[:space:]]+/, "", val_str)
    sub(/[^0-9].*$/, "", val_str)
    val = val_str + 0

    if (key == "passed") passed = val
    else if (key == "failed") failed = val
    else if (key == "broken") broken = val
    else if (key == "skipped") skipped = val
    else if (key == "warnings") warnings = val
    next
  }

  if (line ~ /^FAIL LTP CASE[[:space:]]+.+:[[:space:]]+[0-9]+[[:space:]]*$/) {
    end_case()
    next
  }
}

END {
  end_case()
}
' "$input_file" \
    | LC_ALL=C sort -t "$tab" -k1,1nr -k2,2n \
    | awk -F "$tab" '
BEGIN {
  total_passed = total_failed = total_broken = total_skipped = total_warnings = 0
}

{
  case_name = $3
  passed = $4 + 0
  failed = $5 + 0
  broken = $6 + 0
  skipped = $7 + 0
  warnings = $8 + 0

  print "RUN LTP CASE " case_name
  print "Summary"
  print "passed   " passed
  print "failed   " failed
  print "broken   " broken
  print "skipped  " skipped
  print "warnings " warnings
  print "FAIL LTP CASE " case_name " : 0"
  print ""

  total_passed += passed
  total_failed += failed
  total_broken += broken
  total_skipped += skipped
  total_warnings += warnings
}

END {
  print "Summary"
  print "passed   " total_passed
  print "failed   " total_failed
  print "broken   " total_broken
  print "skipped  " total_skipped
  print "warnings " total_warnings
}
' > "$output_file"
}

extract_summary "$INPUT_RV" "$OUTPUT_RV"
extract_summary "$INPUT_LA" "$OUTPUT_LA"

echo "generated: $OUTPUT_RV"
echo "generated: $OUTPUT_LA"
