#!/bin/sh
# mpkg installer
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Ty-ln/mpkg/main/install.sh | sh
#   curl -fsSL …/install.sh | sh -s -- --no-setup     # skip the final `mpkg setup`
#   ./install.sh --from target/universal/mpkg          # install a local binary (testing)
#   ./install.sh --uninstall
#
# Environment variables:
#   MPKG_REPO     GitHub repo (default: see REPO below)
#   MPKG_VERSION  release tag, e.g. v0.1.0 (default: latest)
#   MPKG_HOME     install location (default: ~/.mpkg)
#
# POSIX sh, so that `curl … | sh` also works with the old macOS bash/zsh.
set -eu

REPO="${MPKG_REPO:-Ty-ln/mpkg}"
VERSION="${MPKG_VERSION:-latest}"
PREFIX="${MPKG_HOME:-$HOME/.mpkg}"
BIN_DIR="$PREFIX/bin"
ASSET="mpkg-macos-universal.tar.gz"

say() { printf '%s\n' "$*"; }
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }

profile_file() {
  case "${SHELL:-}" in
    */bash) echo "$HOME/.bash_profile" ;;
    *) echo "$HOME/.zprofile" ;;
  esac
}

check_system() {
  [ "$(uname -s)" = "Darwin" ] || die "mpkg only runs on macOS."
  version=$(sw_vers -productVersion)
  major=$(echo "$version" | cut -d. -f1)
  [ "$major" -ge 11 ] || die "macOS $version is not supported (macOS 11 Big Sur or later required)."
  say "macOS $version ($(uname -m))"
}

download_url() {
  if [ "$VERSION" = "latest" ]; then
    echo "https://github.com/$REPO/releases/latest/download/$1"
  else
    echo "https://github.com/$REPO/releases/download/$VERSION/$1"
  fi
}

install_binary() {
  mkdir -p "$BIN_DIR"
  cp "$1" "$BIN_DIR/mpkg"
  chmod 755 "$BIN_DIR/mpkg"
  # Files downloaded with a browser carry a quarantine flag; curl does not set one.
  xattr -d com.apple.quarantine "$BIN_DIR/mpkg" 2>/dev/null || true
  say "✓ mpkg installed: $BIN_DIR/mpkg"
}

install_from_release() {
  case "$REPO" in
    OWNER/*) die "No public release yet. For testing: ./install.sh --from target/universal/mpkg" ;;
  esac
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT

  url=$(download_url "$ASSET")
  say "Downloading $url …"
  curl -fsSL "$url" -o "$tmp/$ASSET" || die "Download failed."
  curl -fsSL "$url.sha256" -o "$tmp/$ASSET.sha256" || die "Could not fetch the checksum."

  expected=$(awk '{print $1}' "$tmp/$ASSET.sha256")
  actual=$(shasum -a 256 "$tmp/$ASSET" | awk '{print $1}')
  [ "$expected" = "$actual" ] || die "Checksum mismatch – download corrupted or tampered with."
  say "✓ Checksum verified"

  tar -xzf "$tmp/$ASSET" -C "$tmp"
  install_binary "$tmp/mpkg"
  # Swift helper for the VM backend (in the archive since release 0.2.0).
  if [ -f "$tmp/mpkg-vm" ]; then
    cp "$tmp/mpkg-vm" "$BIN_DIR/mpkg-vm"
    chmod 755 "$BIN_DIR/mpkg-vm"
    say "✓ VM helper installed: $BIN_DIR/mpkg-vm"
  fi
}

ensure_path() {
  profile=$(profile_file)
  if [ -f "$profile" ] && grep -q "$BIN_DIR" "$profile"; then
    say "✓ PATH entry already in $profile"
  else
    # $PATH deliberately stays unexpanded in the file.
    printf '\n# mpkg\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$profile"
    say "✓ PATH entry added to $profile"
  fi
}

run_setup() {
  say ""
  say "Setting up the package source (mpkg setup: Homebrew on Apple Silicon, MacPorts on Intel) …"
  # With `curl | sh`, stdin is the pipe – prompts and sudo need the terminal.
  if [ -r /dev/tty ] && [ -w /dev/tty ]; then
    "$BIN_DIR/mpkg" setup < /dev/tty
  else
    say "No terminal available – please run this yourself afterwards: mpkg setup"
  fi
}

uninstall() {
  rm -f "$BIN_DIR/mpkg" "$BIN_DIR/mpkg-vm"
  say "✓ mpkg removed."
  say "  Homebrew/MacPorts, installed packages and $PREFIX/state.json are kept."
  say "  Remove the PATH entry from $(profile_file) if you no longer need it."
}

main() {
  from=""
  setup=1
  while [ $# -gt 0 ]; do
    case "$1" in
      --from) [ $# -ge 2 ] || die "--from needs a path"; from="$2"; shift 2 ;;
      --no-setup) setup=0; shift ;;
      --uninstall) uninstall; exit 0 ;;
      -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
      *) die "Unknown option: $1" ;;
    esac
  done
  [ "${MPKG_NO_SETUP:-0}" = 1 ] && setup=0

  check_system
  if [ -n "$from" ]; then
    [ -f "$from" ] || die "$from not found"
    install_binary "$from"
  else
    install_from_release
  fi
  ensure_path
  [ "$setup" = 1 ] && run_setup

  say ""
  say "Done. Open a new terminal (or: source $(profile_file)), then run: mpkg doctor"
}

main "$@"
