//! Fallback-Backend: Debian in einer leichten VM (Phase 2).
//!
//! Der Rust-Kern spricht die VM nicht direkt an, sondern ruft den
//! Swift-Helfer `mpkg-vm` auf (siehe `helpers/mpkg-vm`). Protokoll:
//!
//!   mpkg-vm status            -> JSON {"state": "running" | "stopped" | "missing"}
//!   mpkg-vm start | stop
//!   mpkg-vm exec -- <cmd> …   -> führt <cmd> im Gast aus, Exit-Code wird durchgereicht
//!
//! Experiment (Option 3): dieselbe Schnittstelle direkt über die Crate
//! `objc2-virtualization` implementieren und den Helfer damit ersetzen.

use super::{Availability, Backend, which};
use anyhow::{Result, bail};

const HELPER: &str = "mpkg-vm";

pub struct LinuxVm;

impl LinuxVm {
    pub fn new() -> Self {
        Self
    }

    fn not_yet<T>(&self) -> Result<T> {
        bail!("Das VM-Backend kommt in Phase 2 (Swift-Helfer `{HELPER}`).")
    }
}

impl Backend for LinuxVm {
    fn id(&self) -> &'static str {
        "linuxvm"
    }

    fn is_ready(&self) -> bool {
        which(HELPER)
    }

    fn availability(&self, _name: &str) -> Result<Availability> {
        if !self.is_ready() {
            return Ok(Availability::NotFound);
        }
        // TODO: `mpkg-vm exec -- apt-cache show <name>` auswerten.
        Ok(Availability::NotFound)
    }

    fn install(&self, _name: &str, _dry_run: bool) -> Result<()> {
        // TODO: `apt-get install -y <name>` im Gast, danach Shim in ~/.mpkg/bin anlegen.
        self.not_yet()
    }

    fn remove(&self, _name: &str, _dry_run: bool) -> Result<()> {
        self.not_yet()
    }

    fn search(&self, _query: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn list(&self) -> Result<()> {
        Ok(())
    }

    fn update(&self, _dry_run: bool) -> Result<()> {
        Ok(())
    }

    fn upgrade(&self, _dry_run: bool) -> Result<()> {
        Ok(())
    }
}
