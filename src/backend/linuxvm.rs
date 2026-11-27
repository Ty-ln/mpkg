//! Fallback-Backend: Debian in einer leichten VM (Phase 2).
//!
//! VM-Verwaltung und ssh-Zugriff stecken in `crate::vm`. Hier nur die Paketbefehle:
//! `apt-get` im Gast, dazu Shims in ~/.mpkg/bin, die die Programme des Pakets
//! über `mpkg vm exec` aufrufen (z. B. `htop` → läuft in der VM, Terminal bleibt das des Macs).

use super::{Availability, Backend};
use crate::vm::{self, quote};
use anyhow::{Context, Result, bail};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const SHIM_MARKER: &str = "angelegt von mpkg";

pub struct LinuxVm;

impl LinuxVm {
    pub fn new() -> Self {
        Self
    }
}

/// Debian-Paketnamen: Kleinbuchstaben, Ziffern, + - . – alles andere ablehnen,
/// bevor der Name in einer entfernten Shell landet.
fn valid_package(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+-.".contains(c));
    if !ok {
        bail!("ungültiger Debian-Paketname `{name}`");
    }
    Ok(())
}

fn apt(args: &str, dry_run: bool) -> Result<()> {
    let line = format!("sudo DEBIAN_FRONTEND=noninteractive apt-get {args}");
    println!("→ [vm] {line}");
    if dry_run {
        return Ok(());
    }
    let status = vm::exec_raw(&line)?;
    if !status.success() {
        bail!("`apt-get` in der VM wurde mit {status} beendet");
    }
    Ok(())
}

fn shim_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME nicht gesetzt")?;
    Ok(PathBuf::from(home).join(".mpkg/bin"))
}

/// Programme, die ein installiertes Paket mitbringt (/usr/bin, /usr/sbin, /bin, /sbin, /usr/games).
fn package_binaries(name: &str) -> Result<Vec<String>> {
    let (ok, out) = vm::exec_output(&format!("dpkg -L {}", quote(name)))?;
    Ok(if ok { binaries_from_dpkg_list(&out) } else { Vec::new() })
}

fn binaries_from_dpkg_list(list: &str) -> Vec<String> {
    list.lines()
        .filter_map(|path| {
            let (dir, file) = path.rsplit_once('/')?;
            let is_bin = matches!(dir, "/usr/bin" | "/usr/sbin" | "/bin" | "/sbin" | "/usr/games");
            (is_bin && !file.is_empty()).then(|| file.to_owned())
        })
        .collect()
}

/// Shim-Skript: ruft das Programm über mpkg in der VM auf.
fn shim_script(mpkg: &str, program: &str) -> String {
    format!(
        "#!/bin/sh\n# {SHIM_MARKER} (Backend linuxvm)\nexec {} vm exec -- {} \"$@\"\n",
        quote(mpkg),
        quote(program)
    )
}

fn is_our_shim(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.contains(SHIM_MARKER))
}

fn write_shims(binaries: &[String]) -> Result<()> {
    let dir = shim_dir()?;
    std::fs::create_dir_all(&dir)?;
    let mpkg = std::env::current_exe().context("Pfad von mpkg unbekannt")?;
    let mpkg = mpkg.to_str().context("Pfad von mpkg ist kein UTF-8")?;
    for bin in binaries {
        let path = dir.join(bin);
        // Nichts überschreiben, das nicht von uns stammt (z. B. mpkg selbst).
        if path.exists() && !is_our_shim(&path) {
            println!("  ! {} existiert bereits – kein Shim angelegt", path.display());
            continue;
        }
        std::fs::write(&path, shim_script(mpkg, bin))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        println!("  ✓ {bin} → {}", path.display());
    }
    Ok(())
}

fn remove_shims(binaries: &[String]) -> Result<()> {
    let dir = shim_dir()?;
    for path in binaries.iter().map(|b| dir.join(b)) {
        if is_our_shim(&path) {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

impl Backend for LinuxVm {
    fn id(&self) -> &'static str {
        "linuxvm"
    }

    fn is_fallback(&self) -> bool {
        true
    }

    /// Bereit = VM angelegt und Voraussetzungen erfüllt. Gestartet wird erst bei Bedarf.
    fn is_ready(&self) -> bool {
        vm::is_created() && vm::supported().is_ok()
    }

    fn availability(&self, name: &str) -> Result<Availability> {
        if valid_package(name).is_err() {
            return Ok(Availability::NotFound);
        }
        let query = format!("apt-cache show --no-all-versions {} 2>/dev/null", quote(name));
        let (ok, out) = vm::exec_output(&query)?;
        // Debian liefert immer fertige Binärpakete.
        Ok(if ok && !out.trim().is_empty() { Availability::Binary } else { Availability::NotFound })
    }

    fn install(&self, name: &str, dry_run: bool) -> Result<()> {
        valid_package(name)?;
        apt(&format!("install -y {}", quote(name)), dry_run)?;
        if !dry_run {
            write_shims(&package_binaries(name)?)?;
        }
        Ok(())
    }

    fn remove(&self, name: &str, dry_run: bool) -> Result<()> {
        valid_package(name)?;
        let binaries = if dry_run { Vec::new() } else { package_binaries(name)? };
        apt(&format!("remove -y {}", quote(name)), dry_run)?;
        remove_shims(&binaries)
    }

    /// Sucht nur, wenn die VM schon läuft – `mpkg search` soll sie nicht extra hochfahren.
    fn search(&self, query: &str) -> Result<Vec<String>> {
        if !vm::is_running() {
            return Ok(Vec::new());
        }
        let (_, out) = vm::exec_output(&format!("apt-cache search --names-only {}", quote(query)))?;
        Ok(out.lines().filter_map(|l| l.split_whitespace().next()).map(str::to_owned).collect())
    }

    fn list(&self) -> Result<()> {
        if !vm::is_running() {
            println!("(VM gestoppt)");
            return Ok(());
        }
        let (_, out) = vm::exec_output("apt-mark showmanual")?;
        print!("{out}");
        Ok(())
    }

    fn update(&self, dry_run: bool) -> Result<()> {
        apt("update -q", dry_run)
    }

    fn upgrade(&self, dry_run: bool) -> Result<()> {
        apt("upgrade -y", dry_run)
    }
}

#[cfg(test)]
mod tests {
    use super::{binaries_from_dpkg_list, shim_script, valid_package};

    #[test]
    fn package_names_are_validated() {
        assert!(valid_package("libc6-dev").is_ok());
        assert!(valid_package("g++").is_ok());
        assert!(valid_package("htop; rm -rf ~").is_err());
        assert!(valid_package("").is_err());
    }

    #[test]
    fn picks_binaries_from_dpkg_list() {
        let list = "/.\n/usr\n/usr/bin\n/usr/bin/htop\n/usr/share/doc/htop/README\n/usr/sbin/foo\n/usr/games/cowsay\n";
        assert_eq!(binaries_from_dpkg_list(list), vec!["htop", "foo", "cowsay"]);
    }

    #[test]
    fn shim_quotes_paths() {
        let s = shim_script("/Users/a b/.mpkg/bin/mpkg", "htop");
        assert!(s.contains("exec '/Users/a b/.mpkg/bin/mpkg' vm exec -- 'htop' \"$@\""));
    }
}
