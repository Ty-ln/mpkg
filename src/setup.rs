//! `mpkg setup`: richtet das Haupt-Backend automatisch ein.
//!
//! Apple Silicon: Homebrew über den offiziellen, signierten `Homebrew.pkg`
//! (Gatekeeper-geprüft, kein `curl | bash`) und `brew shellenv` in der Shell-Konfiguration.
//!
//! Intel – Ablauf für MacPorts:
//!   1. Xcode Command Line Tools prüfen (sonst den Apple-Dialog öffnen)
//!   2. Neuesten MacPorts-Release über die GitHub-API ermitteln
//!   3. Passenden Installer für die macOS-Hauptversion wählen
//!      (Schema: MacPorts-<version>-<major>-<Name>.pkg)
//!   4. Laden, Gatekeeper/Notarisierung prüfen, mit `installer` installieren
//!   5. PATH in der Shell-Konfiguration eintragen (nach Rückfrage)
//!   6. `port selfupdate`

use crate::backend::run;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const PORT: &str = "/opt/local/bin/port";
const RELEASES_API: &str = "https://api.github.com/repos/macports/macports-base/releases/latest";
const PATH_LINE: &str = r#"export PATH="/opt/local/bin:/opt/local/sbin:$PATH""#;
const BREW_RELEASES_API: &str = "https://api.github.com/repos/Homebrew/brew/releases/latest";
const BREW_SHELLENV_LINE: &str = r#"eval "$(/opt/homebrew/bin/brew shellenv)""#;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub fn run_setup(dry_run: bool) -> Result<()> {
    if std::env::consts::ARCH == "aarch64" {
        return setup_homebrew(dry_run);
    }
    // Nach einem macOS-Upgrade passt die vorhandene MacPorts-Installation nicht mehr:
    // dann den Installer für die neue Version darüber installieren.
    let mut os_upgrade = false;
    if Path::new(PORT).exists() {
        match crate::doctor::macports_platform_mismatch() {
            Some(Some(msg)) => {
                println!("! MacPorts passt nicht mehr zur macOS-Version ({msg}).");
                println!("  Installiere die passende MacPorts-Version …");
                os_upgrade = true;
            }
            _ => {
                println!("✓ MacPorts ist bereits installiert.");
                return ensure_path(dry_run);
            }
        }
    }

    ensure_clt(dry_run)?;

    let major = macos_major()?;
    if major < 11 {
        bail!("macOS {major} wird nicht unterstützt (mindestens macOS 11 Big Sur).");
    }

    let (version, asset) = find_installer(major)?;
    println!("MacPorts {version} für macOS {major}: {}", asset.name);

    if dry_run {
        if os_upgrade {
            println!("→ danach müssten die installierten Pakete neu gebaut werden: mpkg migrate-macports");
        }
        println!("→ würde laden: {}", asset.browser_download_url);
        println!("→ würde installieren: sudo installer -pkg {} -target /", asset.name);
        return ensure_path(true);
    }

    install_pkg(&asset)?;

    ensure_path(false)?;
    run("sudo", &[PORT, "selfupdate"], false)?;

    println!("\n✓ MacPorts ist eingerichtet. Öffne ein neues Terminal oder lade deine Shell-Konfiguration neu.");
    if os_upgrade {
        println!("\n! Die bisher installierten Pakete wurden für die alte macOS-Version gebaut");
        println!("  und müssen neu installiert werden: mpkg migrate-macports");
    }
    Ok(())
}

/// Command Line Tools vorhanden? Sonst Apple-Installationsdialog öffnen.
pub(crate) fn ensure_clt(dry_run: bool) -> Result<()> {
    let ok = Command::new("xcode-select")
        .arg("-p")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        println!("✓ Xcode Command Line Tools vorhanden");
        return Ok(());
    }
    if dry_run {
        println!("→ würde `xcode-select --install` starten");
        return Ok(());
    }
    // Öffnet einen grafischen Dialog; die Installation läuft asynchron.
    run("xcode-select", &["--install"], false)?;
    bail!(
        "Die Command Line Tools werden installiert. Bitte den Dialog abschließen \
         und danach `mpkg setup` erneut ausführen."
    )
}

pub(crate) fn macos_major() -> Result<u32> {
    let out = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .context("`sw_vers` fehlgeschlagen")?;
    let version = String::from_utf8_lossy(&out.stdout);
    version
        .trim()
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .with_context(|| format!("unbekannte macOS-Version `{}`", version.trim()))
}

/// Neuesten Release eines GitHub-Repos über die API abfragen.
fn latest_release(api: &str) -> Result<Release> {
    let out = Command::new("curl")
        .args(["-fsSL", "-H", "Accept: application/vnd.github+json", api])
        .output()
        .context("`curl` fehlgeschlagen")?;
    if !out.status.success() {
        bail!("Releases konnten nicht abgefragt werden (GitHub-API: {api}).");
    }
    serde_json::from_slice(&out.stdout).context("Antwort der GitHub-API nicht lesbar")
}

/// Signiertes .pkg laden, mit Gatekeeper prüfen und installieren.
fn install_pkg(asset: &Asset) -> Result<()> {
    let pkg = std::env::temp_dir().join(&asset.name);
    let pkg_str = pkg.to_str().context("ungültiger Temp-Pfad")?;
    // --retry: Installer sind 30+ MB, Abbrüche bei schwachem WLAN sind häufig.
    run("curl", &["-fL", "--retry", "3", "--progress-bar", "-o", pkg_str, asset.browser_download_url.as_str()], false)?;
    verify_package(&pkg)?;
    run("sudo", &["installer", "-pkg", pkg_str, "-target", "/"], false)?;
    let _ = std::fs::remove_file(&pkg);
    Ok(())
}

/// Apple Silicon: Homebrew einrichten (oder vorhandenes nutzen).
fn setup_homebrew(dry_run: bool) -> Result<()> {
    let brew = crate::backend::homebrew::BREW;
    if Path::new(brew).exists() {
        println!("✓ Homebrew ist bereits installiert.");
        // Schon im PATH (z. B. über ~/.zshrc)? Dann keine zweite Zeile eintragen.
        if std::env::var("PATH").unwrap_or_default().split(':').any(|p| p == "/opt/homebrew/bin") {
            println!("✓ /opt/homebrew/bin ist im PATH");
            return Ok(());
        }
        return ensure_profile_line("/opt/homebrew/bin/brew shellenv", BREW_SHELLENV_LINE, "Homebrew", dry_run);
    }
    ensure_clt(dry_run)?;
    let release = latest_release(BREW_RELEASES_API)?;
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == "Homebrew.pkg")
        .context("Kein Homebrew.pkg im neuesten Homebrew-Release gefunden")?;
    println!("Homebrew {}: {}", release.tag_name, asset.name);
    if dry_run {
        println!("→ würde laden: {}", asset.browser_download_url);
        println!("→ würde installieren: sudo installer -pkg {} -target /", asset.name);
        return ensure_profile_line("/opt/homebrew/bin/brew shellenv", BREW_SHELLENV_LINE, "Homebrew", true);
    }
    install_pkg(&asset)?;
    ensure_profile_line("/opt/homebrew/bin/brew shellenv", BREW_SHELLENV_LINE, "Homebrew", false)?;
    println!("\n✓ Homebrew ist eingerichtet. Öffne ein neues Terminal oder lade deine Shell-Konfiguration neu.");
    Ok(())
}

/// Neuesten Release abfragen und den Installer für `major` heraussuchen.
fn find_installer(major: u32) -> Result<(String, Asset)> {
    let release = latest_release(RELEASES_API)?;

    // "-26-" passt auf "MacPorts-2.12.7-26-Tahoe.pkg", aber nicht auf "-10.11-".
    let marker = format!("-{major}-");
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name.starts_with("MacPorts-") && a.name.ends_with(".pkg") && a.name.contains(&marker))
        .with_context(|| {
            format!("Kein MacPorts-Installer für macOS {major} im neuesten Release gefunden – siehe macports.org/install.php")
        })?;

    let version = release.tag_name.trim_start_matches('v').to_owned();
    Ok((version, asset))
}

/// Gatekeeper-Prüfung: Paket muss mit Developer ID signiert und von Apple akzeptiert sein.
fn verify_package(pkg: &Path) -> Result<()> {
    let ok = Command::new("spctl")
        .args(["--assess", "--type", "install"])
        .arg(pkg)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        bail!("Gatekeeper hat {} abgelehnt – Installation abgebrochen.", pkg.display());
    }
    println!("✓ Signatur und Notarisierung geprüft");
    Ok(())
}

/// Shell-Konfigurationsdatei passend zur Login-Shell.
pub(crate) fn shell_profile() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME nicht gesetzt")?;
    let shell = std::env::var("SHELL").unwrap_or_default();
    let file = if shell.ends_with("bash") { ".bash_profile" } else { ".zprofile" };
    Ok(PathBuf::from(home).join(file))
}

/// /opt/local/bin in die Shell-Konfiguration eintragen (nach Rückfrage).
fn ensure_path(dry_run: bool) -> Result<()> {
    ensure_profile_line("/opt/local/bin", PATH_LINE, "MacPorts", dry_run)
}

/// `line` in die Shell-Konfiguration eintragen (nach Rückfrage), außer `marker` steht schon drin.
fn ensure_profile_line(marker: &str, line: &str, name: &str, dry_run: bool) -> Result<()> {
    let profile = shell_profile()?;
    let content = std::fs::read_to_string(&profile).unwrap_or_default();
    if content.contains(marker) {
        println!("✓ PATH-Eintrag in {} vorhanden", profile.display());
        return Ok(());
    }
    if dry_run {
        println!("→ würde in {} eintragen: {line}", profile.display());
        return Ok(());
    }

    print!("PATH-Eintrag für {name} in {} ergänzen? [J/n] ", profile.display());
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim(), "n" | "N") {
        println!("Übersprungen. Manuell eintragen: {line}");
        return Ok(());
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&profile)
        .with_context(|| format!("{} nicht beschreibbar", profile.display()))?;
    writeln!(file, "\n# {name} (eingetragen von mpkg)\n{line}")?;
    println!("✓ PATH-Eintrag ergänzt");
    Ok(())
}
