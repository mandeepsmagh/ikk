#![allow(clippy::needless_pass_by_value, clippy::unused_async)]
#![allow(clippy::redundant_closure_for_method_calls)]

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
use commands::{
    add, check, config, gc, info, init, list, remove, run, self_update, sync, uninstall, upgrade,
};

#[derive(Parser)]
#[command(
    name    = "ikk",
    about   = "ਇੱਕ — one version, one truth, one command",
    version = env!("CARGO_PKG_VERSION"),
    after_help = "EXAMPLES:\n  ikk init\n  ikk install ripgrep --uri BurntSushi/ripgrep --version 14.1.1\n  ikk sync\n  ikk check\n  ikk upgrade\n  ikk info ripgrep\n  ikk remove ripgrep\n  ikk uninstall --yes",
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
    /// Initialise ikk — creates ~/.ikk and adds to PATH
    #[command(visible_alias = "setup")]
    Init(init::InitArgs),

    /// Install a package from a forge, URL template, or local path
    #[command(visible_alias = "add")]
    Install(add::AddArgs),

    /// Sync installed packages to match ikk.toml
    Sync(sync::SyncArgs),

    /// Remove a package
    #[command(visible_alias = "rm")]
    Remove(remove::RemoveArgs),

    /// Upgrade one or all packages
    Upgrade(upgrade::UpgradeArgs),

    /// Verify integrity of lock file and all binaries
    Check,

    /// Show installed package details
    Info(info::InfoArgs),

    /// List configured packages
    #[command(visible_alias = "ls")]
    List(list::ListArgs),

    /// Run a binary from an installed package root
    Run(run::RunArgs),

    /// Get or set config values
    Config(config::ConfigArgs),

    /// Remove unused store entries
    #[command(visible_alias = "clean")]
    Gc(gc::GcArgs),

    /// Update ikk itself
    SelfUpdate(self_update::SelfUpdateArgs),

    /// Completely remove ikk from this machine
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

    // Remove `{exe}.old`/`{exe}.new` left by a previous self-update (the OS
    // releases the lock on the old binary at process exit).
    self_update::sweep_stale_update_files();

    let home = match cli.home {
        Some(p) => ikk_core::IkkHome::new(p),
        None => ikk_core::IkkHome::default(),
    };

    match cli.command {
        Command::Init(args) => init::run(args, &home),
        Command::Install(args) => add::run(args, &home).await,
        Command::Sync(args) => sync::run(args, &home).await,
        Command::Remove(args) => remove::run(args, &home),
        Command::Upgrade(args) => upgrade::run(args, &home).await,
        Command::Check => check::run(&home),
        Command::Info(args) => info::run(args, &home),
        Command::List(args) => list::run(args, &home),
        Command::Run(args) => run::run(args, &home),
        Command::Config(args) => config::run(args, &home),
        Command::Gc(args) => gc::run(args, &home),
        Command::SelfUpdate(args) => self_update::run(args, &home).await,
        Command::Uninstall(args) => uninstall::run(args, &home),
    }
}
