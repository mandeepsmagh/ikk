use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
use commands::{add, check, info, init, remove, sync, uninstall, upgrade};

#[derive(Parser)]
#[command(
    name    = "ikk",
    about   = "ਇੱਕ — one version, one truth, one command",
    version = env!("CARGO_PKG_VERSION"),
    after_help = "EXAMPLES:\n  ikk init --remote github.com\n  ikk add BurntSushi/ripgrep\n  ikk add sharkdp/fd\n  ikk sync\n  ikk check\n  ikk upgrade\n  ikk info ripgrep\n  ikk remove fd\n  ikk uninstall --yes",
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
    /// Initialise ikk on this machine — creates ~/.ikk and adds to PATH
    #[command(visible_alias = "setup")]
    Init(init::InitArgs),

    /// Sync installed packages to match ikk.toml — install, upgrade, and remove as needed
    Sync,

    /// Add a package from a forge (owner/repo) or local path
    #[command(visible_alias = "install")]
    Add(add::AddArgs),

    /// Remove a package and its symlink
    Remove(remove::RemoveArgs),

    /// Upgrade packages to latest versions (skips pinned by default)
    Upgrade(upgrade::UpgradeArgs),

    /// Verify integrity — checks lock file merkle root and all binary hashes
    Check,

    /// Show details about an installed package
    Info(info::InfoArgs),

    /// Completely remove ikk from this machine (all packages, config, PATH entry)
    #[command(visible_alias = "rmrf")]
    Uninstall(uninstall::UninstallArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("ikk=info".parse()?),
        )
        .without_time()
        .with_target(false)
        .init();

    let cli = Cli::parse();

    let home = match cli.home {
        Some(p) => ikk_core::IkkHome::new(p),
        None => ikk_core::IkkHome::default(),
    };

    match cli.command {
        Command::Init(args) => init::run(args, &home).await,
        Command::Sync => sync::run(&home).await,
        Command::Add(args) => add::run(args, &home).await,
        Command::Remove(args) => remove::run(args, &home),
        Command::Upgrade(args) => upgrade::run(args, &home).await,
        Command::Check => check::run(&home),
        Command::Info(args) => info::run(args, &home),
        Command::Uninstall(args) => uninstall::run(args, &home),
    }
}
