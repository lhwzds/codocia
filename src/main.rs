use anyhow::Result;
use clap::{Parser, Subcommand};
use codocia::{CheckConfig, SnapshotConfig, check, init, snapshot};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "codocia")]
#[command(about = "Keep Markdown docs synchronized with fast-moving code.")]
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
    Snapshot {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "docs")]
        docs: PathBuf,
    },
    Check {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "docs")]
        docs: PathBuf,
        #[arg(long)]
        base: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { path } => init(path),
        Command::Snapshot { workspace, docs } => snapshot(&SnapshotConfig { workspace, docs }),
        Command::Check {
            workspace,
            docs,
            base,
        } => check(&CheckConfig {
            workspace,
            docs,
            base,
        }),
    }
}
