use anyhow::Result;
use clap::Parser;
use crbugs::cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    crbugs::run(cli).await
}
