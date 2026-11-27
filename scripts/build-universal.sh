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

# Swift-Helfer für das VM-Backend, ebenfalls universal; braucht das Virtualization-Entitlement.
SWIFT_ARGS=(-c release --arch arm64 --arch x86_64 --package-path helpers/mpkg-vm)
swift build "${SWIFT_ARGS[@]}"
# Ausgabepfad hängt von der Toolchain-Version ab – daher erfragen.
cp "$(swift build "${SWIFT_ARGS[@]}" --show-bin-path)/mpkg-vm" target/universal/mpkg-vm
codesign --sign - --entitlements helpers/mpkg-vm/mpkg-vm.entitlements --force target/universal/mpkg-vm
lipo -info target/universal/mpkg-vm
