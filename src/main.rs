use anyhow::Result;
use clap::{Parser, Subcommand};
use codocia::{Config, check, generate, init};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "codocia")]
#[command(about = "Generate readable Markdown documentation from code.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    Generate {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "docs")]
        out: PathBuf,
    },
    Check {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "docs")]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { path } => init(path),
        Command::Generate { workspace, out } => generate(&Config { workspace, out }),
        Command::Check { workspace, out } => check(&Config { workspace, out }),
    }
}
