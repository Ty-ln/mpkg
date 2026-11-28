//! Status-Datenbank: welches Paket wurde über welches Backend installiert.
//!
//! Liegt in `~/.mpkg/state.json`. Wird atomar geschrieben (temporäre Datei + rename),
//! damit ein Abbruch nie eine halbe Datei hinterlässt.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installed {
    /// Backend-ID, z. B. "macports"
    pub backend: String,
    /// Name im Backend, z. B. "python312"
    pub native: String,
    /// Unix-Zeitstempel der Installation
    pub installed_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// Format-Version, falls sich die Struktur später ändert.
    #[serde(default = "current_version")]
    pub version: u32,
    /// Kanonischer mpkg-Name → Installationsinfo (sortiert für stabile Ausgabe).
    #[serde(default)]
    pub packages: BTreeMap<String, Installed>,
}

fn current_version() -> u32 {
    1
}

impl State {
    pub fn path() -> Result<PathBuf> {
        let home = std::env::var_os("HOME").context("HOME nicht gesetzt")?;
        Ok(PathBuf::from(home).join(".mpkg/state.json"))
    }

    /// Lädt den Status; fehlt die Datei, beginnt er leer.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self {
                version: current_version(),
                ..Self::default()
            });
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("{} nicht lesbar", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("{} ist beschädigt", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("{} nicht anlegbar", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("{} nicht schreibbar", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("{} nicht ersetzbar", path.display()))?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Installed> {
        self.packages.get(name)
    }

    pub fn record(&mut self, name: &str, backend: &str, native: &str) {
        let installed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.packages.insert(
            name.to_owned(),
            Installed {
                backend: backend.to_owned(),
                native: native.to_owned(),
                installed_at,
            },
        );
    }

    pub fn forget(&mut self, name: &str) -> Option<Installed> {
        self.packages.remove(name)
    }
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn record_and_forget() {
        let mut s = State::default();
        s.record("python3", "macports", "python313");
        assert_eq!(
            s.get("python3").map(|i| i.native.as_str()),
            Some("python313")
        );
        assert!(s.forget("python3").is_some());
        assert!(s.get("python3").is_none());
    }

    #[test]
    fn roundtrip_json() {
        let mut s = State::default();
        s.record("ripgrep", "macports", "ripgrep");
        let json = serde_json::to_string(&s).unwrap();
        let back: State = serde_json::from_str(&json).unwrap();
        assert_eq!(back.get("ripgrep").unwrap().backend, "macports");
    }
}
