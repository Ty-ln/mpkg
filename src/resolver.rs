//! Wählt für jedes Paket das passende Backend und den dortigen Namen.

use crate::backend::{Availability, Backend};
use crate::mapping::MappingDb;
use crate::state::State;
use anyhow::{Result, anyhow, bail};
use std::cell::RefCell;

/// Bis zu dieser macOS-Version wählt mpkg bei „nur Quellcode“ automatisch die VM.
const OLD_MACOS_MAX: u32 = 12;

pub struct Resolver {
    mapping: MappingDb,
    backends: Vec<Box<dyn Backend>>,
    /// RefCell, damit lesende Methoden (&self) den Status trotzdem aktualisieren können.
    state: RefCell<State>,
}

/// Ergebnis der Backend-Wahl für ein Paket.
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
            .ok_or_else(|| anyhow!("unbekanntes Backend `{id}`"))
    }

    fn ready_backends(&self) -> Vec<&dyn Backend> {
        self.backends
            .iter()
            .filter(|b| b.is_ready())
            .map(|b| &**b as &dyn Backend)
            .collect()
    }

    /// Bricht mit einer klaren Meldung ab, wenn kein Backend eingerichtet ist.
    pub fn ensure_any_ready(&self) -> Result<()> {
        if self.ready_backends().is_empty() {
            bail!(
                "Kein Backend eingerichtet (Homebrew auf Apple Silicon, sonst MacPorts).\n\
                 Führe `mpkg setup` aus, um es automatisch einzurichten."
            );
        }
        Ok(())
    }

    /// Reihenfolge: erzwungenes Backend → Binärpaket → nicht prüfbar → nur Quellcode.
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

        // Auf alten Systemen dauern Quellcode-Builds lange und scheitern öfter (siehe 06):
        // dort ist das fertige Debian-Paket aus der VM die bessere Wahl.
        let prefer_vm_over_source = crate::setup::macos_major().is_ok_and(|m| m <= OLD_MACOS_MAX);

        let mut best: Option<Choice> = None;
        for backend in self.ready_backends() {
            if backend.on_request_only() {
                continue;
            }
            // Die VM nur fragen (und dafür ggf. starten), wenn nativ kein Binärpaket da ist:
            // gar nichts gefunden, oder nur Quellcode auf einem alten System.
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
                    "Paket `{name}` wurde in keinem Backend gefunden – es gibt aber die App `{name}`:\n  mpkg install --cask {name}"
                ),
                _ => anyhow!("Paket `{name}` wurde in keinem Backend gefunden"),
            }
        })
    }

    pub fn install(&self, names: &[String], forced: Option<&str>, dry_run: bool) -> Result<()> {
        for name in names {
            // Bekannter Name: nicht erneut installieren und vor allem keinen Eintrag eines
            // anderen Backends überschreiben (z. B. gleichnamige Formel und App aus Drittquellen).
            if let Some(i) = self.state.borrow().get(name) {
                match forced {
                    Some(id) if id != i.backend => bail!(
                        "`{name}` ist schon über {} installiert. Erst `mpkg remove {name}`, dann erneut installieren.",
                        i.backend
                    ),
                    _ => {
                        println!(
                            "{name} ist bereits installiert ({} über {})",
                            i.native, i.backend
                        );
                        continue;
                    }
                }
            }
            let c = self.choose(name, forced)?;
            println!(
                "{name} → {} (über {}){}",
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
                    "  Tipp: `mpkg install --backend linuxvm {name}` nimmt das fertige Debian-Paket aus der VM."
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

    /// Bereits vorhandenes Paket in den Status übernehmen, ohne es neu zu installieren.
    /// Gibt false zurück, wenn mpkg den Namen schon kennt (auch von einem anderen Backend).
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
            // Bekanntes Paket: genau dort entfernen, wo es installiert wurde.
            let known = self.state.borrow().get(name).cloned();
            let (backend, native) = match known {
                Some(i) => (self.backend_by_id(&i.backend)?, i.native),
                None => {
                    let c = self.choose(name, None)?;
                    (c.backend, c.native)
                }
            };
            println!("{name} → entferne {native} (über {})", backend.id());
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
        println!("Paket: {name}");
        if let Some(entry) = self.mapping.entry(name) {
            println!("  Mapping: {entry:?}");
        }
        for backend in self.ready_backends() {
            let native = self.mapping.name_for(name, backend.id());
            println!(
                "  {:<10} {native:<20} {:?}",
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
                println!("Noch keine Pakete über mpkg installiert.");
            }
            for (name, i) in &state.packages {
                println!("{name:<25} {:<20} [{}]", i.native, i.backend);
            }
        }
        if all {
            for backend in self.ready_backends() {
                println!("\n[{} – alle angeforderten Pakete]", backend.id());
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
                "bereit"
            } else {
                "nicht eingerichtet"
            };
            println!("{:<14} {state}", backend.id());
        }
    }
}

/// Warum ein erzwungenes Backend nicht nutzbar ist.
fn not_ready_hint(id: &str) -> String {
    match id {
        "homebrew-cask" | "homebrew" => {
            if std::env::consts::ARCH == "aarch64" {
                "Homebrew ist nicht installiert – `mpkg setup` richtet es ein.".to_owned()
            } else {
                "Homebrew (und damit Apps/Casks) nutzt mpkg nur auf Apple Silicon. Auf Intel baut Homebrew keine Pakete mehr.".to_owned()
            }
        }
        "linuxvm" => "Die VM ist noch nicht angelegt – `mpkg vm create`.".to_owned(),
        _ => format!("Backend `{id}` ist nicht eingerichtet – `mpkg setup` oder `mpkg doctor`."),
    }
}

/// Zusatzhinweis zur Verfügbarkeit für die Ausgabe.
pub fn note(a: Availability) -> &'static str {
    match a {
        Availability::Binary => "",
        Availability::Unknown => "  [Binärpaket nicht prüfbar]",
        Availability::OlderBinary => {
            "  [⏳ Binärpaket nur für die Vorversion – jetzt Build aus Quellcode, oder später erneut versuchen]"
        }
        Availability::SourceOnly => "  [⚠ kein Binärpaket – wird aus dem Quellcode gebaut]",
        Availability::NotFound => "  [nicht gefunden]",
    }
}
