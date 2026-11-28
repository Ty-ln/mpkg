//! Backend Homebrew – nur auf Apple Silicon.
//!
//! Auf Intel baut Homebrew keine Bottles mehr; dort bleibt es bei MacPorts. Auf Apple
//! Silicon ist Homebrew das erste Backend, MacPorts (falls installiert) und die VM folgen.
//! brew läuft bewusst ohne sudo – /opt/homebrew gehört dem Nutzer.

use super::{Availability, Backend, run};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

pub const BREW: &str = "/opt/homebrew/bin/brew";

/// Bottle-Kennungen von alt nach neu (Schlüssel im JSON: "arm64_<name>").
const CODENAMES: [(u32, &str); 6] = [
    (11, "big_sur"),
    (12, "monterey"),
    (13, "ventura"),
    (14, "sonoma"),
    (15, "sequoia"),
    (26, "tahoe"),
];

pub struct Homebrew;

impl Homebrew {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct Info {
    formulae: Vec<Formula>,
}

#[derive(Deserialize)]
struct Formula {
    #[serde(default)]
    bottle: Bottles,
}

#[derive(Deserialize, Default)]
struct Bottles {
    stable: Option<BottleSpec>,
}

#[derive(Deserialize)]
struct BottleSpec {
    #[serde(default)]
    files: std::collections::HashMap<String, serde::de::IgnoredAny>,
}

/// Position einer macOS-Version in CODENAMES; neuere, noch unbekannte Versionen zählen
/// als neuer als alle bekannten.
fn os_rank(major: u32) -> usize {
    CODENAMES
        .iter()
        .position(|(m, _)| *m == major)
        .unwrap_or_else(|| {
            if major > CODENAMES[CODENAMES.len() - 1].0 {
                CODENAMES.len()
            } else {
                0
            }
        })
}

/// Passt eine der Bottles auf dieses System? Homebrew nimmt auch Bottles älterer
/// macOS-Versionen, aber nie neuerer.
fn has_usable_bottle<'a>(keys: impl IntoIterator<Item = &'a str>, major: u32) -> bool {
    let current = os_rank(major);
    keys.into_iter().any(|key| {
        key == "all"
            || key
                .strip_prefix("arm64_")
                .and_then(|name| CODENAMES.iter().position(|(_, n)| *n == name))
                .is_some_and(|rank| rank <= current)
    })
}

/// Mindest-macOS (Hauptversion) aus `depends_on.macos`, z. B. {">=": ["14"]} → 14.
/// shortcut: nur ">=" wird ausgewertet (andere Vergleiche nutzt kaum ein Cask) – bei Bedarf ergänzen.
fn min_macos(cask: &serde_json::Value) -> Option<u32> {
    cask["depends_on"]["macos"][">="]
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str()?.split('.').next()?.parse().ok())
        .min()
}

fn brew(args: &[&str], dry_run: bool) -> Result<()> {
    run(BREW, args, dry_run)
}

impl Backend for Homebrew {
    fn id(&self) -> &'static str {
        "homebrew"
    }

    fn is_ready(&self) -> bool {
        std::env::consts::ARCH == "aarch64" && Path::new(BREW).exists()
    }

    fn availability(&self, name: &str) -> Result<Availability> {
        let out = Command::new(BREW)
            .args(["info", "--json=v2", "--formula", name])
            .output()
            .context("`brew info` fehlgeschlagen")?;
        if !out.status.success() {
            return Ok(Availability::NotFound);
        }
        let info: Info =
            serde_json::from_slice(&out.stdout).context("Antwort von `brew info` nicht lesbar")?;
        let Some(formula) = info.formulae.first() else {
            return Ok(Availability::NotFound);
        };
        let keys = formula
            .bottle
            .stable
            .iter()
            .flat_map(|s| s.files.keys().map(String::as_str));
        let major = crate::setup::macos_major()?;
        Ok(if has_usable_bottle(keys, major) {
            Availability::Binary
        } else {
            Availability::SourceOnly
        })
    }

    fn install(&self, name: &str, dry_run: bool) -> Result<()> {
        brew(&["install", "--formula", name], dry_run)
    }

    fn remove(&self, name: &str, dry_run: bool) -> Result<()> {
        brew(&["uninstall", "--formula", name], dry_run)
    }

    fn search(&self, query: &str) -> Result<Vec<String>> {
        let out = Command::new(BREW)
            .args(["search", "--formula", query])
            .output()
            .context("`brew search` fehlgeschlagen")?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("==>"))
            .map(str::to_owned)
            .collect())
    }

    fn list(&self) -> Result<()> {
        brew(&["leaves"], false)
    }

    fn update(&self, dry_run: bool) -> Result<()> {
        brew(&["update"], dry_run)
    }

    fn upgrade(&self, dry_run: bool) -> Result<()> {
        brew(&["upgrade", "--formula"], dry_run)
    }
}

/// Casks: fertige Mac-Apps (Firefox, VS Code …) über `brew --cask`. Nur auf Wunsch
/// (`mpkg install --cask`), weil manche Namen auch Formeln sind (z. B. `docker`).
pub struct HomebrewCask;

impl Backend for HomebrewCask {
    fn id(&self) -> &'static str {
        "homebrew-cask"
    }

    fn on_request_only(&self) -> bool {
        true
    }

    fn is_ready(&self) -> bool {
        Homebrew.is_ready()
    }

    /// Casks sind immer fertige Apps; das Mindest-macOS prüft `install`.
    fn availability(&self, name: &str) -> Result<Availability> {
        let ok = Command::new(BREW)
            .args(["info", "--json=v2", "--cask", name])
            .output()
            .context("`brew info --cask` fehlgeschlagen")?
            .status
            .success();
        Ok(if ok {
            Availability::Binary
        } else {
            Availability::NotFound
        })
    }

    fn install(&self, name: &str, dry_run: bool) -> Result<()> {
        let out = Command::new(BREW)
            .args(["info", "--json=v2", "--cask", name])
            .output()
            .context("`brew info --cask` fehlgeschlagen")?;
        let json: serde_json::Value = serde_json::from_slice(&out.stdout)
            .context("Antwort von `brew info --cask` nicht lesbar")?;
        let current = crate::setup::macos_major()?;
        if let Some(min) = min_macos(&json["casks"][0]).filter(|min| *min > current) {
            bail!("Die App `{name}` braucht mindestens macOS {min}, hier läuft macOS {current}.");
        }
        brew(&["install", "--cask", name], dry_run)
    }

    fn remove(&self, name: &str, dry_run: bool) -> Result<()> {
        brew(&["uninstall", "--cask", name], dry_run)
    }

    fn search(&self, query: &str) -> Result<Vec<String>> {
        let out = Command::new(BREW)
            .args(["search", "--cask", query])
            .output()
            .context("`brew search --cask` fehlgeschlagen")?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("==>"))
            .map(str::to_owned)
            .collect())
    }

    fn list(&self) -> Result<()> {
        brew(&["list", "--cask"], false)
    }

    /// `brew update` erledigt schon das Formel-Backend – nicht doppelt ausführen.
    fn update(&self, _dry_run: bool) -> Result<()> {
        Ok(())
    }

    fn upgrade(&self, dry_run: bool) -> Result<()> {
        brew(&["upgrade", "--cask"], dry_run)
    }
}

#[cfg(test)]
mod tests {
    use super::{has_usable_bottle, min_macos};
    use serde_json::json;

    #[test]
    fn reads_minimum_macos_of_cask() {
        assert_eq!(
            min_macos(&json!({"depends_on": {"macos": {">=": ["14"]}}})),
            Some(14)
        );
        assert_eq!(
            min_macos(&json!({"depends_on": {"macos": {">=": ["10.15"]}}})),
            Some(10)
        );
        assert_eq!(min_macos(&json!({"depends_on": {"macos": {}}})), None);
        assert_eq!(min_macos(&json!({})), None);
    }

    #[test]
    fn bottle_for_same_or_older_macos_is_usable() {
        assert!(has_usable_bottle(["arm64_tahoe"], 26));
        assert!(has_usable_bottle(["arm64_sonoma", "sonoma"], 15)); // ältere Bottle passt
        assert!(has_usable_bottle(["all"], 13));
        assert!(has_usable_bottle(["arm64_tahoe"], 27)); // noch unbekannte, neuere Version
    }

    #[test]
    fn bottle_for_newer_macos_or_intel_is_not_usable() {
        assert!(!has_usable_bottle(["arm64_tahoe"], 15));
        assert!(!has_usable_bottle(["tahoe", "x86_64_linux"], 26));
        assert!(!has_usable_bottle([], 26));
    }
}
