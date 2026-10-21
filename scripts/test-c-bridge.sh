#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --locked --no-default-features --lib
mkdir -p target/c-bridge
# System libraries required by Rust's standard library on Linux.
cc -std=c11 -Wall -Wextra -Werror -Iinclude tests/bridge.c target/debug/libwayx.a \
  -ldl -lpthread -lm -lrt -lutil -o target/c-bridge/test
./target/c-bridge/test
printf 'C ABI layout, callbacks, framebuffer bounds and clipboard tests passed.\n'
