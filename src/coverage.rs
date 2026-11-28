//! `mpkg coverage`: misst, wie viele der meistgenutzten Homebrew-Formeln
//! MacPorts abdeckt – auf diesem System oder für eine andere Plattform (z. B. Intel).
//!
//! Quelle: Homebrews öffentliche Statistik „install-on-request“ der letzten 30 Tage.

use crate::backend::macports::MacPorts;
use crate::backend::{Availability, Backend};
use crate::mapping::MappingDb;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const ANALYTICS_URL: &str = "https://formulae.brew.sh/api/analytics/install-on-request/30d.json";
const WORKERS: usize = 8;

#[derive(Deserialize)]
struct Analytics {
    start_date: String,
    end_date: String,
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    number: u32,
    formula: String,
    /// String mit Tausendertrennern, z. B. "315,023"
    count: String,
}

enum Outcome {
    Checked(Availability),
    /// Formel aus einem Drittanbieter-Tap ("user/tap/name")
    ThirdParty,
}

struct Row {
    rank: u32,
    formula: String,
    native: String,
    installs: u64,
    outcome: Outcome,
}

const CATEGORIES: [&str; 6] = [
    "Binärpaket",
    "Binärpaket nur für Vorversion",
    "Nur Quellcode",
    "Nicht prüfbar",
    "Nicht in MacPorts",
    "Drittquelle (Tap)",
];

fn category(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Checked(Availability::Binary) => CATEGORIES[0],
        Outcome::Checked(Availability::OlderBinary) => CATEGORIES[1],
        Outcome::Checked(Availability::SourceOnly) => CATEGORIES[2],
        Outcome::Checked(Availability::Unknown) => CATEGORIES[3],
        Outcome::Checked(Availability::NotFound) => CATEGORIES[4],
        Outcome::ThirdParty => CATEGORIES[5],
    }
}

fn pct(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 * 100.0 / total as f64
    }
}

fn fetch_analytics() -> Result<Analytics> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "60", ANALYTICS_URL])
        .output()
        .context("`curl` nicht startbar")?;
    if !out.status.success() {
        bail!(
            "Homebrew-Statistik nicht abrufbar: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).context("Homebrew-Statistik nicht lesbar")
}

fn check(item: &Item, mapping: &MappingDb, macports: &MacPorts) -> Row {
    let installs = item.count.replace(',', "").parse().unwrap_or(0);
    if item.formula.contains('/') {
        return Row {
            rank: item.number,
            formula: item.formula.clone(),
            native: String::new(),
            installs,
            outcome: Outcome::ThirdParty,
        };
    }
    let canonical = mapping.canonical_from_brew(&item.formula);
    let native = mapping.name_for(&canonical, "macports");
    let availability = macports
        .availability(&native)
        .unwrap_or(Availability::Unknown);
    Row {
        rank: item.number,
        formula: item.formula.clone(),
        native,
        installs,
        outcome: Outcome::Checked(availability),
    }
}

pub fn run(
    mapping: &MappingDb,
    top: usize,
    platform: Option<&str>,
    report: Option<&str>,
) -> Result<()> {
    // ".darwin_20.x86_64" und "darwin_20.x86_64" sind beide erlaubt.
    let platform = platform.map(|p| {
        if p.starts_with('.') {
            p.to_owned()
        } else {
            format!(".{p}")
        }
    });
    let macports = match &platform {
        Some(p) => MacPorts::with_platform(p),
        None => MacPorts::new(),
    };
    if !macports.is_ready() {
        bail!("MacPorts ist nicht installiert – zuerst `mpkg setup` ausführen.");
    }

    eprintln!("Lade Homebrew-Statistik …");
    let analytics = fetch_analytics()?;
    let items: Vec<Item> = analytics.items.into_iter().take(top).collect();
    let target = platform.as_deref().unwrap_or("dieses System");
    eprintln!(
        "Prüfe {} Formeln für {target} ({WORKERS} parallel) …",
        items.len()
    );

    // Einfacher Arbeits-Pool: jeder Thread holt sich den nächsten Index.
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let rows = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let row = check(item, mapping, &macports);
                    rows.lock().unwrap().push(row);
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    eprint!("\r  {n}/{}", items.len());
                }
            });
        }
    });
    eprintln!();

    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| r.rank);
    let text = render(&rows, &analytics.start_date, &analytics.end_date, target);

    match report {
        Some(path) => {
            std::fs::write(path, &text).with_context(|| format!("{path} nicht schreibbar"))?;
            // Nur die Zusammenfassung (bis zur ersten Detail-Überschrift) ausgeben.
            let summary = text.split("\n## ").next().unwrap_or(&text);
            println!("{summary}\nVollständiger Bericht: {path}");
        }
        None => print!("{text}"),
    }
    Ok(())
}

fn render(rows: &[Row], start: &str, end: &str, target: &str) -> String {
    let total = rows.len() as u64;
    let total_installs: u64 = rows.iter().map(|r| r.installs).sum();
    let stats = |cat: &str| -> (u64, u64) {
        rows.iter()
            .filter(|r| category(&r.outcome) == cat)
            .fold((0, 0), |(n, inst), r| (n + 1, inst + r.installs))
    };

    let mut out = String::new();
    out += &format!("# MacPorts-Abdeckung der Top-{total} Homebrew-Formeln\n\n");
    out += &format!("- Plattform: {target}\n");
    out += &format!("- Homebrew-Statistik (install-on-request): {start} bis {end}\n\n");
    out += "| Kategorie | Formeln | Anteil | gewichtet nach Installationen |\n";
    out += "|---|---:|---:|---:|\n";
    for cat in CATEGORIES {
        let (n, inst) = stats(cat);
        out += &format!(
            "| {cat} | {n} | {:.1} % | {:.1} % |\n",
            pct(n, total),
            pct(inst, total_installs)
        );
    }

    // „Nicht prüfbar“ heißt: Port existiert, nur der Binärpaket-Check scheiterte → installierbar.
    let (bin_n, bin_i) = stats(CATEGORIES[0]);
    let (old_n, old_i) = stats(CATEGORIES[1]);
    let (src_n, src_i) = stats(CATEGORIES[2]);
    let (unk_n, unk_i) = stats(CATEGORIES[3]);
    out += &format!(
        "\n**In MacPorts vorhanden (installierbar):** {:.1} % der Formeln ({:.1} % gewichtet)  \n\
         **Sicher als Binärpaket:** {:.1} % ({:.1} % gewichtet)  \n\
         **Binärpaket inkl. Vorversion (Build-Rückstand):** {:.1} % ({:.1} % gewichtet)\n",
        pct(bin_n + old_n + src_n + unk_n, total),
        pct(bin_i + old_i + src_i + unk_i, total_installs),
        pct(bin_n, total),
        pct(bin_i, total_installs),
        pct(bin_n + old_n, total),
        pct(bin_i + old_i, total_installs),
    );

    // Detail-Listen für alles außer Binärpaketen – das sind die Arbeitspakete.
    for cat in &CATEGORIES[1..] {
        let list: Vec<&Row> = rows
            .iter()
            .filter(|r| category(&r.outcome) == *cat)
            .collect();
        if list.is_empty() {
            continue;
        }
        out += &format!(
            "\n## {cat} ({})\n\n| Rang | Homebrew | MacPorts-Name | Installationen |\n|---:|---|---|---:|\n",
            list.len()
        );
        for r in list {
            out += &format!(
                "| {} | {} | {} | {} |\n",
                r.rank, r.formula, r.native, r.installs
            );
        }
    }
    out
}
