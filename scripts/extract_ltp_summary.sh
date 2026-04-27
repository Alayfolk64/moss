#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -eq 0 ]; then
  files=("output_rv.txt" "output_la.txt")
else
  files=("$@")
fi

for f in "${files[@]}"; do
  if [ ! -f "$f" ]; then
    echo "error: file not found: $f" >&2
    exit 1
  fi
done

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

function print_case_and_accumulate() {
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
  case_count++
}

function end_case() {
  if (!in_case) return

  if (has_summary) {
    print_case_and_accumulate()
  } else {
    missing_summary_count++
  }
  reset_case()
}

BEGIN {
  reset_case()
  total_passed = total_failed = total_broken = total_skipped = total_warnings = 0
  case_count = 0
  missing_summary_count = 0
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

  if (in_summary) {
    if (line ~ /^(passed|failed|broken|skipped|warnings)[[:space:]]+[0-9]+/) {
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
  }

  if (line ~ /^FAIL LTP CASE[[:space:]]+.+:[[:space:]]+[0-9]+[[:space:]]*$/) {
    end_case()
    next
  }
}

END {
  end_case()

  print "Summary"
  print "passed   " total_passed
  print "failed   " total_failed
  print "broken   " total_broken
  print "skipped  " total_skipped
  print "warnings " total_warnings

  if (missing_summary_count > 0) {
    print ""
    print "missing_summary_cases " missing_summary_count
  }
}
' "${files[@]}"
