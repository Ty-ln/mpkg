//! `mpkg migrate-from-brew`: take over explicitly installed Homebrew packages.
//! `mpkg migrate-macports`: rebuild MacPorts after a macOS upgrade.

use crate::backend::{Availability, run, which};
use crate::resolver::{Resolver, note};
use anyhow::{Context, Result, bail};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

/// An entry from `brew leaves`, e.g. "supabase/tap/supabase".
struct BrewPkg {
    name: String,
    /// Third-party tap (e.g. "supabase/tap"); None for homebrew/core.
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

/// Lines of a brew output (one per package).
fn brew_lines(args: &[&str]) -> Result<Vec<String>> {
    let out = Command::new("brew")
        .args(args)
        .output()
        .with_context(|| format!("`brew {}` failed", args.join(" ")))?;
    if !out.status.success() {
        bail!("`brew {}` failed", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Apple Silicon: Homebrew itself is the backend – reinstall nothing, just record packages
/// and apps in the mpkg state so that `list`/`remove` know them.
fn adopt_brew(resolver: &Resolver, dry_run: bool) -> Result<()> {
    let formulae = brew_lines(&["leaves"])?;
    let casks = brew_lines(&["list", "--cask"])?;
    println!("Homebrew is mpkg's backend on this Mac – nothing will be reinstalled.");
    println!(
        "Adding {} packages and {} apps to the mpkg list:\n",
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
        // Formula and app with the same name (only possible via third-party taps): the formula
        // is recorded, the app stays in Homebrew and is counted as "already known".
        let added = resolver.adopt(&name, backend, native, dry_run)?;
        if added {
            new += 1;
            println!("  + {name:<25} [{backend}]");
        } else {
            known += 1;
        }
    }
    println!("\n{new} added · {known} already known");
    Ok(())
}

pub fn from_brew(resolver: &Resolver, dry_run: bool) -> Result<()> {
    if !which("brew") {
        bail!("Homebrew was not found.");
    }
    if std::env::consts::ARCH == "aarch64" && Path::new(crate::backend::homebrew::BREW).exists() {
        return adopt_brew(resolver, dry_run);
    }
    resolver.ensure_any_ready()?;

    // `brew leaves` = only explicitly installed formulae, without dependencies.
    let out = Command::new("brew")
        .arg("leaves")
        .output()
        .context("`brew leaves` failed")?;
    let leaves: Vec<BrewPkg> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(parse_leaf)
        .collect();

    println!(
        "Found {} Homebrew packages (checking binary packages …):\n",
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
                        "  ✗ {:<25} → third-party tap ({tap}), not available natively",
                        pkg.name
                    );
                }
                None => {
                    missing += 1;
                    println!("  ✗ {:<25} → not found", pkg.name);
                }
            },
        }
    }

    println!(
        "\n{binary} as binary package · {unknown} not checkable · {source} source only · {third_party} from third-party taps · {missing} not found"
    );

    // There are no apps via mpkg on Intel (MacPorts has none, the VM cannot run Mac apps).
    let casks = brew_lines(&["list", "--cask"]).unwrap_or_default();
    if !casks.is_empty() {
        println!(
            "\n{} apps (casks) – no replacement via mpkg on Intel:",
            casks.len()
        );
        for c in &casks {
            println!("  · {c}");
        }
        println!("  They keep working. Updates come from the app itself or the vendor's website.");
    }
    if dry_run || to_install.is_empty() {
        return Ok(());
    }

    print!(
        "Install {} packages now? Homebrew stays unchanged. [y/N] ",
        to_install.len()
    );
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y") {
        println!("Aborted.");
        return Ok(());
    }
    resolver.install(&to_install, None, false)
}

/// Leaves the migration to `port migrate` (MacPorts 2.9+): rebuilds MacPorts itself and the
/// requested packages with their dependencies for the new platform. It also detects when
/// there is nothing to do and asks for confirmation itself before starting.
pub fn macports(dry_run: bool) -> Result<()> {
    const PORT: &str = "/opt/local/bin/port";
    if !Path::new(PORT).exists() {
        bail!("MacPorts is not installed – run `mpkg setup` first.");
    }
    // Rebuilding MacPorts needs compilers from the Command Line Tools.
    crate::setup::ensure_clt(dry_run)?;
    run("sudo", &[PORT, "migrate"], dry_run)
}
