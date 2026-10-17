#!/usr/bin/env bash
# Runs proxycli with the given arguments and fails if its output differs from the expected value.
# When PROXYCLI_C is set, the C proxycli built against the C API is checked as well.
# Usage: expect.sh <expected> <proxycli args...>
set -u
expected=$1
shift
status=0
for proxycli in "$PROXYCLI" ${PROXYCLI_C:+"$PROXYCLI_C"}; do
  name=$(basename "$proxycli")
  actual=$("$proxycli" "$@")
  if [ "$actual" != "$expected" ]; then
    echo "$name $* returned '$actual', expected '$expected'"
    status=1
  else
    echo "$name $* returned '$actual'"
  fi
done
exit $status
