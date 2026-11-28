pub mod homebrew;
pub mod linuxvm;
pub mod macports;

use anyhow::{Context, Result, bail};
use std::process::Command;

/// How a package is available in a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// Prebuilt binary package available – fast and reliable.
    Binary,
    /// Package exists; whether there is a binary package could not be checked (e.g. offline).
    Unknown,
    /// Binary package only for an older version – the current one is probably still being built.
    /// Installing now = building from source.
    OlderBinary,
    /// Can only be built from source.
    SourceOnly,
    NotFound,
}

/// Common interface of all package sources.
/// New backends (Nix, …) only implement this trait.
pub trait Backend {
    fn id(&self) -> &'static str;
    /// Fallback (VM): only asked when no native backend has the package.
    fn is_fallback(&self) -> bool {
        false
    }
    /// Only on explicit request (e.g. `--cask`), never in the automatic choice.
    fn on_request_only(&self) -> bool {
        false
    }
    /// Is the backend itself set up on this system?
    fn is_ready(&self) -> bool;
    fn availability(&self, name: &str) -> Result<Availability>;
    fn install(&self, name: &str, dry_run: bool) -> Result<()>;
    fn remove(&self, name: &str, dry_run: bool) -> Result<()>;
    fn search(&self, query: &str) -> Result<Vec<String>>;
    fn list(&self) -> Result<()>;
    fn update(&self, dry_run: bool) -> Result<()>;
    fn upgrade(&self, dry_run: bool) -> Result<()>;
}

/// Backends in priority order: native first, VM as fallback.
/// Homebrew is only ready on Apple Silicon and takes precedence over MacPorts there.
pub fn available_backends() -> Vec<Box<dyn Backend>> {
    vec![
        Box::new(homebrew::Homebrew::new()),
        Box::new(homebrew::HomebrewCask),
        Box::new(macports::MacPorts::new()),
        Box::new(linuxvm::LinuxVm::new()),
    ]
}

/// Run an external program visibly (with `--dry-run` only show it).
pub(crate) fn run(program: &str, args: &[&str], dry_run: bool) -> Result<()> {
    println!("→ {} {}", program, args.join(" "));
    if dry_run {
        return Ok(());
    }
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("could not start `{program}`"))?;
    if !status.success() {
        bail!("`{program}` exited with {status}");
    }
    Ok(())
}

/// Diagnostic output to stderr when `MPKG_DEBUG` is set.
pub(crate) fn debug(msg: impl std::fmt::Display) {
    if std::env::var_os("MPKG_DEBUG").is_some() {
        eprintln!("[debug] {msg}");
    }
}

/// Is `program` anywhere in PATH?
pub(crate) fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}
