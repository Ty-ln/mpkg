//! `mpkg setup`: sets up the main backend automatically.
//!
//! Apple Silicon: Homebrew via the official, signed `Homebrew.pkg`
//! (Gatekeeper-checked, no `curl | bash`) and `brew shellenv` in the shell profile.
//!
//! Intel – steps for MacPorts:
//!   1. Check the Xcode Command Line Tools (otherwise open Apple's dialog)
//!   2. Find the latest MacPorts release via the GitHub API
//!   3. Pick the installer for the macOS major version
//!      (scheme: MacPorts-<version>-<major>-<Name>.pkg)
//!   4. Download, check Gatekeeper/notarisation, install with `installer`
//!   5. Add PATH to the shell profile (after asking)
//!   6. `port selfupdate`

use crate::backend::run;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const PORT: &str = "/opt/local/bin/port";
const RELEASES_API: &str = "https://api.github.com/repos/macports/macports-base/releases/latest";
const PATH_LINE: &str = r#"export PATH="/opt/local/bin:/opt/local/sbin:$PATH""#;
const BREW_RELEASES_API: &str = "https://api.github.com/repos/Homebrew/brew/releases/latest";
const BREW_SHELLENV_LINE: &str = r#"eval "$(/opt/homebrew/bin/brew shellenv)""#;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub fn run_setup(dry_run: bool) -> Result<()> {
    if std::env::consts::ARCH == "aarch64" {
        return setup_homebrew(dry_run);
    }
    // After a macOS upgrade the existing MacPorts install no longer fits:
    // then install the installer for the new version over it.
    let mut os_upgrade = false;
    if Path::new(PORT).exists() {
        match crate::doctor::macports_platform_mismatch() {
            Some(Some(msg)) => {
                println!("! MacPorts no longer matches the macOS version ({msg}).");
                println!("  Installing the matching MacPorts version …");
                os_upgrade = true;
            }
            _ => {
                println!("✓ MacPorts is already installed.");
                return ensure_path(dry_run);
            }
        }
    }

    ensure_clt(dry_run)?;

    let major = macos_major()?;
    if major < 11 {
        bail!("macOS {major} is not supported (macOS 11 Big Sur or later required).");
    }

    let (version, asset) = find_installer(major)?;
    println!("MacPorts {version} for macOS {major}: {}", asset.name);

    if dry_run {
        if os_upgrade {
            println!(
                "→ afterwards the installed packages would need a rebuild: mpkg migrate-macports"
            );
        }
        println!("→ would download: {}", asset.browser_download_url);
        println!(
            "→ would install: sudo installer -pkg {} -target /",
            asset.name
        );
        return ensure_path(true);
    }

    install_pkg(&asset)?;

    ensure_path(false)?;
    run("sudo", &[PORT, "selfupdate"], false)?;

    println!("\n✓ MacPorts is set up. Open a new terminal or reload your shell profile.");
    if os_upgrade {
        println!("\n! The packages installed so far were built for the old macOS version");
        println!("  and need to be reinstalled: mpkg migrate-macports");
    }
    Ok(())
}

/// Command Line Tools present? Otherwise open Apple's install dialog.
pub(crate) fn ensure_clt(dry_run: bool) -> Result<()> {
    let ok = Command::new("xcode-select")
        .arg("-p")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        println!("✓ Xcode Command Line Tools present");
        return Ok(());
    }
    if dry_run {
        println!("→ would start `xcode-select --install`");
        return Ok(());
    }
    // Opens a graphical dialog; the installation runs asynchronously.
    run("xcode-select", &["--install"], false)?;
    bail!(
        "The Command Line Tools are being installed. Please finish the dialog \
         and then run `mpkg setup` again."
    )
}

pub(crate) fn macos_major() -> Result<u32> {
    let out = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .context("`sw_vers` failed")?;
    let version = String::from_utf8_lossy(&out.stdout);
    version
        .trim()
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .with_context(|| format!("unknown macOS version `{}`", version.trim()))
}

/// Fetch the latest release of a GitHub repo via the API.
fn latest_release(api: &str) -> Result<Release> {
    let out = Command::new("curl")
        .args(["-fsSL", "-H", "Accept: application/vnd.github+json", api])
        .output()
        .context("`curl` failed")?;
    if !out.status.success() {
        bail!("Could not fetch releases (GitHub API: {api}).");
    }
    serde_json::from_slice(&out.stdout).context("could not read the GitHub API response")
}

/// Download a signed .pkg, check it with Gatekeeper and install it.
fn install_pkg(asset: &Asset) -> Result<()> {
    let pkg = std::env::temp_dir().join(&asset.name);
    let pkg_str = pkg.to_str().context("invalid temp path")?;
    // --retry: installers are 30+ MB, drops on weak Wi-Fi are common.
    run(
        "curl",
        &[
            "-fL",
            "--retry",
            "3",
            "--progress-bar",
            "-o",
            pkg_str,
            asset.browser_download_url.as_str(),
        ],
        false,
    )?;
    verify_package(&pkg)?;
    run(
        "sudo",
        &["installer", "-pkg", pkg_str, "-target", "/"],
        false,
    )?;
    let _ = std::fs::remove_file(&pkg);
    Ok(())
}

/// Apple Silicon: set up Homebrew (or use the existing one).
fn setup_homebrew(dry_run: bool) -> Result<()> {
    let brew = crate::backend::homebrew::BREW;
    if Path::new(brew).exists() {
        println!("✓ Homebrew is already installed.");
        // Already in PATH (e.g. via ~/.zshrc)? Then do not add a second line.
        if std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .any(|p| p == "/opt/homebrew/bin")
        {
            println!("✓ /opt/homebrew/bin is in PATH");
            return Ok(());
        }
        return ensure_profile_line(
            "/opt/homebrew/bin/brew shellenv",
            BREW_SHELLENV_LINE,
            "Homebrew",
            dry_run,
        );
    }
    ensure_clt(dry_run)?;
    let release = latest_release(BREW_RELEASES_API)?;
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == "Homebrew.pkg")
        .context("no Homebrew.pkg found in the latest Homebrew release")?;
    println!("Homebrew {}: {}", release.tag_name, asset.name);
    if dry_run {
        println!("→ would download: {}", asset.browser_download_url);
        println!(
            "→ would install: sudo installer -pkg {} -target /",
            asset.name
        );
        return ensure_profile_line(
            "/opt/homebrew/bin/brew shellenv",
            BREW_SHELLENV_LINE,
            "Homebrew",
            true,
        );
    }
    install_pkg(&asset)?;
    ensure_profile_line(
        "/opt/homebrew/bin/brew shellenv",
        BREW_SHELLENV_LINE,
        "Homebrew",
        false,
    )?;
    println!("\n✓ Homebrew is set up. Open a new terminal or reload your shell profile.");
    Ok(())
}

/// Fetch the latest release and pick the installer for `major`.
fn find_installer(major: u32) -> Result<(String, Asset)> {
    let release = latest_release(RELEASES_API)?;

    // "-26-" matches "MacPorts-2.12.7-26-Tahoe.pkg" but not "-10.11-".
    let marker = format!("-{major}-");
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name.starts_with("MacPorts-") && a.name.ends_with(".pkg") && a.name.contains(&marker))
        .with_context(|| {
            format!("no MacPorts installer for macOS {major} in the latest release – see macports.org/install.php")
        })?;

    let version = release.tag_name.trim_start_matches('v').to_owned();
    Ok((version, asset))
}

/// Gatekeeper check: the package must be signed with a Developer ID and accepted by Apple.
fn verify_package(pkg: &Path) -> Result<()> {
    let ok = Command::new("spctl")
        .args(["--assess", "--type", "install"])
        .arg(pkg)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        bail!(
            "Gatekeeper rejected {} – installation aborted.",
            pkg.display()
        );
    }
    println!("✓ Signature and notarisation verified");
    Ok(())
}

/// Shell profile file for the login shell.
pub(crate) fn shell_profile() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let shell = std::env::var("SHELL").unwrap_or_default();
    let file = if shell.ends_with("bash") {
        ".bash_profile"
    } else {
        ".zprofile"
    };
    Ok(PathBuf::from(home).join(file))
}

/// Add /opt/local/bin to the shell profile (after asking).
fn ensure_path(dry_run: bool) -> Result<()> {
    ensure_profile_line("/opt/local/bin", PATH_LINE, "MacPorts", dry_run)
}

/// Add `line` to the shell profile (after asking), unless `marker` is already in it.
fn ensure_profile_line(marker: &str, line: &str, name: &str, dry_run: bool) -> Result<()> {
    let profile = shell_profile()?;
    let content = std::fs::read_to_string(&profile).unwrap_or_default();
    if content.contains(marker) {
        println!("✓ PATH entry already in {}", profile.display());
        return Ok(());
    }
    if dry_run {
        println!("→ would add to {}: {line}", profile.display());
        return Ok(());
    }

    print!(
        "Add the PATH entry for {name} to {}? [Y/n] ",
        profile.display()
    );
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim(), "n" | "N") {
        println!("Skipped. Add it manually: {line}");
        return Ok(());
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&profile)
        .with_context(|| format!("cannot write {}", profile.display()))?;
    writeln!(file, "\n# {name} (added by mpkg)\n{line}")?;
    println!("✓ PATH entry added");
    Ok(())
}
