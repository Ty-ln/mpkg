//! `mpkg doctor`: prüft die Voraussetzungen und typische Konflikte.

use crate::setup::shell_profile;
use anyhow::Result;
use std::path::Path;
use std::process::Command;

const MACPORTS_BIN: &str = "/opt/local/bin";
const APPLE_SILICON_BREW_BIN: &str = "/opt/homebrew/bin";
const PORT: &str = "/opt/local/bin/port";
pub(crate) const MIGRATION_URL: &str = "https://trac.macports.org/wiki/Migration";

/// Passt die MacPorts-Installation noch zur laufenden macOS-Version?
/// Nach einem OS-Upgrade bricht `port` mit
/// „Current platform "darwin 26" does not match expected platform "darwin 25"“ ab.
/// Some(None) = passt, Some(Some(meldung)) = OS-Upgrade erkannt, None = nicht prüfbar.
pub(crate) fn macports_platform_mismatch() -> Option<Option<String>> {
    let out = Command::new(PORT)
        .args(["-q", "installed", "requested"])
        .output()
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let hit = text
        .lines()
        .find(|l| l.contains("does not match expected platform"))
        .map(|l| l.trim().trim_start_matches("Error:").trim().to_owned());
    Some(hit)
}

fn macports_version() -> Option<String> {
    let out = Command::new(PORT).arg("version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim()
        .strip_prefix("Version:")
        .map(|v| v.trim().to_owned())
}

fn check(label: &str, ok: bool, hint: &str) {
    if ok {
        println!("  ✓ {label}");
    } else {
        println!("  ✗ {label}\n      → {hint}");
    }
}

/// bin-Verzeichnis einer vorhandenen Homebrew-Installation.
fn homebrew_bin() -> Option<&'static str> {
    ["/opt/homebrew/bin", "/usr/local/bin"]
        .into_iter()
        .find(|dir| Path::new(dir).join("brew").exists())
}

/// Ist `dir` im PATH? Unterscheidet „fehlt“ von „eingetragen, aber noch nicht geladen“.
fn check_path(entries: &[&str], dir: &str, hint: &str) {
    let label = format!("{dir} im PATH");
    let profile = shell_profile().ok();
    let in_profile = profile
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|c| c.contains(dir));
    if !entries.contains(&dir) && in_profile {
        let file = profile.map(|p| p.display().to_string()).unwrap_or_default();
        check(
            &label,
            false,
            &format!(
                "steht in {file}, ist aber noch nicht geladen – neues Terminal öffnen oder `source {file}`"
            ),
        );
    } else {
        check(&label, entries.contains(&dir), hint);
    }
}

fn check_macports(required: bool) {
    let installed = Path::new(PORT).exists();
    if !installed && !required {
        println!("  · MacPorts nicht installiert (optional)");
        return;
    }
    let label = match installed.then(macports_version).flatten() {
        Some(v) => format!("MacPorts installiert ({v})"),
        None => "MacPorts installiert".to_owned(),
    };
    check(&label, installed, "mpkg setup");
    if !installed {
        return;
    }
    match macports_platform_mismatch() {
        Some(None) => check("MacPorts passt zur macOS-Version", true, ""),
        Some(Some(msg)) => check(
            "MacPorts passt zur macOS-Version",
            false,
            &format!(
                "macOS wurde aktualisiert ({msg}).\n        \
                 `mpkg migrate-macports` installiert MacPorts und alle Pakete neu\n        \
                 (Hintergrund: {MIGRATION_URL})"
            ),
        ),
        None => println!("  ? MacPorts-Plattform nicht prüfbar"),
    }
}

pub fn run() -> Result<()> {
    let macos = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unbekannt".into());
    println!("macOS {macos} ({})\n", std::env::consts::ARCH);

    let clt = Command::new("xcode-select")
        .arg("-p")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    check("Xcode Command Line Tools", clt, "xcode-select --install");

    let path = std::env::var("PATH").unwrap_or_default();
    let entries: Vec<&str> = path.split(':').collect();

    // Apple Silicon: Homebrew ist das Haupt-Backend, MacPorts optional.
    if std::env::consts::ARCH == "aarch64" {
        let brew = Path::new(crate::backend::homebrew::BREW).exists();
        check("Homebrew installiert", brew, "mpkg setup");
        if brew {
            check_path(
                &entries,
                APPLE_SILICON_BREW_BIN,
                "mpkg setup (oder manuell: eval \"$(/opt/homebrew/bin/brew shellenv)\" in ~/.zprofile)",
            );
        }
        check_macports(false);
        if Path::new(PORT).exists() {
            check_path(
                &entries,
                MACPORTS_BIN,
                "export PATH=\"/opt/local/bin:/opt/local/sbin:$PATH\" in ~/.zprofile",
            );
        }
        return Ok(());
    }

    // Intel: MacPorts ist das Haupt-Backend.
    check_macports(true);
    check_path(
        &entries,
        MACPORTS_BIN,
        "mpkg setup (oder manuell: export PATH=\"/opt/local/bin:/opt/local/sbin:$PATH\" in ~/.zprofile)",
    );

    // Homebrew-Konflikte
    if let Some(brew_bin) = homebrew_bin() {
        let brew_idx = entries.iter().position(|p| *p == brew_bin);
        let macports_idx = entries.iter().position(|p| *p == MACPORTS_BIN);
        match (brew_idx, macports_idx) {
            (Some(b), Some(m)) if b < m => {
                println!("  ! Homebrew ({brew_bin}) steht im PATH vor MacPorts.");
                println!(
                    "      → Gleichnamige Programme kommen von Homebrew. Die MacPorts-Zeile in der"
                );
                println!("        Shell-Konfiguration hinter `brew shellenv` verschieben.");
            }
            _ => println!(
                "  ! Homebrew ist ebenfalls installiert – PATH-Reihenfolge ist in Ordnung."
            ),
        }
        println!(
            "    Auf Intel baut Homebrew keine neuen Bottles mehr → `mpkg migrate-from-brew`."
        );
    }
    Ok(())
}
