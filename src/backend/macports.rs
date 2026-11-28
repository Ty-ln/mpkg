use super::{Availability, Backend, debug, run};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Standard-Präfix – nur dort passen die offiziellen Binärarchive.
const PORT: &str = "/opt/local/bin/port";
/// Server mit den vorgebauten Archiven (ein Verzeichnis pro Port).
const ARCHIVE_BASE: &str = "https://packages.macports.org";

pub struct MacPorts {
    /// Für `mpkg coverage`: andere Plattform prüfen, z. B. ".darwin_20.x86_64".
    platform_override: Option<String>,
}

impl MacPorts {
    pub fn new() -> Self {
        Self {
            platform_override: None,
        }
    }

    /// Prüft Binärpakete für eine fremde Plattform statt für dieses System.
    pub fn with_platform(platform: &str) -> Self {
        Self {
            platform_override: Some(platform.to_owned()),
        }
    }
}

/// Plattform-Kennung im Archivnamen, z. B. ".darwin_25.arm64" oder ".darwin_20.x86_64".
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

/// Architekturunabhängige Archive, z. B. "….any_any.noarch" (Skripte),
/// "….darwin_any.noarch" oder "….darwin_25.noarch".
fn is_noarch(file: &str, platform: &str) -> bool {
    // platform = ".darwin_25.arm64" → os = ".darwin_25"
    let os = platform.rsplit_once('.').map_or(platform, |(os, _)| os);
    file.contains(".any_any.noarch")
        || file.contains(".darwin_any.noarch")
        || file.contains(&format!("{os}.noarch"))
}

/// Standardvarianten aus der `port variants`-Ausgabe, im Archivnamen-Format.
/// `[+]` markiert Standardvarianten; `(+)` stammt aus der lokalen variants.conf und
/// zählt nicht, weil die offiziellen Archive nur mit Standardvarianten gebaut werden.
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

/// HTTP-Anfrage über curl; liefert (Statuscode, Body) oder None bei Netzwerkproblemen.
///
/// Bewusst ohne `curl -f`: Dessen Exit-Code bei HTTP-Fehlern ist nicht einheitlich
/// (22, bei HTTP/2 teils 56). Der Statuscode über `-w` ist eindeutig.
fn http_request(url: &str, head: bool, timeout_secs: u32) -> Option<(u16, String)> {
    // Bis zu 3 Versuche bei Netzwerkfehlern, 429 (Rate-Limit) und 5xx – mit kurzer Pause.
    for attempt in 1..=3u64 {
        match http_request_once(url, head, timeout_secs) {
            Some((code, body)) if code != 429 && code < 500 => return Some((code, body)),
            result if attempt == 3 => return result,
            _ => {
                debug(format!(
                    "{url} → Versuch {attempt} fehlgeschlagen, wiederhole …"
                ));
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
        // 000 = keine HTTP-Antwort (Timeout, DNS, TLS …)
        debug(format!(
            "{url} → keine Antwort: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
        return None;
    }
    Some((code, body.to_owned()))
}

/// HEAD-Request: Some(true) = existiert, Some(false) = 4xx, None = Netzwerk-/Serverproblem.
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

/// Schneller Weg: exakten Archivnamen bauen und gezielt per HEAD abfragen.
/// Liefert nur bei einem Treffer ein Ergebnis – sonst entscheidet die Verzeichnisliste.
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
            debug(format!("{name}: Treffer per HEAD: {url}"));
            return Some(Availability::Binary);
        }
    }
    debug(format!(
        "{name}: kein HEAD-Treffer für {base}… – Fallback auf Verzeichnisliste"
    ));
    None
}

/// Prüft auf packages.macports.org, ob es für die aktuelle Version ein Archiv
/// für diese Plattform (oder ein architekturunabhängiges) gibt.
///
/// Archivnamen: <name>-<version>_<revision>[+varianten].darwin_<N>.<arch>.tbz2
fn binary_archive_status(
    name: &str,
    version: &str,
    revision: &str,
    variants: &str,
    platform_override: Option<&str>,
) -> Availability {
    let Some(platform) = platform_override.map(str::to_owned).or_else(platform_tag) else {
        debug(format!(
            "{name}: Plattform-Kennung (uname) nicht ermittelbar"
        ));
        return Availability::Unknown;
    };

    if let Some(status) = exact_archive_status(name, version, revision, variants, &platform) {
        return status;
    }

    let url = format!("{ARCHIVE_BASE}/{name}/");
    let listing = match http_request(&url, false, 30) {
        Some((200..=299, body)) => body,
        // Kein Verzeichnis = MacPorts hat für diesen Port nie ein Archiv veröffentlicht
        // (z. B. weil die Lizenz das Verteilen von Binärpaketen nicht erlaubt).
        Some((404, _)) => {
            debug(format!("{name}: {url} → HTTP 404 (kein Archivverzeichnis)"));
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
        // Direkt danach muss eine Variante (+…) oder die Plattform (.darwin…) folgen.
        if !(rest.starts_with('+') || rest.starts_with('.')) {
            return false;
        }
        let file = &rest[..rest.find(".tbz2").unwrap_or(0)];
        file.contains(&platform) || is_noarch(file, &platform)
    });

    debug(format!(
        "{name}: suche `{prefix}` mit `{platform}` in {} Bytes → {}",
        listing.len(),
        if found { "gefunden" } else { "nicht gefunden" }
    ));
    if found {
        return Availability::Binary;
    }

    // Gibt es für diese Plattform wenigstens eine ältere Version? Dann hinkt der Build hinterher.
    let any_version = format!("{name}-");
    let older = listing.match_indices(&any_version).any(|(i, _)| {
        let rest = &listing[i..];
        let file = &rest[..rest.find(".tbz2").unwrap_or(0)];
        file.contains(&platform) || is_noarch(file, &platform)
    });
    debug(format!(
        "{name}: ältere Version für `{platform}` vorhanden: {older}"
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
        // Liefert "version<TAB>revision"; schlägt fehl, wenn es den Port nicht gibt.
        let out = Command::new(PORT)
            .args(["-q", "info", "--line", "--version", "--revision", name])
            .output()
            .context("`port info` fehlgeschlagen")?;
        let info = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || info.trim().is_empty() {
            return Ok(Availability::NotFound);
        }
        // Version und Revision enthalten keine Leerzeichen; der Rest sind die Varianten.
        let mut fields = info.trim().splitn(3, char::is_whitespace);
        let (Some(version), Some(revision)) = (fields.next(), fields.next()) else {
            debug(format!("{name}: `port info` unerwartet: {:?}", info.trim()));
            return Ok(Availability::Unknown);
        };
        let variants_out = Command::new(PORT)
            .args(["-q", "variants", name])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let variants = default_variants(&variants_out);
        debug(format!(
            "{name}: version={version} revision={revision} standardvarianten={variants:?}"
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
        // -N: nicht interaktiv. sudo, weil /opt/local root gehört.
        run("sudo", &[PORT, "-N", "install", name], dry_run)
    }

    fn remove(&self, name: &str, dry_run: bool) -> Result<()> {
        run("sudo", &[PORT, "-N", "uninstall", name], dry_run)
    }

    fn search(&self, query: &str) -> Result<Vec<String>> {
        let out = Command::new(PORT)
            .args(["-q", "search", "--name", "--line", query])
            .output()
            .context("`port search` fehlgeschlagen")?;
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
        // Erst nachsehen, ob etwas veraltet ist: `port upgrade outdated` behandelt
        // „nichts zu tun“ je nach Version als Fehler.
        // Ausgabe von `port -q outdated`: "name   1.0_0 < 1.1_0" pro Zeile.
        let out = Command::new(PORT)
            .args(["-q", "outdated"])
            .output()
            .context("`port outdated` fehlgeschlagen")?;
        let outdated: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_owned)
            .collect();

        if outdated.is_empty() {
            println!("✓ Alle MacPorts-Pakete sind aktuell.");
            return Ok(());
        }
        println!(
            "{} veraltete Pakete: {}",
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
