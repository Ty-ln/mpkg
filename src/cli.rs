use clap::{Parser, Subcommand};

/// apt-artiger Paketmanager für macOS.
#[derive(Parser, Debug)]
#[command(name = "mpkg", version, about)]
pub struct Cli {
    /// Nur anzeigen, was passieren würde – nichts ausführen.
    #[arg(long, global = true)]
    pub dry_run: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Pakete installieren
    Install {
        #[arg(required = true)]
        packages: Vec<String>,
        /// Backend erzwingen (z. B. `macports`, `linuxvm`)
        #[arg(long)]
        backend: Option<String>,
    },
    /// Pakete entfernen
    Remove {
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Pakete suchen
    Search { query: String },
    /// Details zu einem Paket anzeigen
    Show { package: String },
    /// Über mpkg installierte Pakete auflisten
    List {
        /// Zusätzlich alle von den Backends angeforderten Pakete zeigen
        #[arg(long)]
        all: bool,
    },
    /// Paketlisten aller Backends aktualisieren
    Update,
    /// Installierte Pakete aktualisieren
    Upgrade,
    /// MacPorts automatisch einrichten (CLT, Installer, PATH)
    Setup,
    /// System prüfen (CLT, MacPorts, PATH, Konflikte)
    Doctor,
    /// Explizit installierte Homebrew-Pakete übernehmen
    MigrateFromBrew,
    /// Verfügbare Backends anzeigen
    Backends,
}
