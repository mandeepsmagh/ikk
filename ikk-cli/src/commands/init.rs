use anyhow::Result;
use clap::Args;
use ikk_core::{
    config::Config,
    home::IkkHome,
    shell::{Shell, install_path_integration},
};

#[derive(Args)]
pub struct InitArgs {
    /// Skip all prompts — for scripting and automation
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

    /// Show what would happen without making changes
    #[arg(long)]
    pub dry_run: bool,
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

pub async fn run(args: InitArgs, home: &IkkHome) -> Result<()> {
    if args.dry_run {
        println!("would create: {}", home.root.display());
        println!("would create: {}", home.config_file().display());
        println!("would add to PATH: {}", home.bin_dir().display());
        return Ok(());
    }

    // create directory layout
    home.init_dirs()?;

    // resolve default remote
    let remote =
        if args.silent { args.remote.clone() } else { prompt_remote(args.remote.as_deref())? };

    // write config if not already present
    let config_path = home.config_file();
    if !config_path.exists() {
        let mut config = Config::default();
        config.defaults.remote = remote.clone();
        config.save(&config_path)?;
        println!("created {}", config_path.display());
    } else {
        println!("config already exists — skipping");
    }

    // shell integration
    if !args.no_shell {
        let shell = args.shell.map(Shell::from).unwrap_or_else(Shell::detect);
        if args.dry_run {
            println!("would add {} to PATH in {:?}", home.bin_dir().display(), shell.rc_file());
        } else {
            match install_path_integration(&shell, &home.bin_dir())? {
                true => {
                    println!("added {} to PATH in {:?}", home.bin_dir().display(), shell.rc_file())
                }
                false => println!("PATH already configured"),
            }
        }
    }

    println!("\nikk is ready.");

    if let Some(r) = &remote {
        println!("default remote: {r}");
    } else {
        println!("no default remote set — specify host in each package source");
    }

    if !args.silent {
        println!("\nrestart your shell or run:");
        if let Some(rc) = Shell::detect().rc_file() {
            println!("  source {}", rc.display());
        }
    }

    Ok(())
}

fn prompt_remote(preset: Option<&str>) -> Result<Option<String>> {
    if let Some(r) = preset {
        return Ok(Some(r.to_string()));
    }

    print!("default remote host (e.g. github.com, codeberg.org) — leave empty to skip: ");
    use std::io::{self, BufRead, Write};
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
