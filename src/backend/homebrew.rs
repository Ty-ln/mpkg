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

/// Hauptversion aus "14" bzw. "10.15" (→ 10).
fn major_of(v: &serde_json::Value) -> Option<u32> {
    v.as_str()?.split('.').next()?.parse().ok()
}

/// Passt die App laut `depends_on.macos` nicht zu `current`, kommt der Grund zurück.
/// Homebrew nutzt dort nur zwei Formen: {">=": ["14"]} und {"==": ["11", "12", …]}.
/// Unbekannte Formen lassen wir durch – `brew install` prüft sie dann selbst.
fn macos_mismatch(cask: &serde_json::Value, current: u32) -> Option<String> {
    let req = &cask["depends_on"]["macos"];
    if let Some(min) = req[">="]
        .as_array()
        .and_then(|v| v.iter().filter_map(major_of).min())
    {
        return (current < min).then(|| format!("braucht mindestens macOS {min}"));
    }
    let allowed: Vec<u32> = req["=="].as_array()?.iter().filter_map(major_of).collect();
    let list = allowed
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    (!allowed.is_empty() && !allowed.contains(&current))
        .then(|| format!("läuft nur auf macOS {list}"))
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
        if let Some(reason) = macos_mismatch(&json["casks"][0], current) {
            bail!("Die App `{name}` {reason}, hier läuft macOS {current}.");
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
    use super::{has_usable_bottle, macos_mismatch};
    use serde_json::json;

    #[test]
    fn checks_macos_requirement_of_cask() {
        let min14 = json!({"depends_on": {"macos": {">=": ["14"]}}});
        assert_eq!(
            macos_mismatch(&min14, 13).as_deref(),
            Some("braucht mindestens macOS 14")
        );
        assert_eq!(macos_mismatch(&min14, 14), None);
        assert_eq!(
            macos_mismatch(&json!({"depends_on": {"macos": {">=": ["10.15"]}}}), 12),
            None
        );

        let only = json!({"depends_on": {"macos": {"==": ["14", "15"]}}});
        assert_eq!(macos_mismatch(&only, 15), None);
        assert_eq!(
            macos_mismatch(&only, 26).as_deref(),
            Some("läuft nur auf macOS 14, 15")
        );

        assert_eq!(
            macos_mismatch(&json!({"depends_on": {"macos": {}}}), 26),
            None
        );
        assert_eq!(
            macos_mismatch(&json!({"depends_on": {"macos": {"<": ["14"]}}}), 26),
            None
        );
        assert_eq!(macos_mismatch(&json!({}), 26), None);
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
