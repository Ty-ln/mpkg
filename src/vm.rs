//! Debian VM for the `linuxvm` backend: create, start, stop, run commands.
//!
//! The Swift helper `mpkg-vm` (helpers/mpkg-vm) runs the VM itself. Everything else
//! happens here:
//!   - Image: Debian 13 genericcloud (amd64 on Intel, arm64 on Apple Silicon), SHA512-verified
//!   - Provisioning: cloud-init (NoCloud seed as an ISO, built with `hdiutil`)
//!   - Access: ssh with its own key; IP from macOS's DHCP leases (NAT)
//!   - The home directory is mounted via virtiofs at the same path in the guest,
//!     so relative paths and the current directory work as usual.
//!
//! Files in ~/.mpkg/vm: disk.raw, seed.iso, mac, id_ed25519, known_hosts, pid, ip, last-used, *.log
//! (on macOS 12 also vmlinuz, initrd, cmdline – see vmboot.rs)

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
/// The first boot including cloud-init and `apt-get update` takes much longer than later ones.
const BOOT_TIMEOUT: Duration = Duration::from_secs(300);
const LEASES: &str = "/var/db/dhcpd_leases";

pub fn dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".mpkg/vm"))
}

/// Swift helper: $MPKG_VM_HELPER, next to the mpkg binary, or in PATH.
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

/// Requirements for the VM backend; Err with the reason if something is missing.
pub fn supported() -> Result<PathBuf> {
    let major = crate::setup::macos_major()?;
    if major < 12 {
        bail!("The VM backend needs macOS 12 or later (this system: macOS {major}).");
    }
    helper().with_context(|| format!("Swift helper `{HELPER}` not found"))
}

/// seed.iso is written last – if it exists, the VM is fully created.
pub fn is_created() -> bool {
    dir().is_ok_and(|d| d.join("seed.iso").exists())
}

fn debian_arch() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        other => bail!("architecture {other} is not supported"),
    }
}

fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("could not start `{program}`"))?;
    if !out.status.success() {
        bail!(
            "`{program} {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .with_context(|| format!("path is not UTF-8: {}", p.display()))
}

/// Downloads and verifies the Debian image, then creates the key, MAC address and cloud-init seed.
pub fn create(dry_run: bool) -> Result<()> {
    supported()?;
    let dir = dir()?;
    if is_created() {
        println!("✓ VM already exists ({})", dir.display());
        return Ok(());
    }
    let archive = format!("debian-13-genericcloud-{}.tar.xz", debian_arch()?);
    let url = format!("{IMAGE_BASE}/{archive}");
    if dry_run {
        println!(
            "→ would download: {url} (~400 MB) and set it up in {}",
            dir.display()
        );
        return Ok(());
    }
    // Discard leftovers of an aborted attempt.
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("cannot delete {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir)?;

    let tar = dir.join(&archive);
    run(
        "curl",
        &["-fL", "--progress-bar", "-o", path_str(&tar)?, &url],
        false,
    )?;
    verify_sha512(&tar, &archive)?;
    run(
        "tar",
        &["-xJf", path_str(&tar)?, "-C", path_str(&dir)?, "disk.raw"],
        false,
    )?;
    std::fs::remove_file(&tar)?;

    // Grow sparsely; cloud-init (growpart) extends the root file system on first boot.
    let disk = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join("disk.raw"))?;
    if disk.metadata()?.len() < DISK_BYTES {
        disk.set_len(DISK_BYTES)?;
    }
    // No EFI boot before macOS 13: provide kernel and initrd for VZLinuxBootLoader.
    if crate::setup::macos_major()? < 13 || std::env::var_os("MPKG_VM_LINUX_BOOT").is_some() {
        crate::vmboot::extract(&dir)?;
    }

    let key = dir.join("id_ed25519");
    run(
        "ssh-keygen",
        &[
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "mpkg-vm",
            "-f",
            path_str(&key)?,
        ],
        false,
    )?;
    std::fs::write(dir.join("mac"), random_mac()?)?;
    write_seed(&dir)?;
    println!("✓ VM created: {}", dir.display());
    Ok(())
}

fn verify_sha512(file: &Path, name: &str) -> Result<()> {
    let sums = output("curl", &["-fsSL", &format!("{IMAGE_BASE}/SHA512SUMS")])?;
    let expected = sums
        .lines()
        .find_map(|l| l.strip_suffix(name).map(|h| h.trim().to_owned()))
        .with_context(|| format!("{name} is missing from SHA512SUMS"))?;
    let actual = output("shasum", &["-a", "512", path_str(file)?])?;
    if actual.split_whitespace().next() != Some(expected.as_str()) {
        let _ = std::fs::remove_file(file);
        bail!("Checksum of {name} does not match – download discarded.");
    }
    println!("✓ Checksum (SHA512) verified");
    Ok(())
}

/// Locally administered unicast MAC (second bit of the first byte set).
fn random_mac() -> Result<String> {
    let mut bytes = [0u8; 6];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut bytes)?;
    bytes[0] = (bytes[0] & 0xfc) | 0x02;
    Ok(bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":"))
}

fn write_seed(dir: &Path) -> Result<()> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    let uid = output("id", &["-u"])?.trim().to_owned();
    let pubkey = std::fs::read_to_string(dir.join("id_ed25519.pub"))?;
    let seed = dir.join("seed");
    std::fs::create_dir_all(&seed)?;
    let mac = std::fs::read_to_string(dir.join("mac"))?;
    std::fs::write(
        seed.join("meta-data"),
        "instance-id: mpkg-vm-1\nlocal-hostname: mpkg-vm\n",
    )?;
    std::fs::write(
        seed.join("user-data"),
        user_data(&home, &uid, pubkey.trim()),
    )?;
    std::fs::write(seed.join("network-config"), network_config(mac.trim()))?;
    let iso = dir.join("seed.iso");
    run(
        "hdiutil",
        &[
            "makehybrid",
            "-quiet",
            "-iso",
            "-joliet",
            "-default-volume-name",
            "cidata",
            "-o",
            path_str(&iso)?,
            path_str(&seed)?,
        ],
        false,
    )
}

/// cloud-init: a user with the same UID as on the Mac (file permissions in the shared home),
/// home mounted via virtiofs at the same path.
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

/// DHCP with the MAC as client ID: otherwise systemd-networkd sends a DUID, macOS then
/// records no MAC in /var/db/dhcpd_leases, and the IP could not be found.
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
    std::fs::read_to_string(dir().ok()?.join("pid"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn alive(pid: u32) -> bool {
    // shortcut: kill -0 only checks that the PID exists; after a Mac reboot a reused PID is a
    // false positive – then also check the process name.
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn is_running() -> bool {
    pid().is_some_and(alive)
}

/// Starts the VM in the background (if needed) and waits until ssh answers.
/// Touches last-used so the helper does not put the VM to sleep.
pub fn ensure_running() -> Result<String> {
    if !is_created() {
        bail!("The VM has not been created yet – run `mpkg vm create`.");
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
    let home = std::env::var("HOME").context("HOME is not set")?;
    // Drop the old IP; it only becomes valid once the new VM answers over ssh.
    let _ = std::fs::remove_file(dir.join("ip"));
    eprintln!("Starting the Debian VM …");
    let child = Command::new(helper)
        .arg("run")
        .arg(&dir)
        .arg(&home)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(dir.join("console.log"))?)
        .stderr(std::fs::File::create(dir.join("helper.log"))?)
        // Own process group: Ctrl-C in the terminal does not take the VM down with it.
        .process_group(0)
        .spawn()
        .context("could not start the Swift helper")?;
    std::fs::write(dir.join("pid"), child.id().to_string())?;
    Ok(())
}

fn wait_ready() -> Result<String> {
    let mac = std::fs::read_to_string(dir()?.join("mac"))?;
    let started = Instant::now();
    while started.elapsed() < BOOT_TIMEOUT {
        if !is_running() {
            let log = std::fs::read_to_string(dir()?.join("helper.log")).unwrap_or_default();
            bail!("The VM exited:\n{}", log.trim());
        }
        let leases = std::fs::read_to_string(LEASES).unwrap_or_default();
        if let Some(ip) = lease_ip(&leases, mac.trim()) {
            // Only ready once cloud-init is done (apt is still locked before that).
            let ready = ssh_command(&ip, "cloud-init status --wait >/dev/null 2>&1; true", false)?
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if ready {
                // For the helper: clean shutdown over ssh (see mpkg-vm).
                std::fs::write(dir()?.join("ip"), &ip)?;
                return Ok(ip);
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    bail!(
        "The VM is not reachable after {} s – see {}",
        BOOT_TIMEOUT.as_secs(),
        dir()?.join("console.log").display()
    )
}

/// IP address for the MAC from /var/db/dhcpd_leases. macOS writes the MAC without leading
/// zeros ("hw_address=1,2:3:a4:…"), so compare byte by byte.
fn lease_ip(leases: &str, mac: &str) -> Option<String> {
    let parse = |s: &str| -> Option<Vec<u8>> {
        s.split(':')
            .map(|b| u8::from_str_radix(b, 16).ok())
            .collect()
    };
    let wanted = parse(mac)?;
    leases.split('}').find_map(|block| {
        let field = |key: &str| {
            block
                .lines()
                .find_map(|l| l.trim().strip_prefix(key).map(str::to_owned))
        };
        let hw = field("hw_address=")?;
        let hw = hw.split_once(',').map_or(hw.as_str(), |(_, m)| m);
        (parse(hw)? == wanted)
            .then(|| field("ip_address="))
            .flatten()
    })
}

fn ssh_command(ip: &str, remote: &str, tty: bool) -> Result<Command> {
    let dir = dir()?;
    let mut cmd = Command::new("ssh");
    cmd.arg("-i")
        .arg(dir.join("id_ed25519"))
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            path_str(&dir.join("known_hosts"))?
        ))
        .args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "LogLevel=ERROR",
        ]);
    cmd.arg(if tty { "-t" } else { "-T" });
    cmd.arg(format!("{GUEST_USER}@{ip}")).arg(remote);
    Ok(cmd)
}

/// Quote safely for the remote shell.
pub fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Run an (already quoted) command in the guest, in the current directory if it lies
/// inside the shared home.
pub fn exec_raw(remote: &str) -> Result<ExitStatus> {
    let ip = ensure_running()?;
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|d| d.to_str().map(quote))
        .unwrap_or_default();
    let line = format!("cd {cwd} 2>/dev/null; {remote}");
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    ssh_command(&ip, &line, tty)?.status().context("ssh failed")
}

/// Run a command in the guest and return its output.
pub fn exec_output(remote: &str) -> Result<(bool, String)> {
    let ip = ensure_running()?;
    let out = ssh_command(&ip, remote, false)?
        .output()
        .context("ssh failed")?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

pub fn exec(args: &[String]) -> Result<i32> {
    let line = args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
    Ok(exec_raw(&format!("exec {line}"))?.code().unwrap_or(1))
}

pub fn stop() -> Result<()> {
    match pid().filter(|p| alive(*p)) {
        Some(p) => {
            run("kill", &["-TERM", &p.to_string()], false)?;
            println!("✓ VM is shutting down");
        }
        None => println!("VM is not running."),
    }
    Ok(())
}

pub fn status() -> Result<()> {
    let state = match (is_created(), is_running()) {
        (false, _) => "not created (`mpkg vm create`)",
        (true, false) => "stopped",
        (true, true) => "running",
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
        assert_eq!(
            lease_ip(leases, "02:0a:3c:04:05:06").as_deref(),
            Some("192.168.64.7")
        );
        assert_eq!(lease_ip(leases, "02:00:00:00:00:01"), None);
    }

    #[test]
    fn quotes_for_remote_shell() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's; rm -rf /"), r"'it'\''s; rm -rf /'");
    }
}
