use anyhow::Result;
use clap::Args;
use ikk_core::{
    config::{Config, DEFAULT_SELF_UPDATE_REPO},
    home::IkkHome,
    shell::{Shell, write_rc},
};

#[derive(Args)]
pub struct InitArgs {
    /// Skip prompts — for scripting
    #[arg(long, short)]
    pub silent: bool,

    /// Default remote host (e.g. github.com, codeberg.org)
    #[arg(long)]
    pub remote: Option<String>,

    /// Shell to configure PATH for (auto-detected if not set)
    #[arg(long, value_enum)]
    pub shell: Option<ShellArg>,

    /// Do not modify shell rc file
    #[arg(long)]
    pub no_shell: bool,
}

#[derive(clap::ValueEnum, Clone)]
pub enum ShellArg {
    Zsh,
    Bash,
    Fish,
    Nushell,
    Powershell,
}

impl From<ShellArg> for Shell {
    fn from(s: ShellArg) -> Self {
        match s {
            ShellArg::Zsh => Shell::Zsh,
            ShellArg::Bash => Shell::Bash,
            ShellArg::Fish => Shell::Fish,
            ShellArg::Nushell => Shell::Nushell,
            ShellArg::Powershell => Shell::PowerShell,
        }
    }
}

pub fn run(args: InitArgs, home: &IkkHome) -> Result<()> {
    home.init_dirs()?;

    let remote =
        if args.silent { args.remote.clone() } else { prompt_remote(args.remote.as_deref())? };

    let config_path = home.config_file();
    let created = !config_path.exists();

    let mut config = if created { Config::default() } else { Config::load(&config_path)? };

    // `self_update_repo` is always set by default — and persisted, not just
    // defaulted in memory — so it's visible and editable in ikk.toml. Backfill
    // it if a config was created elsewhere (or by an older ikk) without it.
    // A user-set value (e.g. a fork) is never touched.
    let had_repo = created
        || std::fs::read_to_string(&config_path)
            .map(|s| s.contains("self_update_repo"))
            .unwrap_or(false);

    let mut changed = created || !had_repo;
    if config.defaults.self_update_repo.trim().is_empty() {
        config.defaults.self_update_repo = DEFAULT_SELF_UPDATE_REPO.to_string();
        changed = true;
    }

    // Backfill the default remote if one was supplied and none is set yet.
    // An existing remote is left alone (explicitly configured by the user).
    if let Some(r) = &remote
        && config.defaults.remote.is_none()
    {
        config.defaults.remote = Some(r.clone());
        changed = true;
    }

    if changed {
        config.save(&config_path)?;
        if created {
            println!("created {}", config_path.display());
        } else {
            println!("updated {}", config_path.display());
        }
    } else {
        println!("config already up to date — {}", config_path.display());
    }

    if !args.no_shell {
        let shell = args.shell.map_or_else(Shell::detect, Shell::from);
        match shell.rc_file() {
            Some(rc) => {
                let dir = rc.parent().unwrap_or(std::path::Path::new("."));
                write_rc(dir, shell.as_str(), home)?;
                println!("added {} to PATH in {}", home.bin_dir().display(), rc.display());
            }
            None => {
                eprintln!("unknown shell — add {} to your PATH manually", home.bin_dir().display());
            }
        }
    }

    println!("\nikk is ready.");

    match &config.defaults.remote {
        Some(r) => println!("default remote: {r}"),
        None => println!("no default remote set — specify host in each package URI"),
    }

    println!("self-update repo: {} (edit in ikk.toml to change)", config.defaults.self_update_repo);

    if !args.silent {
        println!("\nrestart your shell or run:");
        if let Some(rc) = Shell::detect().rc_file() {
            println!("  source {}", rc.display());
        }
    }

    Ok(())
}

use std::io::{self, BufRead, Write};

fn prompt_remote(preset: Option<&str>) -> Result<Option<String>> {
    if let Some(r) = preset {
        return Ok(Some(r.to_string()));
    }

    print!("default remote host (e.g. github.com) — enter to skip: ");
    io::stdout().flush()?;

    let stdin = io::stdin();
    let line = stdin.lock().lines().next().transpose()?.unwrap_or_default().trim().to_string();

    if line.is_empty() {
        println!("no default remote set");
        Ok(None)
    } else {
        Ok(Some(line))
    }
}
