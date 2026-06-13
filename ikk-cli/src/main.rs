use clap::{Parser, Subcommand};
use anyhow::Result;

mod commands;
use commands::{init, add, remove, sync, upgrade, check, info, uninstall};

#[derive(Parser)]
#[command(
    name    = "ikk",
    about   = "ਇੱਕ — one version, one truth, one command",
    version = env!("CARGO_PKG_VERSION"),
)]
struct Cli {
    /// Override ikk home directory (default: ~/.ikk)
    #[arg(long, global = true, env = "IKK_HOME")]
    home: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialise ikk on this machine
    Init(init::InitArgs),

    /// Sync installed packages to match ikk.lock
    Sync,

    /// Add a package
    Add(add::AddArgs),

    /// Remove a package
    Remove(remove::RemoveArgs),

    /// Upgrade packages to latest versions
    Upgrade(upgrade::UpgradeArgs),

    /// Verify all installed package hashes
    Check,

    /// Show information about an installed package
    Info(info::InfoArgs),

    /// Completely remove ikk from this machine
    Uninstall(uninstall::UninstallArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ikk=info".parse()?)
        )
        .without_time()
        .with_target(false)
        .init();

    let cli = Cli::parse();

    let home = match cli.home {
        Some(p) => ikk_core::IkkHome::new(p),
        None    => ikk_core::IkkHome::default(),
    };

    match cli.command {
        Command::Init(args)      => init::run(args, &home).await,
        Command::Sync            => sync::run(&home).await,
        Command::Add(args)       => add::run(args, &home).await,
        Command::Remove(args)    => remove::run(args, &home),
        Command::Upgrade(args)   => upgrade::run(args, &home).await,
        Command::Check           => check::run(&home),
        Command::Info(args)      => info::run(args, &home),
        Command::Uninstall(args) => uninstall::run(args, &home),
    }
}
