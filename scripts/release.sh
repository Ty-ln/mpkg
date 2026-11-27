#!/usr/bin/env bash
# Baut das Release-Archiv für install.sh:
#   dist/mpkg-macos-universal.tar.gz
#   dist/mpkg-macos-universal.tar.gz.sha256
set -euo pipefail
cd "$(dirname "$0")/.."

./scripts/build-universal.sh

ASSET="mpkg-macos-universal.tar.gz"
mkdir -p dist
tar -czf "dist/$ASSET" -C target/universal mpkg
(cd dist && shasum -a 256 "$ASSET" > "$ASSET.sha256")

VERSION="v$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
echo
echo "Release $VERSION gebaut:"
ls -lh dist/
echo
echo "Veröffentlichen (GitHub CLI):"
echo "  gh release create $VERSION dist/$ASSET dist/$ASSET.sha256 --title \"mpkg $VERSION\""
