#!/usr/bin/env bash
# Builds a universal binary (Intel + Apple Silicon) into target/universal/mpkg
set -euo pipefail
cd "$(dirname "$0")/.."

rustup target add x86_64-apple-darwin aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin

mkdir -p target/universal
lipo -create -output target/universal/mpkg \
  target/x86_64-apple-darwin/release/mpkg \
  target/aarch64-apple-darwin/release/mpkg

# Ad-hoc signature: Apple Silicon does not run unsigned arm64 binaries.
# (For distribution without warnings later: Developer ID signature + notarisation.)
codesign --sign - --force target/universal/mpkg

lipo -info target/universal/mpkg

# Swift helper for the VM backend, also universal; needs the virtualization entitlement.
SWIFT_ARGS=(-c release --arch arm64 --arch x86_64 --package-path helpers/mpkg-vm)
swift build "${SWIFT_ARGS[@]}"
# The output path depends on the toolchain version – so ask for it.
cp "$(swift build "${SWIFT_ARGS[@]}" --show-bin-path)/mpkg-vm" target/universal/mpkg-vm
codesign --sign - --entitlements helpers/mpkg-vm/mpkg-vm.entitlements --force target/universal/mpkg-vm
lipo -info target/universal/mpkg-vm
