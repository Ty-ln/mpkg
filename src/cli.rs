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
    /// MacPorts und alle Pakete nach einem macOS-Upgrade (oder Umzug auf einen anderen Mac) neu installieren
    MigrateMacports,
    /// Verfügbare Backends anzeigen
    Backends,
    /// Debian-VM für Pakete, die es in MacPorts nicht gibt (Backend `linuxvm`)
    Vm {
        #[command(subcommand)]
        action: VmAction,
    },
    /// MacPorts-Abdeckung der meistgenutzten Homebrew-Formeln messen
    Coverage {
        /// Anzahl der Formeln (nach Beliebtheit)
        #[arg(long, default_value_t = 300)]
        top: usize,
        /// Ziel-Plattform, z. B. darwin_20.x86_64 (Big Sur Intel) … darwin_25.x86_64 (Tahoe Intel)
        #[arg(long, conflicts_with = "intel")]
        platform: Option<String>,
        /// Kurzform für --platform darwin_25.x86_64 (macOS 26 Tahoe auf Intel)
        #[arg(long)]
        intel: bool,
        /// Vollständigen Bericht als Markdown-Datei speichern
        #[arg(long)]
        report: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum VmAction {
    /// Debian-Image laden und die VM einrichten (einmalig, ~400 MB)
    Create,
    /// VM starten (passiert sonst automatisch bei Bedarf)
    Start,
    /// VM herunterfahren
    Stop,
    /// Zustand der VM anzeigen
    Status,
    /// Befehl in der VM ausführen (nutzen die Shims in ~/.mpkg/bin)
    Exec {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Shell in der VM öffnen
    Shell,
}
