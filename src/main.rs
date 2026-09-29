use anyhow::Result;
use clap::Parser;
use crbugs::cli::Cli;

fn main() -> Result<()> {
    let cli = Cli::parse();
    crbugs::run(cli)
}
