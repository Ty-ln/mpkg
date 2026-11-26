#!/usr/bin/env bash
# Baut eine Universal Binary (Intel + Apple Silicon) nach target/universal/mpkg
set -euo pipefail

rustup target add x86_64-apple-darwin aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin

mkdir -p target/universal
lipo -create -output target/universal/mpkg \
  target/x86_64-apple-darwin/release/mpkg \
  target/aarch64-apple-darwin/release/mpkg

lipo -info target/universal/mpkg
