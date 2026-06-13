use anyhow::Result;
use clap::Args;
use ikk_core::{home::IkkHome, ops};
use std::io::{self, BufRead, Write};

#[derive(Args)]
pub struct UninstallArgs {
    /// Skip confirmation prompt
    #[arg(long, short)]
    pub yes: bool,
}

pub fn run(args: UninstallArgs, home: &IkkHome) -> Result<()> {
    if !home.exists() {
        println!("ikk is not installed at {}", home.root.display());
        return Ok(());
    }

    if !args.yes {
        print!(
            "this will remove {} and all installed packages. continue? [y/N] ",
            home.root.display()
        );
        io::stdout().flush()?;

        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;

        if !line.trim().eq_ignore_ascii_case("y") {
            println!("aborted");
            return Ok(());
        }
    }

    ops::self_uninstall(home)?;

    println!("ikk removed from {}", home.root.display());
    println!("restart your shell to complete removal");
    Ok(())
}
