use anyhow::Result;
use clap::{Parser, Subcommand};
use codocia::{CheckConfig, SnapshotConfig, check, init, skill_prompt, snapshot};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "codocia")]
#[command(about = "Keep Markdown docs synchronized with fast-moving code.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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
    Skill,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None | Some(Command::Skill) => {
            print!("{}", skill_prompt());
            Ok(())
        }
        Some(Command::Init { path }) => init(path),
        Some(Command::Snapshot { workspace, docs }) => {
            snapshot(&SnapshotConfig { workspace, docs })
        }
        Some(Command::Check {
            workspace,
            docs,
            base,
        }) => check(&CheckConfig {
            workspace,
            docs,
            base,
        }),
    }
}
