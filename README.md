# mpkg – Paketmanager für macOS

**Stand:** 27.11.2026
**Status:** MVP und VM-Fallback umgesetzt (v0.3.1), Tests auf Intel-Hardware stehen aus

## Worum es geht

Seit September 2026 baut Homebrew keine Binärpakete mehr für Intel-Macs, und ab September 2027 läuft Homebrew dort gar nicht mehr. Besitzer von Intel-MacBooks haben dann keinen einfachen, verlässlichen Weg mehr, Software über die Kommandozeile zu installieren.

`mpkg` soll das lösen:

1. **Auf Intel-Macs zuverlässig und einfach funktionieren**: ein Installer, keine Bastelei.
2. **Einfache, gewohnte Befehle bieten**: `mpkg install`, `mpkg search`, `mpkg upgrade`.
3. **Mehrere Backends nutzen**: native macOS-Pakete (Homebrew auf Apple Silicon, MacPorts auf Intel), bei Bedarf als Fallback eine leichte Debian-VM.

## Code

Kern in **Rust**, VM-Helfer in **Swift**.

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
    homebrew.rs      Homebrew-Backends für Pakete und Apps/Casks (nur Apple Silicon)
    macports.rs      MacPorts-Backend
    linuxvm.rs       VM-Backend: Debian-Pakete im Gast + Shims in ~/.mpkg/bin
mapping/packages.toml  Mapping-DB v1
helpers/mpkg-vm/     Swift-Helfer: betreibt die VM über Virtualization.framework
scripts/build-universal.sh  Universal Binary Intel + Apple Silicon
```

### Installation (für Nutzer)

```sh
curl -fsSL https://raw.githubusercontent.com/Ty-ln/mpkg/main/install.sh | sh
```

Das lädt die Universal Binary (Prüfsumme wird geprüft) nach `~/.mpkg/bin`, trägt den PATH ein und startet `mpkg setup`. Das richtet auf Apple Silicon Homebrew ein (offizielles, signiertes `Homebrew.pkg`; ein vorhandenes Homebrew wird einfach genutzt) und auf Intel MacPorts. Deinstallation: `./install.sh --uninstall`.

### Welche Paketquelle wird benutzt?

| Mac | Reihenfolge |
|---|---|
| Apple Silicon | Homebrew → MacPorts (falls installiert) → Debian-VM |
| Intel | MacPorts → Debian-VM (Homebrew baut für Intel keine Pakete mehr) |

Die Befehle sind überall gleich: `mpkg install ripgrep` ruft auf Apple Silicon im Hintergrund `brew install` auf, auf Intel `port install`. Mit `--backend homebrew|macports|linuxvm` lässt sich die Quelle erzwingen.

### Apps (nur Apple Silicon)

Mac-Apps mit Oberfläche kommen als Homebrew-Casks:

```sh
mpkg install --cask firefox   # App nach /Applications
mpkg remove firefox           # mpkg weiß, dass es eine App ist
mpkg search firefox           # zeigt Pakete [homebrew] und Apps [homebrew-cask]
mpkg upgrade                  # aktualisiert Pakete und Apps
```

`--cask` muss man angeben, weil manche Namen beides sind (`docker` ist als Paket die Kommandozeile, als App Docker Desktop). Fehlt ein Paket, gibt es aber eine gleichnamige App, schlägt mpkg `--cask` vor. Auf Intel gibt es keine Apps über mpkg: Dort nutzt mpkg Homebrew nicht. Passt eine App nicht zur installierten macOS-Version (zu alt oder nur für bestimmte Versionen gebaut), bricht mpkg vor dem Download mit einer klaren Meldung ab.

### Von Homebrew umsteigen

`mpkg migrate-from-brew` übernimmt auf Apple Silicon alle vorhandenen Homebrew-Pakete und -Apps in die mpkg-Liste, ohne etwas neu zu installieren. Danach kennen `mpkg list` und `mpkg remove` sie. Auf Intel installiert der Befehl die Pakete über MacPorts neu und listet Apps getrennt auf: Für sie gibt es dort keinen Ersatz über mpkg.

### VM-Fallback (ab macOS 12)

Pakete, die es nativ (Homebrew/MacPorts) nicht gibt, installiert mpkg aus Debian in einer leichten VM:

```sh
mpkg vm create            # einmalig: Debian 13 laden (~400 MB, SHA512-geprüft)
mpkg install hollywood    # nativ nicht verfügbar → automatisch über die VM
mpkg install --backend linuxvm ripgrep   # VM erzwingen
mpkg vm shell | stop | status
```

Die Programme landen als Shims in `~/.mpkg/bin` und laufen im aktuellen Verzeichnis: Das Home-Verzeichnis ist im Gast am selben Pfad eingehängt.

**Start und Stopp passieren automatisch.** Von Hand nötig ist nur einmal `mpkg vm create`. Danach startet mpkg die VM selbst, sobald sie gebraucht wird: beim Aufruf eines Programms aus der VM (z. B. `hollywood`), bei `mpkg install`/`remove` über die VM und bei `mpkg update`/`upgrade`. Der erste Start nach dem Anlegen dauert ~40 s, spätere ~8 s. Nach 10 Minuten ohne Nutzung fährt die VM von selbst herunter, aber nie, solange noch ein Programm darin läuft. `mpkg vm start` und `mpkg vm stop` braucht man nur, um das von Hand vorzuziehen.

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
  Nutzer  ───▶  │  mpkg CLI                    │
                └──────────────┬───────────────┘
                               │ Resolver: Name-Mapping + Backend-Wahl
              ┌────────────────┼─────────────────┐
              ▼                ▼                 ▼
     Homebrew-Bottles    MacPorts-Binaries    Debian-amd64/arm64-VM
     (Apple Silicon,     (Intel; auf Apple    (Fallback, CLI-Tools
      Vorrang)            Silicon optional)    über Shims im PATH)
```
