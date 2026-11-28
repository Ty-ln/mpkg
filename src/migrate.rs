//! `mpkg migrate-from-brew`: explizit installierte Homebrew-Pakete übernehmen.
//! `mpkg migrate-macports`: MacPorts nach einem macOS-Upgrade neu aufsetzen.

use crate::backend::{Availability, run, which};
use crate::resolver::{Resolver, note};
use anyhow::{Context, Result, bail};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

/// Ein Eintrag aus `brew leaves`, z. B. "supabase/tap/supabase".
struct BrewPkg {
    name: String,
    /// Drittanbieter-Tap (z. B. "supabase/tap"); None bei homebrew/core.
    tap: Option<String>,
}

fn parse_leaf(line: &str) -> BrewPkg {
    match line.rsplit_once('/') {
        Some((tap, name)) if tap != "homebrew/core" => BrewPkg {
            name: name.to_owned(),
            tap: Some(tap.to_owned()),
        },
        Some((_, name)) => BrewPkg {
            name: name.to_owned(),
            tap: None,
        },
        None => BrewPkg {
            name: line.to_owned(),
            tap: None,
        },
    }
}

/// Zeilen einer brew-Ausgabe (eine pro Paket).
fn brew_lines(args: &[&str]) -> Result<Vec<String>> {
    let out = Command::new("brew")
        .args(args)
        .output()
        .with_context(|| format!("`brew {}` fehlgeschlagen", args.join(" ")))?;
    if !out.status.success() {
        bail!("`brew {}` fehlgeschlagen", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Apple Silicon: Homebrew ist selbst das Backend – nichts neu installieren, nur Pakete
/// und Apps in den mpkg-Status übernehmen, damit `list`/`remove` sie kennen.
fn adopt_brew(resolver: &Resolver, dry_run: bool) -> Result<()> {
    let formulae = brew_lines(&["leaves"])?;
    let casks = brew_lines(&["list", "--cask"])?;
    println!("Homebrew ist auf diesem Mac das Backend von mpkg – es wird nichts neu installiert.");
    println!(
        "Übernehme {} Pakete und {} Apps in die mpkg-Liste:\n",
        formulae.len(),
        casks.len()
    );
    let (mut new, mut known) = (0, 0);
    let entries = formulae
        .iter()
        .map(|f| {
            (
                resolver.mapping().canonical_from_brew(&parse_leaf(f).name),
                "homebrew",
                f.as_str(),
            )
        })
        .chain(
            casks
                .iter()
                .map(|c| (c.clone(), "homebrew-cask", c.as_str())),
        );
    for (name, backend, native) in entries {
        // Gleichnamige Formel und App (nur aus Drittquellen möglich): die Formel wird übernommen,
        // die App bleibt in Homebrew und erscheint unter „schon bekannt“.
        let added = resolver.adopt(&name, backend, native, dry_run)?;
        if added {
            new += 1;
            println!("  + {name:<25} [{backend}]");
        } else {
            known += 1;
        }
    }
    println!("\n{new} übernommen · {known} waren schon bekannt");
    Ok(())
}

pub fn from_brew(resolver: &Resolver, dry_run: bool) -> Result<()> {
    if !which("brew") {
        bail!("Homebrew wurde nicht gefunden.");
    }
    if std::env::consts::ARCH == "aarch64" && Path::new(crate::backend::homebrew::BREW).exists() {
        return adopt_brew(resolver, dry_run);
    }
    resolver.ensure_any_ready()?;

    // `brew leaves` = nur explizit installierte Formeln, ohne Abhängigkeiten.
    let out = Command::new("brew")
        .arg("leaves")
        .output()
        .context("`brew leaves` fehlgeschlagen")?;
    let leaves: Vec<BrewPkg> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(parse_leaf)
        .collect();

    println!(
        "{} Homebrew-Pakete gefunden (Binärpakete werden geprüft …):\n",
        leaves.len()
    );

    let (mut binary, mut unknown, mut source, mut third_party, mut missing) = (0, 0, 0, 0, 0);
    let mut to_install = Vec::new();

    for pkg in &leaves {
        let canonical = resolver.mapping().canonical_from_brew(&pkg.name);
        match resolver.choose(&canonical, None) {
            Ok(c) => {
                let mark = match c.availability {
                    Availability::Binary => {
                        binary += 1;
                        "✓"
                    }
                    Availability::SourceOnly => {
                        source += 1;
                        "⚠"
                    }
                    Availability::OlderBinary => {
                        source += 1;
                        "⏳"
                    }
                    _ => {
                        unknown += 1;
                        "?"
                    }
                };
                println!(
                    "  {mark} {:<25} → {} ({}){}",
                    pkg.name,
                    c.native,
                    c.backend.id(),
                    note(c.availability)
                );
                to_install.push(canonical);
            }
            Err(_) => match &pkg.tap {
                Some(tap) => {
                    third_party += 1;
                    println!(
                        "  ✗ {:<25} → Drittquelle (Tap {tap}), nicht nativ verfügbar",
                        pkg.name
                    );
                }
                None => {
                    missing += 1;
                    println!("  ✗ {:<25} → nicht gefunden", pkg.name);
                }
            },
        }
    }

    println!(
        "\n{binary} als Binärpaket · {unknown} nicht prüfbar · {source} nur aus Quellcode · {third_party} aus Drittquellen · {missing} nicht gefunden"
    );

    // Apps gibt es auf Intel über mpkg nicht (MacPorts hat keine, die VM kann keine Mac-Apps).
    let casks = brew_lines(&["list", "--cask"]).unwrap_or_default();
    if !casks.is_empty() {
        println!(
            "\n{} Apps (Casks) – auf Intel ohne Ersatz über mpkg:",
            casks.len()
        );
        for c in &casks {
            println!("  · {c}");
        }
        println!(
            "  Sie laufen weiter. Updates kommen über die App selbst oder von der Hersteller-Website."
        );
    }
    if dry_run || to_install.is_empty() {
        return Ok(());
    }

    print!(
        "{} Pakete jetzt installieren? Homebrew bleibt unverändert. [j/N] ",
        to_install.len()
    );
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim(), "j" | "J" | "y" | "Y") {
        println!("Abgebrochen.");
        return Ok(());
    }
    resolver.install(&to_install, None, false)
}

/// Überlässt die Migration `port migrate` (ab MacPorts 2.9): baut MacPorts selbst und die
/// angeforderten Pakete samt Abhängigkeiten für die neue Plattform neu. Erkennt auch,
/// wenn nichts zu tun ist, und fragt vor dem Start selbst nach.
pub fn macports(dry_run: bool) -> Result<()> {
    const PORT: &str = "/opt/local/bin/port";
    if !Path::new(PORT).exists() {
        bail!("MacPorts ist nicht installiert – zuerst `mpkg setup`.");
    }
    // Der Neubau von MacPorts braucht Compiler aus den Command Line Tools.
    crate::setup::ensure_clt(dry_run)?;
    run("sudo", &[PORT, "migrate"], dry_run)
}
