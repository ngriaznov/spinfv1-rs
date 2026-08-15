#!/bin/sh
# C-side FFI safety battery: build the static library, then run the
# hostile-host harness under AddressSanitizer + UndefinedBehaviorSanitizer
# (with leak detection), and again under valgrind when it is installed.
#
# Usage: tests/c/run.sh   (from the repository root; needs cc)
set -eu

cd "$(dirname "$0")/../.."
BUILD_DIR="target/ffi-safety"
mkdir -p "$BUILD_DIR"

echo "== building static library (release, ffi feature) =="
cargo rustc --release --features ffi --crate-type staticlib

CC="${CC:-cc}"
COMMON="-O1 -g -Wall -Wextra -Werror -I include tests/c/ffi_safety.c \
        target/release/libspinfv1.a -lm -lpthread -ldl"

echo "== ASan + UBSan + LeakSanitizer =="
# shellcheck disable=SC2086
$CC -fsanitize=address,undefined -fno-sanitize-recover=all \
    -o "$BUILD_DIR/ffi_safety_asan" $COMMON
ASAN_OPTIONS=detect_leaks=1 "$BUILD_DIR/ffi_safety_asan"

if command -v valgrind >/dev/null 2>&1; then
    echo "== valgrind (memcheck, full leak check) =="
    # shellcheck disable=SC2086
    $CC -o "$BUILD_DIR/ffi_safety_plain" $COMMON
    valgrind --quiet --error-exitcode=1 --leak-check=full \
        --errors-for-leak-kinds=definite,possible \
        "$BUILD_DIR/ffi_safety_plain"
else
    echo "== valgrind not installed; skipped =="
fi

echo "ffi safety battery: PASS"
