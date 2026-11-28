use super::{Availability, Backend, debug, run};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Default prefix – the official binary archives only work there.
const PORT: &str = "/opt/local/bin/port";
/// Server with the prebuilt archives (one directory per port).
const ARCHIVE_BASE: &str = "https://packages.macports.org";

pub struct MacPorts {
    /// For `mpkg coverage`: check another platform, e.g. ".darwin_20.x86_64".
    platform_override: Option<String>,
}

impl MacPorts {
    pub fn new() -> Self {
        Self {
            platform_override: None,
        }
    }

    /// Checks binary packages for another platform instead of this system.
    pub fn with_platform(platform: &str) -> Self {
        Self {
            platform_override: Some(platform.to_owned()),
        }
    }
}

/// Platform tag in the archive name, e.g. ".darwin_25.arm64" or ".darwin_20.x86_64".
fn platform_tag() -> Option<String> {
    let uname = |flag: &str| {
        Command::new("uname")
            .arg(flag)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let darwin_major = uname("-r")?.split('.').next()?.to_owned();
    let arch = uname("-m")?;
    Some(format!(".darwin_{darwin_major}.{arch}"))
}

/// Architecture-independent archives, e.g. "….any_any.noarch" (scripts),
/// "….darwin_any.noarch" or "….darwin_25.noarch".
fn is_noarch(file: &str, platform: &str) -> bool {
    // platform = ".darwin_25.arm64" → os = ".darwin_25"
    let os = platform.rsplit_once('.').map_or(platform, |(os, _)| os);
    file.contains(".any_any.noarch")
        || file.contains(".darwin_any.noarch")
        || file.contains(&format!("{os}.noarch"))
}

/// Default variants from the `port variants` output, in archive-name format.
/// `[+]` marks default variants; `(+)` comes from the local variants.conf and does not
/// count, because the official archives are only built with default variants.
///   "[+]huge: Build huge vim\n   x11: …" → "+huge"
fn default_variants(output: &str) -> String {
    let mut variants: Vec<&str> = output
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("[+]"))
        .filter_map(|rest| rest.split(':').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect();
    variants.sort_unstable();
    variants.dedup();
    variants.iter().map(|v| format!("+{v}")).collect()
}

/// HTTP request via curl; returns (status code, body) or None on network problems.
///
/// Deliberately without `curl -f`: its exit code on HTTP errors is not consistent
/// (22, sometimes 56 with HTTP/2). The status code via `-w` is unambiguous.
fn http_request(url: &str, head: bool, timeout_secs: u32) -> Option<(u16, String)> {
    // Up to 3 attempts on network errors, 429 (rate limit) and 5xx – with a short pause.
    for attempt in 1..=3u64 {
        match http_request_once(url, head, timeout_secs) {
            Some((code, body)) if code != 429 && code < 500 => return Some((code, body)),
            result if attempt == 3 => return result,
            _ => {
                debug(format!("{url} → attempt {attempt} failed, retrying …"));
                std::thread::sleep(std::time::Duration::from_secs(attempt));
            }
        }
    }
    None
}

fn http_request_once(url: &str, head: bool, timeout_secs: u32) -> Option<(u16, String)> {
    let timeout = timeout_secs.to_string();
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sSL",
        "--max-time",
        timeout.as_str(),
        "-w",
        "\n%{http_code}",
    ]);
    if head {
        cmd.args(["-I", "-o", "/dev/null"]);
    }
    let out = cmd.arg(url).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", text.as_ref()));
    let code: u16 = code.trim().parse().unwrap_or(0);
    if code == 0 {
        // 000 = no HTTP response (timeout, DNS, TLS …)
        debug(format!(
            "{url} → no response: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
        return None;
    }
    Some((code, body.to_owned()))
}

/// HEAD request: Some(true) = exists, Some(false) = 4xx, None = network/server problem.
fn url_exists(url: &str) -> Option<bool> {
    match http_request(url, true, 10)? {
        (200..=299, _) => Some(true),
        (400..=499, _) => Some(false),
        (code, _) => {
            debug(format!("HEAD {url} → HTTP {code}"));
            None
        }
    }
}

/// Fast path: build the exact archive name and query it with HEAD.
/// Only returns a result on a hit – otherwise the directory listing decides.
fn exact_archive_status(
    name: &str,
    version: &str,
    revision: &str,
    variants: &str,
    platform: &str,
) -> Option<Availability> {
    let os = platform.rsplit_once('.').map_or(platform, |(os, _)| os);
    let base = format!("{ARCHIVE_BASE}/{name}/{name}-{version}_{revision}{variants}");
    let suffixes = [
        platform.to_owned(),
        ".any_any.noarch".to_owned(),
        ".darwin_any.noarch".to_owned(),
        format!("{os}.noarch"),
    ];
    for suffix in &suffixes {
        let url = format!("{base}{suffix}.tbz2");
        if url_exists(&url) == Some(true) {
            debug(format!("{name}: HEAD hit: {url}"));
            return Some(Availability::Binary);
        }
    }
    debug(format!(
        "{name}: no HEAD hit for {base}… – falling back to the directory listing"
    ));
    None
}

/// Checks packages.macports.org for an archive of the current version for this
/// platform (or an architecture-independent one).
///
/// Archive names: <name>-<version>_<revision>[+variants].darwin_<N>.<arch>.tbz2
fn binary_archive_status(
    name: &str,
    version: &str,
    revision: &str,
    variants: &str,
    platform_override: Option<&str>,
) -> Availability {
    let Some(platform) = platform_override.map(str::to_owned).or_else(platform_tag) else {
        debug(format!(
            "{name}: could not determine the platform tag (uname)"
        ));
        return Availability::Unknown;
    };

    if let Some(status) = exact_archive_status(name, version, revision, variants, &platform) {
        return status;
    }

    let url = format!("{ARCHIVE_BASE}/{name}/");
    let listing = match http_request(&url, false, 30) {
        Some((200..=299, body)) => body,
        // No directory = MacPorts never published an archive for this port
        // (e.g. because its license does not allow distributing binaries).
        Some((404, _)) => {
            debug(format!("{name}: {url} → HTTP 404 (no archive directory)"));
            return Availability::SourceOnly;
        }
        Some((code, _)) => {
            debug(format!("{name}: {url} → HTTP {code}"));
            return Availability::Unknown;
        }
        None => return Availability::Unknown,
    };

    let prefix = format!("{name}-{version}_{revision}");
    let found = listing.match_indices(&prefix).any(|(i, _)| {
        let rest = &listing[i + prefix.len()..];
        // Must be followed directly by a variant (+…) or the platform (.darwin…).
        if !(rest.starts_with('+') || rest.starts_with('.')) {
            return false;
        }
        let file = &rest[..rest.find(".tbz2").unwrap_or(0)];
        file.contains(&platform) || is_noarch(file, &platform)
    });

    debug(format!(
        "{name}: looking for `{prefix}` with `{platform}` in {} bytes → {}",
        listing.len(),
        if found { "found" } else { "not found" }
    ));
    if found {
        return Availability::Binary;
    }

    // Is there at least an older version for this platform? Then the build is lagging behind.
    let any_version = format!("{name}-");
    let older = listing.match_indices(&any_version).any(|(i, _)| {
        let rest = &listing[i..];
        let file = &rest[..rest.find(".tbz2").unwrap_or(0)];
        file.contains(&platform) || is_noarch(file, &platform)
    });
    debug(format!(
        "{name}: older version for `{platform}` available: {older}"
    ));
    if older {
        Availability::OlderBinary
    } else {
        Availability::SourceOnly
    }
}

impl Backend for MacPorts {
    fn id(&self) -> &'static str {
        "macports"
    }

    fn is_ready(&self) -> bool {
        Path::new(PORT).exists()
    }

    fn availability(&self, name: &str) -> Result<Availability> {
        // Returns "version<TAB>revision"; fails if the port does not exist.
        let out = Command::new(PORT)
            .args(["-q", "info", "--line", "--version", "--revision", name])
            .output()
            .context("`port info` failed")?;
        let info = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || info.trim().is_empty() {
            return Ok(Availability::NotFound);
        }
        // Version and revision contain no spaces; the rest are the variants.
        let mut fields = info.trim().splitn(3, char::is_whitespace);
        let (Some(version), Some(revision)) = (fields.next(), fields.next()) else {
            debug(format!(
                "{name}: unexpected `port info` output: {:?}",
                info.trim()
            ));
            return Ok(Availability::Unknown);
        };
        let variants_out = Command::new(PORT)
            .args(["-q", "variants", name])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let variants = default_variants(&variants_out);
        debug(format!(
            "{name}: version={version} revision={revision} default_variants={variants:?}"
        ));
        Ok(binary_archive_status(
            name,
            version,
            revision,
            &variants,
            self.platform_override.as_deref(),
        ))
    }

    fn install(&self, name: &str, dry_run: bool) -> Result<()> {
        // -N: non-interactive. sudo, because /opt/local belongs to root.
        run("sudo", &[PORT, "-N", "install", name], dry_run)
    }

    fn remove(&self, name: &str, dry_run: bool) -> Result<()> {
        run("sudo", &[PORT, "-N", "uninstall", name], dry_run)
    }

    fn search(&self, query: &str) -> Result<Vec<String>> {
        let out = Command::new(PORT)
            .args(["-q", "search", "--name", "--line", query])
            .output()
            .context("`port search` failed")?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_owned)
            .collect())
    }

    fn list(&self) -> Result<()> {
        run(PORT, &["-q", "installed", "requested"], false)
    }

    fn update(&self, dry_run: bool) -> Result<()> {
        run("sudo", &[PORT, "selfupdate"], dry_run)
    }

    fn upgrade(&self, dry_run: bool) -> Result<()> {
        // Check first whether anything is outdated: depending on the version,
        // `port upgrade outdated` treats "nothing to do" as an error.
        // Output of `port -q outdated`: "name   1.0_0 < 1.1_0" per line.
        let out = Command::new(PORT)
            .args(["-q", "outdated"])
            .output()
            .context("`port outdated` failed")?;
        let outdated: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_owned)
            .collect();

        if outdated.is_empty() {
            println!("✓ All MacPorts packages are up to date.");
            return Ok(());
        }
        println!(
            "{} outdated packages: {}",
            outdated.len(),
            outdated.join(", ")
        );
        run("sudo", &[PORT, "-N", "upgrade", "outdated"], dry_run)
    }
}

#[cfg(test)]
mod tests {
    use super::{default_variants, is_noarch};

    #[test]
    fn variants_to_archive_suffix() {
        let out = "   x11: Build GUI version\n[+]huge: Build huge vim\n     * conflicts with big small\n(+)python313: Python 3.13\n[+]credential_osxkeychain: Keychain";
        assert_eq!(default_variants(out), "+credential_osxkeychain+huge");
        assert_eq!(default_variants("   universal: Build universal"), "");
        assert_eq!(default_variants(""), "");
    }

    #[test]
    fn noarch_patterns() {
        let p = ".darwin_25.arm64";
        assert!(is_noarch(".any_any.noarch", p));
        assert!(is_noarch(".darwin_any.noarch", p));
        assert!(is_noarch(".darwin_25.noarch", p));
        assert!(!is_noarch(".darwin_24.x86_64", p));
    }
}
