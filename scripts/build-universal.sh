#!/usr/bin/env bash
# Baut eine Universal Binary (Intel + Apple Silicon) nach target/universal/mpkg
set -euo pipefail
cd "$(dirname "$0")/.."

rustup target add x86_64-apple-darwin aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin

mkdir -p target/universal
lipo -create -output target/universal/mpkg \
  target/x86_64-apple-darwin/release/mpkg \
  target/aarch64-apple-darwin/release/mpkg

# Ad-hoc-Signatur: Apple Silicon führt unsignierte arm64-Binaries nicht aus.
# (Für die Verteilung ohne Warnungen später: Developer-ID-Signatur + Notarisierung.)
codesign --sign - --force target/universal/mpkg

lipo -info target/universal/mpkg
