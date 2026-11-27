# newProject – apt-artiger Paketmanager für macOS (Scoping)

**Arbeitstitel:** `mpkg`
**Stand:** 26.11.2026
**Status:** Idee / Scoping

## Worum es geht

Seit September 2026 baut Homebrew keine Binärpakete mehr für Intel-Macs, und ab September 2027 läuft Homebrew dort gar nicht mehr. Besitzer von Intel-MacBooks haben dann keinen einfachen, verlässlichen Weg mehr, Software über die Kommandozeile zu installieren.

`mpkg` soll das lösen:

1. **Auf Intel-Macs zuverlässig und einfach funktionieren**: ein Installer, keine Bastelei.
2. **Die Bedienung von apt & Co. auf den Mac bringen**: `mpkg install`, `mpkg search`, `mpkg upgrade`.
3. **Mehrere Backends nutzen**: native macOS-Pakete (MacPorts), bei Bedarf als Fallback eine leichte Debian-VM.

## Dokumente

| Datei | Inhalt |
|---|---|
| [01-problem-und-zielgruppe.md](01-problem-und-zielgruppe.md) | Problem, Motivation, Zielgruppe, Zeitfenster |
| [02-bestehende-loesungen.md](02-bestehende-loesungen.md) | Marktüberblick: Homebrew, MacPorts, Fink, Nix, VM-Lösungen |
| [03-architektur.md](03-architektur.md) | Technischer Ansatz, Komponenten, Backend-Auswahl |
| [04-mvp-und-roadmap.md](04-mvp-und-roadmap.md) | MVP-Umfang, Phasen, Nicht-Ziele |
| [05-risiken-und-offene-fragen.md](05-risiken-und-offene-fragen.md) | Risiken, Gegenmaßnahmen, offene Entscheidungen |
| [06-messergebnisse.md](06-messergebnisse.md) | Gemessene MacPorts-Abdeckung (arm64, Intel Tahoe, Intel Big Sur) und Folgen |

## Code

Kern in **Rust**, VM-Helfer in **Swift** (Entscheidungen in `05-risiken-und-offene-fragen.md`).

```
Cargo.toml
src/
  main.rs            Einstieg, Befehle verteilen
  cli.rs             clap-Befehle (install, remove, search, …)
  resolver.rs        Backend-Wahl + Name-Mapping
  mapping.rs         Mapping-DB laden (eingebaut + ~/.mpkg/mapping.toml)
  doctor.rs          Systemprüfung
  migrate.rs         migrate-from-brew, migrate-macports
  vm.rs              Debian-VM: anlegen, starten, ssh-Zugriff (`mpkg vm …`)
  backend/
    mod.rs           Trait `Backend`, Hilfsfunktionen
    macports.rs      MacPorts-Backend (MVP)
    linuxvm.rs       VM-Backend: apt im Gast + Shims in ~/.mpkg/bin
mapping/packages.toml  Mapping-DB v1
helpers/mpkg-vm/     Swift-Helfer: betreibt die VM über Virtualization.framework
scripts/build-universal.sh  Universal Binary Intel + Apple Silicon
```

### Installation (für Nutzer)

```sh
curl -fsSL https://raw.githubusercontent.com/Ty-ln/mpkg/main/install.sh | sh
```

Das lädt die Universal Binary (Prüfsumme wird geprüft) nach `~/.mpkg/bin`, trägt den PATH ein und startet `mpkg setup`, das MacPorts einrichtet. Deinstallation: `./install.sh --uninstall`.

### VM-Fallback (ab macOS 11; Home-Freigabe ab macOS 12)

Pakete, die es in MacPorts nicht gibt, installiert mpkg aus Debian in einer leichten VM:

```sh
mpkg vm create            # einmalig: Debian 13 laden (~400 MB, SHA512-geprüft)
mpkg install hollywood    # nicht in MacPorts → automatisch über die VM
mpkg install --backend linuxvm ripgrep   # VM erzwingen
mpkg vm shell | stop | status
```

Die Programme landen als Shims in `~/.mpkg/bin` und laufen im aktuellen Verzeichnis: Das Home-Verzeichnis ist im Gast am selben Pfad eingehängt. Die VM startet beim ersten Aufruf (beim ersten Mal ~40 s, danach ~8 s) und fährt nach 10 Minuten ohne Nutzung wieder herunter.

### Release bauen

```sh
./scripts/release.sh                                  # → dist/mpkg-macos-universal.tar.gz + .sha256
./install.sh --from target/universal/mpkg --no-setup  # lokal testen
```

### Entwickeln

```sh
cargo build
cargo run -- --help
cargo run -- doctor
cargo run -- --dry-run install python3 ripgrep
cargo run -- --dry-run migrate-from-brew
```

## Kernidee in einem Bild

```
                ┌──────────────────────────────┐
  Nutzer  ───▶  │  mpkg CLI (apt-artige Befehle)│
                └──────────────┬───────────────┘
                               │ Resolver: Name-Mapping + Backend-Wahl
              ┌────────────────┼─────────────────┐
              ▼                ▼                 ▼
     MacPorts-Binaries   Homebrew (optional,   Debian-amd64/arm64-VM
     (nativ, Intel +     nur Apple Silicon)    (Fallback, CLI-Tools
      Apple Silicon)                            über Shims im PATH)
```
