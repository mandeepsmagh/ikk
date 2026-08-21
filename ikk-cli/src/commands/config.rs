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
    /// Config key (e.g. `defaults.remote`, `security.min_release_age_days`)
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
        "defaults.self_update_repo" => {
            println!("{}", config.defaults.self_update_repo);
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
        "defaults.self_update_repo" => {
            validate_self_update_repo(&args.value)?;
            config.defaults.self_update_repo = args.value.clone();
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

/// Validate the `owner/repo` shape of a self-update repo value.
fn validate_self_update_repo(value: &str) -> Result<()> {
    if value.matches('/').count() != 1 || value.trim().is_empty() {
        bail!("value must be in owner/repo form (got '{}')", value);
    }
    Ok(())
}

fn run_show_all(home: &IkkHome) -> Result<()> {
    let config = Config::load(&home.config_file())?;

    println!(
        "defaults.remote               {}",
        config.defaults.remote.as_deref().unwrap_or("(not set)")
    );
    println!("defaults.self_update_repo     {}", config.defaults.self_update_repo);
    println!("security.min_release_age_days {}", config.security.min_release_age_days);
    println!("packages                      {} configured", config.packages.len());
    if !config.remotes.is_empty() {
        let hosts: Vec<&str> = config.remotes.iter().map(|r| r.host.as_str()).collect();
        println!("remotes                       {}", hosts.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_self_update_repo_accepts_owner_repo() {
        assert!(validate_self_update_repo("mandeepsmagh/ikk").is_ok());
    }

    #[test]
    fn validate_self_update_repo_rejects_no_slash() {
        assert!(validate_self_update_repo("not-a-repo").is_err());
    }

    #[test]
    fn validate_self_update_repo_rejects_multiple_slashes() {
        assert!(validate_self_update_repo("a/b/c").is_err());
    }

    #[test]
    fn validate_self_update_repo_rejects_empty() {
        assert!(validate_self_update_repo("").is_err());
        assert!(validate_self_update_repo("   ").is_err());
    }
}
