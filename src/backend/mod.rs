pub mod homebrew;
pub mod linuxvm;
pub mod macports;

use anyhow::{Context, Result, bail};
use std::process::Command;

/// Wie ein Paket in einem Backend verfügbar ist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// Fertiges Binärpaket vorhanden – schnell und zuverlässig.
    Binary,
    /// Paket existiert, ob es ein Binärpaket gibt, ließ sich nicht prüfen (z. B. offline).
    Unknown,
    /// Binärpaket nur für eine ältere Version – die aktuelle wird vermutlich noch gebaut.
    /// Installation jetzt = Build aus dem Quellcode.
    OlderBinary,
    /// Nur aus dem Quellcode baubar.
    SourceOnly,
    NotFound,
}

/// Gemeinsame Schnittstelle aller Paketquellen.
/// Neue Backends (Homebrew, Nix, …) implementieren nur diesen Trait.
pub trait Backend {
    fn id(&self) -> &'static str;
    /// Fallback (VM): nur gefragt, wenn kein natives Backend das Paket hat.
    fn is_fallback(&self) -> bool {
        false
    }
    /// Ist das Backend selbst auf diesem System eingerichtet?
    fn is_ready(&self) -> bool;
    fn availability(&self, name: &str) -> Result<Availability>;
    fn install(&self, name: &str, dry_run: bool) -> Result<()>;
    fn remove(&self, name: &str, dry_run: bool) -> Result<()>;
    fn search(&self, query: &str) -> Result<Vec<String>>;
    fn list(&self) -> Result<()>;
    fn update(&self, dry_run: bool) -> Result<()>;
    fn upgrade(&self, dry_run: bool) -> Result<()>;
}

/// Backends in Prioritätsreihenfolge: nativ zuerst, VM als Fallback.
/// Homebrew ist nur auf Apple Silicon bereit und hat dort Vorrang vor MacPorts.
pub fn available_backends() -> Vec<Box<dyn Backend>> {
    vec![
        Box::new(homebrew::Homebrew::new()),
        Box::new(macports::MacPorts::new()),
        Box::new(linuxvm::LinuxVm::new()),
    ]
}

/// Externes Programm sichtbar ausführen (mit `--dry-run` nur anzeigen).
pub(crate) fn run(program: &str, args: &[&str], dry_run: bool) -> Result<()> {
    println!("→ {} {}", program, args.join(" "));
    if dry_run {
        return Ok(());
    }
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("konnte `{program}` nicht starten"))?;
    if !status.success() {
        bail!("`{program}` wurde mit {status} beendet");
    }
    Ok(())
}

/// Diagnose-Ausgabe nach stderr, wenn `MPKG_DEBUG` gesetzt ist.
pub(crate) fn debug(msg: impl std::fmt::Display) {
    if std::env::var_os("MPKG_DEBUG").is_some() {
        eprintln!("[debug] {msg}");
    }
}

/// Liegt `program` irgendwo im PATH?
pub(crate) fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}
