use clap::{Parser, Subcommand};

/// Package manager for macOS.
#[derive(Parser, Debug)]
#[command(name = "mpkg", version, about)]
pub struct Cli {
    /// Only show what would happen – run nothing.
    #[arg(long, global = true)]
    pub dry_run: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Install packages
    Install {
        #[arg(required = true)]
        packages: Vec<String>,
        /// Force a backend (e.g. `homebrew`, `macports`, `linuxvm`)
        #[arg(long)]
        backend: Option<String>,
        /// Install a Mac app (Homebrew cask), e.g. `--cask firefox` – Apple Silicon only
        #[arg(long, conflicts_with = "backend")]
        cask: bool,
    },
    /// Remove packages
    Remove {
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Search for packages
    Search { query: String },
    /// Show details about a package
    Show { package: String },
    /// List packages installed via mpkg
    List {
        /// Also show all packages requested in the backends
        #[arg(long)]
        all: bool,
    },
    /// Refresh the package lists of all backends
    Update,
    /// Upgrade installed packages
    Upgrade,
    /// Set up the package source automatically (Homebrew on Apple Silicon, MacPorts on Intel)
    Setup,
    /// Check the system (CLT, Homebrew/MacPorts, PATH, conflicts)
    Doctor,
    /// Take over packages and apps installed with Homebrew
    MigrateFromBrew,
    /// Reinstall MacPorts and all packages after a macOS upgrade (or a move to another Mac)
    MigrateMacports,
    /// Show the available backends
    Backends,
    /// Debian VM for packages not available natively (backend `linuxvm`)
    Vm {
        #[command(subcommand)]
        action: VmAction,
    },
    /// Measure MacPorts coverage of the most-used Homebrew formulae
    Coverage {
        /// Number of formulae (by popularity)
        #[arg(long, default_value_t = 300)]
        top: usize,
        /// Target platform, e.g. darwin_20.x86_64 (Big Sur Intel) … darwin_25.x86_64 (Tahoe Intel)
        #[arg(long, conflicts_with = "intel")]
        platform: Option<String>,
        /// Shorthand for --platform darwin_25.x86_64 (macOS 26 Tahoe on Intel)
        #[arg(long)]
        intel: bool,
        /// Save the full report as a Markdown file
        #[arg(long)]
        report: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum VmAction {
    /// Download the Debian image and set up the VM (once, ~400 MB)
    Create,
    /// Start the VM (otherwise happens automatically when needed)
    Start,
    /// Shut down the VM
    Stop,
    /// Show the VM's state
    Status,
    /// Run a command in the VM (used by the shims in ~/.mpkg/bin)
    Exec {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Open a shell in the VM
    Shell,
}
