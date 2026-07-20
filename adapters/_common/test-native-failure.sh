#!/bin/sh
set -eu
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
OUT="${TMPDIR:-/tmp}/emucap-native-failure-test-$$"
trap 'rm -f "$OUT"' EXIT INT TERM
"${CXX:-c++}" -std=c++11 -Wall -Wextra -Werror \
  "$HERE/emucap_native_failure.cpp" "$HERE/emucap_native_failure_test.cpp" -o "$OUT"
"$OUT"
