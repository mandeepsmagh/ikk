use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use ikk_core::{config::Config, home::IkkHome};

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Get a config value
    Get(GetArgs),
    /// Set a config value
    Set(SetArgs),
}

#[derive(Args)]
pub struct GetArgs {
    /// Config key (e.g. defaults.remote, security.min_release_age_days)
    pub key: String,
}

#[derive(Args)]
pub struct SetArgs {
    /// Config key
    pub key: String,
    /// Value to set
    pub value: String,
}

pub fn run(args: ConfigArgs, home: &IkkHome) -> Result<()> {
    match args.action {
        Some(ConfigAction::Get(a)) => run_get(a, home),
        Some(ConfigAction::Set(a)) => run_set(a, home),
        None => run_show_all(home),
    }
}

fn run_get(args: GetArgs, home: &IkkHome) -> Result<()> {
    let config = Config::load(&home.config_file())?;

    match args.key.as_str() {
        "defaults.remote" => {
            println!("{}", config.defaults.remote.as_deref().unwrap_or("(not set)"));
        }
        "security.min_release_age_days" => {
            println!("{}", config.security.min_release_age_days);
        }
        _ => {
            bail!("unknown config key '{}'", args.key);
        }
    }

    Ok(())
}

fn run_set(args: SetArgs, home: &IkkHome) -> Result<()> {
    let mut config = Config::load(&home.config_file())?;

    match args.key.as_str() {
        "defaults.remote" => {
            config.defaults.remote = Some(args.value.clone());
        }
        "security.min_release_age_days" => {
            let days: u64 =
                args.value.parse().map_err(|_| anyhow::anyhow!("value must be a number"))?;
            config.security.min_release_age_days = days;
        }
        _ => {
            bail!("unknown config key '{}'", args.key);
        }
    }

    config.save(&home.config_file())?;
    println!("set {} = {}", args.key, args.value);
    Ok(())
}

fn run_show_all(home: &IkkHome) -> Result<()> {
    let config = Config::load(&home.config_file())?;

    println!(
        "defaults.remote               {}",
        config.defaults.remote.as_deref().unwrap_or("(not set)")
    );
    println!("security.min_release_age_days {}", config.security.min_release_age_days);
    println!("packages                      {} configured", config.packages.len());
    if !config.remotes.is_empty() {
        let hosts: Vec<&str> = config.remotes.iter().map(|r| r.host.as_str()).collect();
        println!("remotes                       {}", hosts.join(", "));
    }
    Ok(())
}
