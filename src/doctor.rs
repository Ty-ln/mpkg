//! `mpkg doctor`: prüft die Voraussetzungen und typische Konflikte.

use crate::setup::shell_profile;
use anyhow::Result;
use std::path::Path;
use std::process::Command;

const MACPORTS_BIN: &str = "/opt/local/bin";

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

    check(
        "MacPorts installiert",
        Path::new("/opt/local/bin/port").exists(),
        "mpkg setup",
    );

    // PATH-Prüfung
    let path = std::env::var("PATH").unwrap_or_default();
    let entries: Vec<&str> = path.split(':').collect();
    let macports_idx = entries.iter().position(|p| *p == MACPORTS_BIN);

    let profile = shell_profile().ok();
    let in_profile = profile
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|c| c.contains(MACPORTS_BIN));

    if macports_idx.is_none() && in_profile {
        let file = profile.map(|p| p.display().to_string()).unwrap_or_default();
        check(
            "/opt/local/bin im PATH",
            false,
            &format!("steht in {file}, ist aber noch nicht geladen – neues Terminal öffnen oder `source {file}`"),
        );
    } else {
        check(
            "/opt/local/bin im PATH",
            macports_idx.is_some(),
            "mpkg setup (oder manuell: export PATH=\"/opt/local/bin:/opt/local/sbin:$PATH\" in ~/.zprofile)",
        );
    }

    // Homebrew-Konflikte
    if let Some(brew_bin) = homebrew_bin() {
        let brew_idx = entries.iter().position(|p| *p == brew_bin);
        match (brew_idx, macports_idx) {
            (Some(b), Some(m)) if b < m => {
                println!("  ! Homebrew ({brew_bin}) steht im PATH vor MacPorts.");
                println!("      → Gleichnamige Programme kommen von Homebrew. Die MacPorts-Zeile in der");
                println!("        Shell-Konfiguration hinter `brew shellenv` verschieben.");
            }
            _ => println!("  ! Homebrew ist ebenfalls installiert – PATH-Reihenfolge ist in Ordnung."),
        }
        if std::env::consts::ARCH == "x86_64" {
            println!("    Auf Intel baut Homebrew keine neuen Bottles mehr → `mpkg migrate-from-brew`.");
        }
    }
    Ok(())
}
