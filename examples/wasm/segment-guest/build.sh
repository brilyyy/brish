#!/bin/sh
# Build the example guest and install it into the sibling plugin dir.
#
#   ./build.sh
#   brish plugin add ../../plugins/wasm-status   # then restart
#
# Needs the wasm target:
#   rustup target add wasm32-unknown-unknown
set -eu

cd "$(dirname "$0")"

if ! rustup target list --installed | grep -qx wasm32-unknown-unknown; then
    echo "build.sh: missing wasm32-unknown-unknown target" >&2
    echo "hint: rustup target add wasm32-unknown-unknown" >&2
    exit 1
fi

cargo build --release --target wasm32-unknown-unknown

out=../../plugins/wasm-status/plugin.wasm
cp target/wasm32-unknown-unknown/release/brish_wasm_example_guest.wasm "$out"
echo "build.sh: installed $out ($(wc -c < "$out" | tr -d ' ') bytes)"