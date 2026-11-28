//! Zuordnung von Paketnamen zwischen Debian, Homebrew und MacPorts.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Mitgelieferte Basis-Datenbank.
const BUILTIN: &str = include_str!("../mapping/packages.toml");

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Entry {
    pub macports: Option<String>,
    pub homebrew: Option<String>,
    pub debian: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MappingDb {
    #[serde(default)]
    packages: HashMap<String, Entry>,
}

/// Regel für versionierte Homebrew-Namen → MacPorts:
///   python@3.12    → python312
///   python-tk@3.14 → py314-tkinter
///   node@22        → nodejs22
///   php@8.3        → php83, postgresql@16 → postgresql16, …
fn macports_versioned(name: &str) -> Option<String> {
    let (base, version) = name.split_once('@')?;
    let ver: String = version.chars().filter(|c| *c != '.').collect();
    Some(match base {
        "python" => format!("python{ver}"),
        "python-tk" => format!("py{ver}-tkinter"),
        "node" => format!("nodejs{ver}"),
        _ => format!("{base}{ver}"),
    })
}

impl MappingDb {
    /// Lädt die eingebaute DB und legt `~/.mpkg/mapping.toml` (falls vorhanden) darüber.
    pub fn load_default() -> Result<Self> {
        let mut db: MappingDb =
            toml::from_str(BUILTIN).context("eingebaute Mapping-DB ist ungültig")?;

        if let Some(home) = std::env::var_os("HOME") {
            let user_file = PathBuf::from(home).join(".mpkg/mapping.toml");
            if user_file.exists() {
                let text = std::fs::read_to_string(&user_file)?;
                let user: MappingDb = toml::from_str(&text)
                    .with_context(|| format!("{} ist ungültig", user_file.display()))?;
                db.packages.extend(user.packages);
            }
        }
        Ok(db)
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.packages.get(name)
    }

    /// Name des Pakets im jeweiligen Backend.
    /// Reihenfolge: expliziter Eintrag → Regel für versionierte Namen → Name unverändert.
    pub fn name_for(&self, name: &str, backend: &str) -> String {
        let explicit = self.packages.get(name).and_then(|e| match backend {
            "macports" => e.macports.clone(),
            "homebrew" => e.homebrew.clone(),
            "linuxvm" => e.debian.clone(),
            _ => None,
        });
        explicit
            .or_else(|| match backend {
                "macports" => macports_versioned(name),
                _ => None,
            })
            .unwrap_or_else(|| name.to_owned())
    }

    /// Homebrew-Name → kanonischer mpkg-Name.
    pub fn canonical_from_brew(&self, brew_name: &str) -> String {
        self.packages
            .iter()
            .find(|(_, e)| e.homebrew.as_deref() == Some(brew_name))
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| brew_name.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::macports_versioned;

    #[test]
    fn versioned_names() {
        assert_eq!(
            macports_versioned("python@3.12").as_deref(),
            Some("python312")
        );
        assert_eq!(
            macports_versioned("python-tk@3.14").as_deref(),
            Some("py314-tkinter")
        );
        assert_eq!(macports_versioned("node@22").as_deref(), Some("nodejs22"));
        assert_eq!(macports_versioned("php@8.3").as_deref(), Some("php83"));
        assert_eq!(macports_versioned("ripgrep"), None);
    }
}
