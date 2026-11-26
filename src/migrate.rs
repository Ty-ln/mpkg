//! `mpkg migrate-from-brew`: explizit installierte Homebrew-Pakete übernehmen.

use crate::backend::{Availability, which};
use crate::resolver::{Resolver, note};
use anyhow::{Context, Result, bail};
use std::io::{self, Write};
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
        Some((_, name)) => BrewPkg { name: name.to_owned(), tap: None },
        None => BrewPkg { name: line.to_owned(), tap: None },
    }
}

pub fn from_brew(resolver: &Resolver, dry_run: bool) -> Result<()> {
    if !which("brew") {
        bail!("Homebrew wurde nicht gefunden.");
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

    println!("{} Homebrew-Pakete gefunden (Binärpakete werden geprüft …):\n", leaves.len());

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
                    println!("  ✗ {:<25} → Drittquelle (Tap {tap}), nicht nativ verfügbar", pkg.name);
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
    if dry_run || to_install.is_empty() {
        return Ok(());
    }

    print!("{} Pakete jetzt installieren? Homebrew bleibt unverändert. [j/N] ", to_install.len());
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim(), "j" | "J" | "y" | "Y") {
        println!("Abgebrochen.");
        return Ok(());
    }
    resolver.install(&to_install, None, false)
}
