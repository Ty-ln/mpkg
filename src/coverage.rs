//! `mpkg coverage`: measures how many of the most-used Homebrew formulae MacPorts
//! covers – on this system or for another platform (e.g. Intel).
//!
//! Source: Homebrew's public "install-on-request" analytics for the last 30 days.

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
    /// String with thousands separators, e.g. "315,023"
    count: String,
}

enum Outcome {
    Checked(Availability),
    /// Formula from a third-party tap ("user/tap/name")
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
    "Binary package",
    "Binary package only for previous version",
    "Source only",
    "Not checkable",
    "Not in MacPorts",
    "Third-party tap",
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
        .context("could not start `curl`")?;
    if !out.status.success() {
        bail!(
            "could not fetch Homebrew analytics: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).context("could not read Homebrew analytics")
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
    // ".darwin_20.x86_64" and "darwin_20.x86_64" are both accepted.
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
        bail!("MacPorts is not installed – run `mpkg setup` first.");
    }

    eprintln!("Fetching Homebrew analytics …");
    let analytics = fetch_analytics()?;
    let items: Vec<Item> = analytics.items.into_iter().take(top).collect();
    let target = platform.as_deref().unwrap_or("this system");
    eprintln!(
        "Checking {} formulae for {target} ({WORKERS} in parallel) …",
        items.len()
    );

    // Simple worker pool: each thread takes the next index.
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
            std::fs::write(path, &text).with_context(|| format!("cannot write {path}"))?;
            // Only print the summary (up to the first detail heading).
            let summary = text.split("\n## ").next().unwrap_or(&text);
            println!("{summary}\nFull report: {path}");
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
    out += &format!("# MacPorts coverage of the top {total} Homebrew formulae\n\n");
    out += &format!("- Platform: {target}\n");
    out += &format!("- Homebrew analytics (install-on-request): {start} to {end}\n\n");
    out += "| Category | Formulae | Share | weighted by installs |\n";
    out += "|---|---:|---:|---:|\n";
    for cat in CATEGORIES {
        let (n, inst) = stats(cat);
        out += &format!(
            "| {cat} | {n} | {:.1} % | {:.1} % |\n",
            pct(n, total),
            pct(inst, total_installs)
        );
    }

    // "Not checkable" means: the port exists, only the binary check failed → installable.
    let (bin_n, bin_i) = stats(CATEGORIES[0]);
    let (old_n, old_i) = stats(CATEGORIES[1]);
    let (src_n, src_i) = stats(CATEGORIES[2]);
    let (unk_n, unk_i) = stats(CATEGORIES[3]);
    out += &format!(
        "\n**Available in MacPorts (installable):** {:.1} % of formulae ({:.1} % weighted)  \n\
         **Certainly as a binary package:** {:.1} % ({:.1} % weighted)  \n\
         **Binary package incl. previous version (build backlog):** {:.1} % ({:.1} % weighted)\n",
        pct(bin_n + old_n + src_n + unk_n, total),
        pct(bin_i + old_i + src_i + unk_i, total_installs),
        pct(bin_n, total),
        pct(bin_i, total_installs),
        pct(bin_n + old_n, total),
        pct(bin_i + old_i, total_installs),
    );

    // Detail lists for everything except binary packages – those are the work items.
    for cat in &CATEGORIES[1..] {
        let list: Vec<&Row> = rows
            .iter()
            .filter(|r| category(&r.outcome) == *cat)
            .collect();
        if list.is_empty() {
            continue;
        }
        out += &format!(
            "\n## {cat} ({})\n\n| Rank | Homebrew | MacPorts name | Installs |\n|---:|---|---|---:|\n",
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
