#!/usr/bin/env bash
# Runs proxycli with the given arguments and fails if its output differs from the expected value.
# Usage: expect.sh <expected> <proxycli args...>
set -u
expected=$1
shift
actual=$("$PROXYCLI" "$@")
if [ "$actual" != "$expected" ]; then
  echo "proxycli $* returned '$actual', expected '$expected'"
  exit 1
fi
echo "proxycli $* returned '$actual'"
