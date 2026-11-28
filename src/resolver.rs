//! Picks the right backend for each package and the package's name there.

use crate::backend::{Availability, Backend};
use crate::mapping::MappingDb;
use crate::state::State;
use anyhow::{Result, anyhow, bail};
use std::cell::RefCell;

/// Up to this macOS version mpkg automatically picks the VM when only source is available.
const OLD_MACOS_MAX: u32 = 12;

pub struct Resolver {
    mapping: MappingDb,
    backends: Vec<Box<dyn Backend>>,
    /// RefCell so that read-only methods (&self) can still update the state.
    state: RefCell<State>,
}

/// Result of picking a backend for a package.
pub struct Choice<'a> {
    pub backend: &'a dyn Backend,
    pub native: String,
    pub availability: Availability,
}

impl Resolver {
    pub fn new(mapping: MappingDb, backends: Vec<Box<dyn Backend>>, state: State) -> Self {
        Self {
            mapping,
            backends,
            state: RefCell::new(state),
        }
    }

    pub fn mapping(&self) -> &MappingDb {
        &self.mapping
    }

    fn backend_by_id(&self, id: &str) -> Result<&dyn Backend> {
        self.backends
            .iter()
            .find(|b| b.id() == id)
            .map(|b| &**b as &dyn Backend)
            .ok_or_else(|| anyhow!("unknown backend `{id}`"))
    }

    fn ready_backends(&self) -> Vec<&dyn Backend> {
        self.backends
            .iter()
            .filter(|b| b.is_ready())
            .map(|b| &**b as &dyn Backend)
            .collect()
    }

    /// Aborts with a clear message if no backend is set up.
    pub fn ensure_any_ready(&self) -> Result<()> {
        if self.ready_backends().is_empty() {
            bail!(
                "No backend set up (Homebrew on Apple Silicon, otherwise MacPorts).\n\
                 Run `mpkg setup` to set it up automatically."
            );
        }
        Ok(())
    }

    /// Order: forced backend → binary package → not checkable → source only.
    pub fn choose(&self, name: &str, forced: Option<&str>) -> Result<Choice<'_>> {
        if let Some(id) = forced {
            let backend = self.backend_by_id(id)?;
            if !backend.is_ready() {
                bail!("{}", not_ready_hint(id));
            }
            let native = self.mapping.name_for(name, backend.id());
            let availability = backend.availability(&native)?;
            return Ok(Choice {
                backend,
                native,
                availability,
            });
        }

        self.ensure_any_ready()?;

        let rank = |a: Availability| match a {
            Availability::Binary => 0,
            Availability::Unknown => 1,
            Availability::OlderBinary => 2,
            Availability::SourceOnly => 3,
            Availability::NotFound => 4,
        };

        // On old systems source builds take long and fail more often:
        // there the prebuilt Debian package from the VM is the better choice.
        let prefer_vm_over_source = crate::setup::macos_major().is_ok_and(|m| m <= OLD_MACOS_MAX);

        let mut best: Option<Choice> = None;
        for backend in self.ready_backends() {
            if backend.on_request_only() {
                continue;
            }
            // Only ask the VM (and possibly boot it for that) if there is no native binary:
            // nothing found at all, or only source on an old system.
            let native_needs_build = best.as_ref().is_some_and(|b| {
                matches!(
                    b.availability,
                    Availability::SourceOnly | Availability::OlderBinary
                )
            });
            if backend.is_fallback()
                && best.is_some()
                && !(prefer_vm_over_source && native_needs_build)
            {
                continue;
            }
            let native = self.mapping.name_for(name, backend.id());
            let availability = backend.availability(&native)?;
            if availability == Availability::Binary {
                return Ok(Choice {
                    backend,
                    native,
                    availability,
                });
            }
            if availability != Availability::NotFound
                && best
                    .as_ref()
                    .is_none_or(|b| rank(availability) < rank(b.availability))
            {
                best = Some(Choice {
                    backend,
                    native,
                    availability,
                });
            }
        }
        best.ok_or_else(|| {
            let cask = self.ready_backends().into_iter().find(|b| b.on_request_only());
            match cask {
                Some(c) if c.availability(name).is_ok_and(|a| a == Availability::Binary) => anyhow!(
                    "Package `{name}` was not found in any backend – but there is an app `{name}`:\n  mpkg install --cask {name}"
                ),
                _ => anyhow!("Package `{name}` was not found in any backend"),
            }
        })
    }

    pub fn install(&self, names: &[String], forced: Option<&str>, dry_run: bool) -> Result<()> {
        for name in names {
            // Known name: do not install again, and above all never overwrite an entry of
            // another backend (e.g. a formula and an app with the same name from third-party taps).
            if let Some(i) = self.state.borrow().get(name) {
                match forced {
                    Some(id) if id != i.backend => bail!(
                        "`{name}` is already installed via {}. Run `mpkg remove {name}` first, then install again.",
                        i.backend
                    ),
                    _ => {
                        println!(
                            "{name} is already installed ({} via {})",
                            i.native, i.backend
                        );
                        continue;
                    }
                }
            }
            let c = self.choose(name, forced)?;
            println!(
                "{name} → {} (via {}){}",
                c.native,
                c.backend.id(),
                note(c.availability)
            );
            let vm_ready = self.ready_backends().iter().any(|b| b.is_fallback());
            if vm_ready
                && matches!(
                    c.availability,
                    Availability::SourceOnly | Availability::OlderBinary
                )
            {
                println!(
                    "  Tip: `mpkg install --backend linuxvm {name}` uses the prebuilt Debian package from the VM."
                );
            }
            c.backend.install(&c.native, dry_run)?;
            if !dry_run {
                let mut state = self.state.borrow_mut();
                state.record(name, c.backend.id(), &c.native);
                state.save()?;
            }
        }
        Ok(())
    }

    /// Records an already installed package in the state without reinstalling it.
    /// Returns false if mpkg already knows the name (also from another backend).
    pub fn adopt(&self, name: &str, backend: &str, native: &str, dry_run: bool) -> Result<bool> {
        let mut state = self.state.borrow_mut();
        if state.get(name).is_some() {
            return Ok(false);
        }
        if dry_run {
            return Ok(true);
        }
        state.record(name, backend, native);
        state.save()?;
        Ok(true)
    }

    pub fn remove(&self, names: &[String], dry_run: bool) -> Result<()> {
        for name in names {
            // Known package: remove it exactly where it was installed.
            let known = self.state.borrow().get(name).cloned();
            let (backend, native) = match known {
                Some(i) => (self.backend_by_id(&i.backend)?, i.native),
                None => {
                    let c = self.choose(name, None)?;
                    (c.backend, c.native)
                }
            };
            println!("{name} → removing {native} (via {})", backend.id());
            backend.remove(&native, dry_run)?;
            if !dry_run {
                let mut state = self.state.borrow_mut();
                state.forget(name);
                state.save()?;
            }
        }
        Ok(())
    }

    pub fn search(&self, query: &str) -> Result<()> {
        self.ensure_any_ready()?;
        for backend in self.ready_backends() {
            for hit in backend.search(query)? {
                println!("{hit:<30} [{}]", backend.id());
            }
        }
        Ok(())
    }

    pub fn show(&self, name: &str) -> Result<()> {
        self.ensure_any_ready()?;
        println!("Package: {name}");
        if let Some(entry) = self.mapping.entry(name) {
            println!("  Mapping: {entry:?}");
        }
        for backend in self.ready_backends() {
            let native = self.mapping.name_for(name, backend.id());
            println!(
                "  {:<14} {native:<20} {:?}",
                backend.id(),
                backend.availability(&native)?
            );
        }
        Ok(())
    }

    pub fn list(&self, all: bool) -> Result<()> {
        {
            let state = self.state.borrow();
            if state.packages.is_empty() {
                println!("No packages installed via mpkg yet.");
            }
            for (name, i) in &state.packages {
                println!("{name:<25} {:<20} [{}]", i.native, i.backend);
            }
        }
        if all {
            for backend in self.ready_backends() {
                println!("\n[{} – all requested packages]", backend.id());
                backend.list()?;
            }
        }
        Ok(())
    }

    pub fn update(&self, dry_run: bool) -> Result<()> {
        for backend in self.ready_backends() {
            backend.update(dry_run)?;
        }
        Ok(())
    }

    pub fn upgrade(&self, dry_run: bool) -> Result<()> {
        for backend in self.ready_backends() {
            backend.upgrade(dry_run)?;
        }
        Ok(())
    }

    pub fn print_backends(&self) {
        for backend in &self.backends {
            let state = if backend.is_ready() {
                "ready"
            } else {
                "not set up"
            };
            println!("{:<14} {state}", backend.id());
        }
    }
}

/// Why a forced backend cannot be used.
fn not_ready_hint(id: &str) -> String {
    match id {
        "homebrew-cask" | "homebrew" => {
            if std::env::consts::ARCH == "aarch64" {
                "Homebrew is not installed – `mpkg setup` sets it up.".to_owned()
            } else {
                "mpkg uses Homebrew (and therefore apps/casks) only on Apple Silicon. Homebrew no longer builds packages for Intel.".to_owned()
            }
        }
        "linuxvm" => "The VM has not been created yet – run `mpkg vm create`.".to_owned(),
        _ => format!("Backend `{id}` is not set up – run `mpkg setup` or `mpkg doctor`."),
    }
}

/// Availability hint for the output.
pub fn note(a: Availability) -> &'static str {
    match a {
        Availability::Binary => "",
        Availability::Unknown => "  [binary package could not be checked]",
        Availability::OlderBinary => {
            "  [⏳ binary package only for the previous version – building from source now, or try again later]"
        }
        Availability::SourceOnly => "  [⚠ no binary package – will be built from source]",
        Availability::NotFound => "  [not found]",
    }
}
