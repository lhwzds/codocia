use anyhow::Result;
use clap::{Parser, Subcommand};
use codocia::{
    CheckConfig, PlainServeConfig, SiteBuildConfig, SiteConfig, SiteServeConfig, SnapshotConfig,
    check, generate_starlight_site, init, serve_plain_docs, serve_starlight_site, skill_prompt,
    snapshot, starlight_build,
};
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
    Site {
        #[command(subcommand)]
        command: Option<SiteCommand>,
    },
    Serve {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "docs")]
        docs: PathBuf,
        #[arg(long, default_value = ".codocia/starlight")]
        output: PathBuf,
        #[arg(long, default_value = "Documentation")]
        title: String,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 4321)]
        port: u16,
        #[arg(long)]
        plain: bool,
    },
    Skill,
}

#[derive(Debug, Subcommand)]
enum SiteCommand {
    Generate {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "docs")]
        docs: PathBuf,
        #[arg(long, default_value = ".codocia/starlight")]
        output: PathBuf,
        #[arg(long, default_value = "Documentation")]
        title: String,
    },
    Build {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "docs")]
        docs: PathBuf,
        #[arg(long, default_value = ".codocia/starlight")]
        output: PathBuf,
        #[arg(long, default_value = "Documentation")]
        title: String,
        #[arg(long)]
        skip_install: bool,
    },
    Serve {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "docs")]
        docs: PathBuf,
        #[arg(long, default_value = ".codocia/starlight")]
        output: PathBuf,
        #[arg(long, default_value = "Documentation")]
        title: String,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 4321)]
        port: u16,
        #[arg(long)]
        skip_install: bool,
    },
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
        Some(Command::Site { command }) => match command {
            None => generate_starlight_site(&SiteConfig {
                workspace: PathBuf::from("."),
                docs: PathBuf::from("docs"),
                output: PathBuf::from(".codocia/starlight"),
                title: "Documentation".to_string(),
            })
            .map(|_| ()),
            Some(SiteCommand::Generate {
                workspace,
                docs,
                output,
                title,
            }) => generate_starlight_site(&SiteConfig {
                workspace,
                docs,
                output,
                title,
            })
            .map(|_| ()),
            Some(SiteCommand::Build {
                workspace,
                docs,
                output,
                title,
                skip_install,
            }) => starlight_build(&SiteBuildConfig {
                site: SiteConfig {
                    workspace,
                    docs,
                    output,
                    title,
                },
                skip_install,
            }),
            Some(SiteCommand::Serve {
                workspace,
                docs,
                output,
                title,
                host,
                port,
                skip_install,
            }) => serve_starlight_site(&SiteServeConfig {
                site: SiteConfig {
                    workspace,
                    docs,
                    output,
                    title,
                },
                host,
                port,
                skip_install,
            }),
        },
        Some(Command::Serve {
            workspace,
            docs,
            output,
            title,
            host,
            port,
            plain,
        }) => {
            if plain {
                serve_plain_docs(&PlainServeConfig {
                    workspace,
                    docs,
                    host,
                    port,
                })
            } else {
                serve_starlight_site(&SiteServeConfig {
                    site: SiteConfig {
                        workspace,
                        docs,
                        output,
                        title,
                    },
                    host,
                    port,
                    skip_install: false,
                })
            }
        }
    }
}
