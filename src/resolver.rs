//! Wählt für jedes Paket das passende Backend und den dortigen Namen.

use crate::backend::{Availability, Backend};
use crate::mapping::MappingDb;
use crate::state::State;
use anyhow::{Result, anyhow, bail};
use std::cell::RefCell;

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
        Self { mapping, backends, state: RefCell::new(state) }
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
                "Kein Backend eingerichtet – MacPorts ist nicht installiert.\n\
                 Führe `mpkg setup` aus, um es automatisch einzurichten."
            );
        }
        Ok(())
    }

    /// Reihenfolge: erzwungenes Backend → Binärpaket → nicht prüfbar → nur Quellcode.
    pub fn choose(&self, name: &str, forced: Option<&str>) -> Result<Choice<'_>> {
        if let Some(id) = forced {
            let backend = self.backend_by_id(id)?;
            let native = self.mapping.name_for(name, backend.id());
            let availability = backend.availability(&native)?;
            return Ok(Choice { backend, native, availability });
        }

        self.ensure_any_ready()?;

        let rank = |a: Availability| match a {
            Availability::Binary => 0,
            Availability::Unknown => 1,
            Availability::OlderBinary => 2,
            Availability::SourceOnly => 3,
            Availability::NotFound => 4,
        };

        let mut best: Option<Choice> = None;
        for backend in self.ready_backends() {
            let native = self.mapping.name_for(name, backend.id());
            let availability = backend.availability(&native)?;
            if availability == Availability::Binary {
                return Ok(Choice { backend, native, availability });
            }
            if availability != Availability::NotFound
                && best.as_ref().is_none_or(|b| rank(availability) < rank(b.availability))
            {
                best = Some(Choice { backend, native, availability });
            }
        }
        best.ok_or_else(|| anyhow!("Paket `{name}` wurde in keinem Backend gefunden"))
    }

    pub fn install(&self, names: &[String], forced: Option<&str>, dry_run: bool) -> Result<()> {
        for name in names {
            if forced.is_none() {
                if let Some(i) = self.state.borrow().get(name) {
                    println!("{name} ist bereits installiert ({} über {})", i.native, i.backend);
                    continue;
                }
            }
            let c = self.choose(name, forced)?;
            println!("{name} → {} (über {}){}", c.native, c.backend.id(), note(c.availability));
            c.backend.install(&c.native, dry_run)?;
            if !dry_run {
                let mut state = self.state.borrow_mut();
                state.record(name, c.backend.id(), &c.native);
                state.save()?;
            }
        }
        Ok(())
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
            println!("  {:<10} {native:<20} {:?}", backend.id(), backend.availability(&native)?);
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
            let state = if backend.is_ready() { "bereit" } else { "nicht eingerichtet" };
            println!("{:<10} {state}", backend.id());
        }
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
