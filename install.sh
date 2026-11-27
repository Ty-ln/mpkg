#!/bin/sh
# mpkg-Installer
#
# Verwendung:
#   curl -fsSL https://raw.githubusercontent.com/Ty-ln/mpkg/main/install.sh | sh
#   curl -fsSL …/install.sh | sh -s -- --no-setup     # ohne anschließendes `mpkg setup`
#   ./install.sh --from target/universal/mpkg          # lokale Binary installieren (Test)
#   ./install.sh --uninstall
#
# Umgebungsvariablen:
#   MPKG_REPO     GitHub-Repo (Standard: siehe REPO unten)
#   MPKG_VERSION  Release-Tag, z. B. v0.1.0 (Standard: latest)
#   MPKG_HOME     Installationsort (Standard: ~/.mpkg)
#
# POSIX-sh, damit `curl … | sh` auch mit der alten macOS-bash/zsh funktioniert.
set -eu

REPO="${MPKG_REPO:-Ty-ln/mpkg}"
VERSION="${MPKG_VERSION:-latest}"
PREFIX="${MPKG_HOME:-$HOME/.mpkg}"
BIN_DIR="$PREFIX/bin"
ASSET="mpkg-macos-universal.tar.gz"

say() { printf '%s\n' "$*"; }
die() { printf 'Fehler: %s\n' "$*" >&2; exit 1; }

profile_file() {
  case "${SHELL:-}" in
    */bash) echo "$HOME/.bash_profile" ;;
    *) echo "$HOME/.zprofile" ;;
  esac
}

check_system() {
  [ "$(uname -s)" = "Darwin" ] || die "mpkg läuft nur auf macOS."
  version=$(sw_vers -productVersion)
  major=$(echo "$version" | cut -d. -f1)
  [ "$major" -ge 11 ] || die "macOS $version wird nicht unterstützt (mindestens macOS 11 Big Sur)."
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
  # Per Browser geladene Dateien tragen ein Quarantäne-Flag; curl setzt keins.
  xattr -d com.apple.quarantine "$BIN_DIR/mpkg" 2>/dev/null || true
  say "✓ mpkg installiert: $BIN_DIR/mpkg"
}

install_from_release() {
  case "$REPO" in
    OWNER/*) die "Noch kein öffentliches Release. Zum Testen: ./install.sh --from target/universal/mpkg" ;;
  esac
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT

  url=$(download_url "$ASSET")
  say "Lade $url …"
  curl -fsSL "$url" -o "$tmp/$ASSET" || die "Download fehlgeschlagen."
  curl -fsSL "$url.sha256" -o "$tmp/$ASSET.sha256" || die "Prüfsumme nicht abrufbar."

  expected=$(awk '{print $1}' "$tmp/$ASSET.sha256")
  actual=$(shasum -a 256 "$tmp/$ASSET" | awk '{print $1}')
  [ "$expected" = "$actual" ] || die "Prüfsumme stimmt nicht – Download beschädigt oder manipuliert."
  say "✓ Prüfsumme korrekt"

  tar -xzf "$tmp/$ASSET" -C "$tmp"
  install_binary "$tmp/mpkg"
  # Swift-Helfer für das VM-Backend (ab Release 0.2.0 im Archiv).
  if [ -f "$tmp/mpkg-vm" ]; then
    cp "$tmp/mpkg-vm" "$BIN_DIR/mpkg-vm"
    chmod 755 "$BIN_DIR/mpkg-vm"
    say "✓ VM-Helfer installiert: $BIN_DIR/mpkg-vm"
  fi
}

ensure_path() {
  profile=$(profile_file)
  if [ -f "$profile" ] && grep -q "$BIN_DIR" "$profile"; then
    say "✓ PATH-Eintrag in $profile vorhanden"
  else
    # $PATH bleibt bewusst unexpandiert in der Datei stehen.
    printf '\n# mpkg\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$profile"
    say "✓ PATH-Eintrag in $profile ergänzt"
  fi
}

run_setup() {
  say ""
  say "Richte MacPorts ein (mpkg setup) …"
  # Bei `curl | sh` ist stdin die Pipe – Rückfragen und sudo brauchen das Terminal.
  if [ -r /dev/tty ] && [ -w /dev/tty ]; then
    "$BIN_DIR/mpkg" setup < /dev/tty
  else
    say "Kein Terminal verfügbar – bitte danach selbst ausführen: mpkg setup"
  fi
}

uninstall() {
  rm -f "$BIN_DIR/mpkg" "$BIN_DIR/mpkg-vm"
  say "✓ mpkg entfernt."
  say "  MacPorts, installierte Pakete und $PREFIX/state.json bleiben erhalten."
  say "  Den PATH-Eintrag ggf. aus $(profile_file) entfernen."
}

main() {
  from=""
  setup=1
  while [ $# -gt 0 ]; do
    case "$1" in
      --from) [ $# -ge 2 ] || die "--from braucht einen Pfad"; from="$2"; shift 2 ;;
      --no-setup) setup=0; shift ;;
      --uninstall) uninstall; exit 0 ;;
      -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
      *) die "Unbekannte Option: $1" ;;
    esac
  done
  [ "${MPKG_NO_SETUP:-0}" = 1 ] && setup=0

  check_system
  if [ -n "$from" ]; then
    [ -f "$from" ] || die "$from nicht gefunden"
    install_binary "$from"
  else
    install_from_release
  fi
  ensure_path
  [ "$setup" = 1 ] && run_setup

  say ""
  say "Fertig. Neues Terminal öffnen (oder: source $(profile_file)), dann: mpkg doctor"
}

main "$@"
