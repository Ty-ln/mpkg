mod backend;
mod cli;
mod coverage;
mod doctor;
mod mapping;
mod migrate;
mod resolver;
mod setup;
mod state;
mod vm;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, VmAction};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mapping = mapping::MappingDb::load_default()?;
    let state = state::State::load()?;
    let resolver = resolver::Resolver::new(mapping, backend::available_backends(), state);

    match cli.command {
        Command::Install { packages, backend } => {
            resolver.install(&packages, backend.as_deref(), cli.dry_run)
        }
        Command::Remove { packages } => resolver.remove(&packages, cli.dry_run),
        Command::Search { query } => resolver.search(&query),
        Command::Show { package } => resolver.show(&package),
        Command::List { all } => resolver.list(all),
        Command::Update => resolver.update(cli.dry_run),
        Command::Upgrade => resolver.upgrade(cli.dry_run),
        Command::Setup => setup::run_setup(cli.dry_run),
        Command::Doctor => doctor::run(),
        Command::MigrateFromBrew => migrate::from_brew(&resolver, cli.dry_run),
        Command::MigrateMacports => migrate::macports(cli.dry_run),
        Command::Coverage { top, platform, intel, report } => {
            let platform = if intel { Some("darwin_25.x86_64".to_owned()) } else { platform };
            coverage::run(resolver.mapping(), top, platform.as_deref(), report.as_deref())
        }
        Command::Vm { action } => match action {
            VmAction::Create => vm::create(cli.dry_run),
            VmAction::Start => vm::ensure_running().map(|ip| println!("✓ VM läuft ({ip})")),
            VmAction::Stop => vm::stop(),
            VmAction::Status => vm::status(),
            VmAction::Exec { command } => std::process::exit(vm::exec(&command)?),
            VmAction::Shell => std::process::exit(vm::exec(&["bash".to_owned(), "-l".to_owned()])?),
        },
        Command::Backends => {
            resolver.print_backends();
            Ok(())
        }
    }
}
