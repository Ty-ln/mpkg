//! Debian-VM für das Backend `linuxvm`: anlegen, starten, stoppen, Befehle ausführen.
//!
//! Die VM selbst betreibt der Swift-Helfer `mpkg-vm` (helpers/mpkg-vm). Alles andere
//! passiert hier:
//!   - Image: Debian 13 genericcloud (amd64 auf Intel, arm64 auf Apple Silicon), SHA512-geprüft
//!   - Provisionierung: cloud-init (NoCloud-Seed als ISO, erzeugt mit `hdiutil`)
//!   - Zugriff: ssh mit eigenem Schlüssel; IP aus den DHCP-Leases von macOS (NAT)
//!   - Das Home-Verzeichnis ist per virtiofs am selben Pfad im Gast eingehängt,
//!     daher funktionieren relative Pfade und das aktuelle Verzeichnis wie gewohnt.
//!
//! Dateien in ~/.mpkg/vm: disk.raw, seed.iso, mac, id_ed25519, known_hosts, pid, last-used, *.log

use crate::backend::run;
use anyhow::{Context, Result, bail};
use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const IMAGE_BASE: &str = "https://cloud.debian.org/images/cloud/trixie/latest";
const HELPER: &str = "mpkg-vm";
const GUEST_USER: &str = "mpkg";
const DISK_BYTES: u64 = 20 * 1024 * 1024 * 1024;
/// Erster Start inkl. cloud-init und `apt-get update` dauert deutlich länger als spätere.
const BOOT_TIMEOUT: Duration = Duration::from_secs(300);
const LEASES: &str = "/var/db/dhcpd_leases";

pub fn dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME nicht gesetzt")?;
    Ok(PathBuf::from(home).join(".mpkg/vm"))
}

/// Swift-Helfer: $MPKG_VM_HELPER, neben der mpkg-Binary oder im PATH.
pub fn helper() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MPKG_VM_HELPER") {
        return Some(PathBuf::from(p));
    }
    let sibling = std::env::current_exe().ok()?.with_file_name(HELPER);
    if sibling.is_file() {
        return Some(sibling);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(HELPER))
        .find(|p| p.is_file())
}

/// Voraussetzungen fürs VM-Backend; Err mit Begründung, wenn etwas fehlt.
pub fn supported() -> Result<PathBuf> {
    let major = crate::setup::macos_major()?;
    if major < 13 {
        bail!("Das VM-Backend braucht derzeit macOS 13 oder neuer (dieses System: macOS {major}).");
    }
    helper().with_context(|| format!("Swift-Helfer `{HELPER}` nicht gefunden"))
}

/// seed.iso wird als Letztes geschrieben – existiert sie, ist die VM vollständig angelegt.
pub fn is_created() -> bool {
    dir().is_ok_and(|d| d.join("seed.iso").exists())
}

fn debian_arch() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        other => bail!("Architektur {other} wird nicht unterstützt"),
    }
}

fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("konnte `{program}` nicht starten"))?;
    if !out.status.success() {
        bail!("`{program} {}` fehlgeschlagen: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str().with_context(|| format!("Pfad ist kein UTF-8: {}", p.display()))
}

/// Lädt das Debian-Image, prüft es und legt Schlüssel, MAC-Adresse und cloud-init-Seed an.
pub fn create(dry_run: bool) -> Result<()> {
    supported()?;
    let dir = dir()?;
    if is_created() {
        println!("✓ VM ist bereits angelegt ({})", dir.display());
        return Ok(());
    }
    let archive = format!("debian-13-genericcloud-{}.tar.xz", debian_arch()?);
    let url = format!("{IMAGE_BASE}/{archive}");
    if dry_run {
        println!("→ würde laden: {url} (~400 MB) und in {} einrichten", dir.display());
        return Ok(());
    }
    // Reste eines abgebrochenen Versuchs verwerfen.
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("{} nicht löschbar", dir.display()))?;
    }
    std::fs::create_dir_all(&dir)?;

    let tar = dir.join(&archive);
    run("curl", &["-fL", "--progress-bar", "-o", path_str(&tar)?, &url], false)?;
    verify_sha512(&tar, &archive)?;
    run("tar", &["-xJf", path_str(&tar)?, "-C", path_str(&dir)?, "disk.raw"], false)?;
    std::fs::remove_file(&tar)?;

    // Sparse vergrößern; cloud-init (growpart) erweitert das Root-Dateisystem beim ersten Start.
    let disk = std::fs::OpenOptions::new().write(true).open(dir.join("disk.raw"))?;
    if disk.metadata()?.len() < DISK_BYTES {
        disk.set_len(DISK_BYTES)?;
    }

    let key = dir.join("id_ed25519");
    run("ssh-keygen", &["-q", "-t", "ed25519", "-N", "", "-C", "mpkg-vm", "-f", path_str(&key)?], false)?;
    std::fs::write(dir.join("mac"), random_mac()?)?;
    write_seed(&dir)?;
    println!("✓ VM angelegt: {}", dir.display());
    Ok(())
}

fn verify_sha512(file: &Path, name: &str) -> Result<()> {
    let sums = output("curl", &["-fsSL", &format!("{IMAGE_BASE}/SHA512SUMS")])?;
    let expected = sums
        .lines()
        .find_map(|l| l.strip_suffix(name).map(|h| h.trim().to_owned()))
        .with_context(|| format!("{name} fehlt in SHA512SUMS"))?;
    let actual = output("shasum", &["-a", "512", path_str(file)?])?;
    if actual.split_whitespace().next() != Some(expected.as_str()) {
        let _ = std::fs::remove_file(file);
        bail!("Prüfsumme von {name} stimmt nicht – Download verworfen.");
    }
    println!("✓ Prüfsumme (SHA512) stimmt");
    Ok(())
}

/// Lokal verwaltete Unicast-MAC (zweites Bit des ersten Bytes gesetzt).
fn random_mac() -> Result<String> {
    let mut bytes = [0u8; 6];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut bytes)?;
    bytes[0] = (bytes[0] & 0xfc) | 0x02;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":"))
}

fn write_seed(dir: &Path) -> Result<()> {
    let home = std::env::var("HOME").context("HOME nicht gesetzt")?;
    let uid = output("id", &["-u"])?.trim().to_owned();
    let pubkey = std::fs::read_to_string(dir.join("id_ed25519.pub"))?;
    let seed = dir.join("seed");
    std::fs::create_dir_all(&seed)?;
    let mac = std::fs::read_to_string(dir.join("mac"))?;
    std::fs::write(seed.join("meta-data"), "instance-id: mpkg-vm-1\nlocal-hostname: mpkg-vm\n")?;
    std::fs::write(seed.join("user-data"), user_data(&home, &uid, pubkey.trim()))?;
    std::fs::write(seed.join("network-config"), network_config(mac.trim()))?;
    let iso = dir.join("seed.iso");
    run(
        "hdiutil",
        &["makehybrid", "-quiet", "-iso", "-joliet", "-default-volume-name", "cidata", "-o", path_str(&iso)?, path_str(&seed)?],
        false,
    )
}

/// cloud-init: Nutzer mit derselben UID wie auf dem Mac (Dateirechte im geteilten Home),
/// Home per virtiofs am selben Pfad einhängen.
fn user_data(home: &str, uid: &str, pubkey: &str) -> String {
    format!(
        "#cloud-config
hostname: mpkg-vm
users:
  - name: {GUEST_USER}
    uid: {uid}
    shell: /bin/bash
    sudo: \"ALL=(ALL) NOPASSWD:ALL\"
    ssh_authorized_keys:
      - {pubkey}
mounts:
  - [home, \"{home}\", virtiofs, \"defaults,nofail\", \"0\", \"0\"]
package_update: true
"
    )
}

/// DHCP mit der MAC als Client-ID: systemd-networkd schickt sonst eine DUID, und macOS
/// trägt dann keine MAC in /var/db/dhcpd_leases ein – die IP wäre nicht auffindbar.
fn network_config(mac: &str) -> String {
    format!(
        "version: 2
ethernets:
  eth0:
    match:
      macaddress: \"{mac}\"
    set-name: eth0
    dhcp4: true
    dhcp-identifier: mac
"
    )
}

fn pid() -> Option<u32> {
    std::fs::read_to_string(dir().ok()?.join("pid")).ok()?.trim().parse().ok()
}

fn alive(pid: u32) -> bool {
    // shortcut: kill -0 prüft nur, ob die PID existiert; bei PID-Wiederverwendung nach einem
    // Neustart des Macs falsch positiv – dann den Prozessnamen prüfen.
    Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn is_running() -> bool {
    pid().is_some_and(alive)
}

/// Startet die VM im Hintergrund (falls nötig) und wartet, bis ssh antwortet.
/// Berührt last-used, damit der Helfer die VM nicht in den Ruhemodus schickt.
pub fn ensure_running() -> Result<String> {
    if !is_created() {
        bail!("Die VM ist noch nicht angelegt – `mpkg vm create`.");
    }
    std::fs::write(dir()?.join("last-used"), "")?;
    if !is_running() {
        start_helper()?;
    }
    wait_ready()
}

fn start_helper() -> Result<()> {
    let helper = supported()?;
    let dir = dir()?;
    let home = std::env::var("HOME").context("HOME nicht gesetzt")?;
    eprintln!("Starte die Debian-VM …");
    let child = Command::new(helper)
        .arg("run")
        .arg(&dir)
        .arg(&home)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(dir.join("console.log"))?)
        .stderr(std::fs::File::create(dir.join("helper.log"))?)
        // Eigene Prozessgruppe: Strg-C im Terminal beendet die VM nicht mit.
        .process_group(0)
        .spawn()
        .context("Swift-Helfer konnte nicht gestartet werden")?;
    std::fs::write(dir.join("pid"), child.id().to_string())?;
    Ok(())
}

fn wait_ready() -> Result<String> {
    let mac = std::fs::read_to_string(dir()?.join("mac"))?;
    let started = Instant::now();
    while started.elapsed() < BOOT_TIMEOUT {
        if !is_running() {
            let log = std::fs::read_to_string(dir()?.join("helper.log")).unwrap_or_default();
            bail!("Die VM hat sich beendet:\n{}", log.trim());
        }
        let leases = std::fs::read_to_string(LEASES).unwrap_or_default();
        if let Some(ip) = lease_ip(&leases, mac.trim()) {
            // Erst fertig, wenn cloud-init durch ist (sonst blockiert apt noch).
            let ready = ssh_command(&ip, "cloud-init status --wait >/dev/null 2>&1; true", false)?
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if ready {
                return Ok(ip);
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    bail!("Die VM ist nach {} s nicht erreichbar – siehe {}", BOOT_TIMEOUT.as_secs(), dir()?.join("console.log").display())
}

/// IP-Adresse zur MAC aus /var/db/dhcpd_leases. macOS schreibt die MAC ohne führende
/// Nullen ("hw_address=1,2:3:a4:…"), daher Byte für Byte vergleichen.
fn lease_ip(leases: &str, mac: &str) -> Option<String> {
    let parse = |s: &str| -> Option<Vec<u8>> { s.split(':').map(|b| u8::from_str_radix(b, 16).ok()).collect() };
    let wanted = parse(mac)?;
    leases.split('}').find_map(|block| {
        let field = |key: &str| block.lines().find_map(|l| l.trim().strip_prefix(key).map(str::to_owned));
        let hw = field("hw_address=")?;
        let hw = hw.split_once(',').map_or(hw.as_str(), |(_, m)| m);
        (parse(hw)? == wanted).then(|| field("ip_address=")).flatten()
    })
}

fn ssh_command(ip: &str, remote: &str, tty: bool) -> Result<Command> {
    let dir = dir()?;
    let mut cmd = Command::new("ssh");
    cmd.arg("-i").arg(dir.join("id_ed25519"))
        .arg("-o").arg(format!("UserKnownHostsFile={}", path_str(&dir.join("known_hosts"))?))
        .args(["-o", "StrictHostKeyChecking=accept-new", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "LogLevel=ERROR"]);
    cmd.arg(if tty { "-t" } else { "-T" });
    cmd.arg(format!("{GUEST_USER}@{ip}")).arg(remote);
    Ok(cmd)
}

/// Für die entfernte Shell sicher quotieren.
pub fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Befehl (bereits quotiert) im Gast ausführen, im aktuellen Verzeichnis, falls es im
/// geteilten Home liegt.
pub fn exec_raw(remote: &str) -> Result<ExitStatus> {
    let ip = ensure_running()?;
    let cwd = std::env::current_dir().ok().and_then(|d| d.to_str().map(quote)).unwrap_or_default();
    let line = format!("cd {cwd} 2>/dev/null; {remote}");
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    ssh_command(&ip, &line, tty)?.status().context("ssh fehlgeschlagen")
}

/// Befehl im Gast ausführen und die Ausgabe zurückgeben.
pub fn exec_output(remote: &str) -> Result<(bool, String)> {
    let ip = ensure_running()?;
    let out = ssh_command(&ip, remote, false)?.output().context("ssh fehlgeschlagen")?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}

pub fn exec(args: &[String]) -> Result<i32> {
    let line = args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
    Ok(exec_raw(&format!("exec {line}"))?.code().unwrap_or(1))
}

pub fn stop() -> Result<()> {
    match pid().filter(|p| alive(*p)) {
        Some(p) => {
            run("kill", &["-TERM", &p.to_string()], false)?;
            println!("✓ VM wird heruntergefahren");
        }
        None => println!("VM läuft nicht."),
    }
    Ok(())
}

pub fn status() -> Result<()> {
    let state = match (is_created(), is_running()) {
        (false, _) => "nicht angelegt (`mpkg vm create`)",
        (true, false) => "gestoppt",
        (true, true) => "läuft",
    };
    println!("VM: {state}");
    if let Err(e) = supported() {
        println!("  ! {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{lease_ip, quote};

    #[test]
    fn finds_lease_without_leading_zeros() {
        let leases = "{\n\tname=other\n\tip_address=192.168.64.2\n\thw_address=1,aa:bb:cc:dd:ee:ff\n}\n{\n\tname=mpkg-vm\n\tip_address=192.168.64.7\n\thw_address=1,2:a:3c:4:5:6\n\tlease=0x1\n}\n";
        assert_eq!(lease_ip(leases, "02:0a:3c:04:05:06").as_deref(), Some("192.168.64.7"));
        assert_eq!(lease_ip(leases, "02:00:00:00:00:01"), None);
    }

    #[test]
    fn quotes_for_remote_shell() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's; rm -rf /"), r"'it'\''s; rm -rf /'");
    }
}
