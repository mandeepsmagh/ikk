use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use ikk_core::{config::Config, home::IkkHome};

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Get a config value (e.g. 'ikk config get defaults.remote')
    Get(GetArgs),
    /// Set a config value (e.g. 'ikk config set defaults.remote github.com')
    Set(SetArgs),
}

#[derive(Args)]
pub struct GetArgs {
    /// Config key in dot notation (e.g. defaults.remote, security.min_release_age_days)
    pub key: String,
}

#[derive(Args)]
pub struct SetArgs {
    /// Config key in dot notation
    pub key: String,
    /// Value to set
    pub value: String,
}

pub fn run(args: ConfigArgs, home: &IkkHome) -> Result<()> {
    match args.action {
        ConfigAction::Get(a) => run_get(a, home),
        ConfigAction::Set(a) => run_set(a, home),
    }
}

fn run_get(args: GetArgs, home: &IkkHome) -> Result<()> {
    let config = Config::load(&home.config_file())?;

    match args.key.as_str() {
        "defaults.remote" => {
            let val = config.defaults.remote.as_deref().unwrap_or("(not set)");
            println!("{val}");
        }
        "security.min_release_age_days" => {
            println!("{}", config.security.min_release_age_days);
        }
        _ => {
            bail!(
                "unknown config key '{}' — try defaults.remote or security.min_release_age_days",
                args.key
            );
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
            let days: u64 = args
                .value
                .parse()
                .map_err(|_| anyhow::anyhow!("value must be a number (e.g. 3)"))?;
            config.security.min_release_age_days = days;
        }
        _ => {
            bail!(
                "unknown config key '{}' — try defaults.remote or security.min_release_age_days",
                args.key
            );
        }
    }

    config.save(&home.config_file())?;
    println!("set {} = {}", args.key, args.value);
    Ok(())
}
