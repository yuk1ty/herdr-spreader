#!/bin/sh
# Run Verus verification on the crate.
#
# Requires the cargo-verus binary from the Verus release
# (https://github.com/verus-lang/verus/releases) on PATH, or point
# VERUS_BIN_DIR at the extracted release directory.
set -eu

VERUS_BIN_DIR="${VERUS_BIN_DIR:-$HOME/tools/verus-arm64-macos}"
export PATH="$VERUS_BIN_DIR:$PATH"

exec cargo verus verify --features verus
