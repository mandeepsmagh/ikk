use anyhow::Result;
use clap::Args;
use ikk_core::{
    config::Config,
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
    if config_path.exists() {
        println!("config already exists — skipping");
    } else {
        let mut config = Config::default();
        config.defaults.remote.clone_from(&remote);
        config.save(&config_path)?;
        println!("created {}", config_path.display());
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

    if let Some(r) = &remote {
        println!("default remote: {r}");
    } else {
        println!("no default remote set — specify host in each package URI");
    }

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
