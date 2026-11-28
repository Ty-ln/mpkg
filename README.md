# mpkg – package manager for macOS

**As of:** 2026-11-28
**Status:** MVP, VM fallback, Homebrew backend and apps done (v0.7.0); tests on Intel hardware are still pending

## What it is about

Since September 2026 Homebrew no longer builds binary packages for Intel Macs, and from September 2027 Homebrew will not run there at all. Owners of Intel MacBooks will then have no simple, reliable way to install software from the command line.

`mpkg` aims to solve that:

1. **Work reliably and simply on Intel Macs**: one installer, no tinkering.
2. **Offer simple, familiar commands**: `mpkg install`, `mpkg search`, `mpkg upgrade`.
3. **Use several backends**: native macOS packages (Homebrew on Apple Silicon, MacPorts on Intel), with a lightweight Debian VM as a fallback when needed.

## Code

Core in **Rust**, VM helper in **Swift**.

```
Cargo.toml
src/
  main.rs            entry point, dispatches commands
  cli.rs             clap commands (install, remove, search, …)
  resolver.rs        backend choice + name mapping
  mapping.rs         loads the mapping DB (built-in + ~/.mpkg/mapping.toml)
  doctor.rs          system check
  migrate.rs         migrate-from-brew, migrate-macports
  vm.rs              Debian VM: create, start, ssh access (`mpkg vm …`)
  backend/
    mod.rs           trait `Backend`, helpers
    homebrew.rs      Homebrew backends for packages and apps/casks (Apple Silicon only)
    macports.rs      MacPorts backend
    linuxvm.rs       VM backend: Debian packages in the guest + shims in ~/.mpkg/bin
mapping/packages.toml  mapping DB v1
helpers/mpkg-vm/     Swift helper: runs the VM via Virtualization.framework
scripts/build-universal.sh  universal binary Intel + Apple Silicon
```

### Installation (for users)

```sh
curl -fsSL https://raw.githubusercontent.com/Ty-ln/mpkg/main/install.sh | sh
```

This downloads the universal binary (checksum verified) to `~/.mpkg/bin`, adds it to PATH and runs `mpkg setup`. On Apple Silicon that sets up Homebrew (the official, signed `Homebrew.pkg`; an existing Homebrew is simply used), on Intel MacPorts. Uninstall: `./install.sh --uninstall`.

### Which package source is used?

| Mac | Order |
|---|---|
| Apple Silicon | Homebrew → MacPorts (if installed) → Debian VM |
| Intel | MacPorts → Debian VM (Homebrew no longer builds packages for Intel) |

The commands are the same everywhere: `mpkg install ripgrep` runs `brew install` in the background on Apple Silicon and `port install` on Intel. `--backend homebrew|macports|linuxvm` forces a source.

### Apps (Apple Silicon only)

Mac apps with a user interface come as Homebrew casks:

```sh
mpkg install --cask firefox   # app into /Applications
mpkg remove firefox           # mpkg knows it is an app
mpkg search firefox           # shows packages [homebrew] and apps [homebrew-cask]
mpkg upgrade                  # upgrades packages and apps
```

`--cask` is required so that mpkg never installs an app when you asked for a command-line package. If a package is missing but an app with that name exists, mpkg suggests `--cask`. There are no apps via mpkg on Intel, because mpkg does not use Homebrew there. If an app does not fit the installed macOS version (too old, or built only for specific versions), mpkg aborts with a clear message before downloading.

### Switching from Homebrew

On Apple Silicon, `mpkg migrate-from-brew` adds all existing Homebrew packages and apps to the mpkg list without reinstalling anything. After that, `mpkg list` and `mpkg remove` know them. On Intel the command reinstalls the packages via MacPorts and lists apps separately, since there is no replacement for them via mpkg.

### VM fallback (macOS 12 and later)

Packages that are not available natively (Homebrew/MacPorts) are installed by mpkg from Debian in a lightweight VM:

```sh
mpkg vm create            # once: download Debian 13 (~400 MB, SHA512-verified)
mpkg install hollywood    # not available natively → automatically via the VM
mpkg install --backend linuxvm ripgrep   # force the VM
mpkg vm shell | stop | status
```

The programs end up as shims in `~/.mpkg/bin` and run in the current directory: the home directory is mounted at the same path in the guest.

**Start and stop happen automatically.** The only manual step is running `mpkg vm create` once. After that mpkg starts the VM itself whenever it is needed: when you run a program from the VM (e.g. `hollywood`), on `mpkg install`/`remove` via the VM and on `mpkg update`/`upgrade`. The first boot after creation takes ~40 s, later ones ~8 s. After 10 minutes without use the VM shuts down by itself, but never while a program is still running in it. You only need `mpkg vm start` and `mpkg vm stop` to do this ahead of time by hand.

### Building a release

```sh
./scripts/release.sh                                  # → dist/mpkg-macos-universal.tar.gz + .sha256
./install.sh --from target/universal/mpkg --no-setup  # test locally
```

### Development

```sh
cargo build
cargo run -- --help
cargo run -- doctor
cargo run -- --dry-run install python3 ripgrep
cargo run -- --dry-run migrate-from-brew
```

## The core idea in one picture

```
                ┌──────────────────────────────┐
  User    ───▶  │  mpkg CLI                    │
                └──────────────┬───────────────┘
                               │ resolver: name mapping + backend choice
              ┌────────────────┼─────────────────┐
              ▼                ▼                 ▼
     Homebrew bottles    MacPorts binaries    Debian amd64/arm64 VM
     (Apple Silicon,     (Intel; optional     (fallback, CLI tools
      preferred)          on Apple Silicon)    via shims in PATH)
```
