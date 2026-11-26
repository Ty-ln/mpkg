mod backend;
mod cli;
mod doctor;
mod mapping;
mod migrate;
mod resolver;
mod setup;
mod state;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};

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
        Command::Backends => {
            resolver.print_backends();
            Ok(())
        }
    }
}
