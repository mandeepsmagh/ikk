use anyhow::Result;
use clap::Args;
use ikk_core::home::IkkHome;

#[derive(Args)]
pub struct RunArgs {
    /// Package name
    pub name: String,
    /// Binary to run inside the package
    pub binary: String,
    /// Arguments to pass to the binary
    #[arg(last = true)]
    pub args: Vec<String>,
}

pub fn run(_args: RunArgs, _home: &IkkHome) -> Result<()> {
    anyhow::bail!("ikk run not yet implemented (Stage 4 — multi-file packages)");
}
