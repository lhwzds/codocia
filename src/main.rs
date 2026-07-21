use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use codocia::generate::generate_docs;
use codocia::generation_model::{DocumentationDepth, GenerateConfig};
use codocia::{
    CheckConfig, PlainServeConfig, SiteBuildConfig, SiteConfig, SiteServeConfig, SnapshotConfig,
    check, generate_starlight_site, init, serve_plain_docs, serve_starlight_site, skill_prompt,
    snapshot, starlight_build,
};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "codocia")]
#[command(about = "Generate complete, source-grounded Markdown documentation with Codex.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate a complete managed Markdown documentation system.
    Generate {
        /// Optional documentation goal or emphasis.
        #[arg(value_name = "GOAL", num_args = 0..)]
        goal: Vec<String>,
        /// Repository root to inspect.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Markdown output directory, relative to the workspace by default.
        #[arg(long, default_value = "docs")]
        docs: PathBuf,
        /// Language requested for generated documentation.
        #[arg(long, default_value = "en")]
        language: String,
        /// Optional second complete language, emitted below docs/<normalized-locale>/.
        #[arg(long)]
        secondary_language: Option<String>,
        /// Intended readers for the generated documentation.
        #[arg(long, default_value = "developers")]
        audience: String,
        /// Documentation density and level of detail.
        #[arg(long, value_enum, default_value_t = DepthArg::Deep)]
        depth: DepthArg,
        /// Codex model override; omit to use the executable default.
        #[arg(long)]
        model: Option<String>,
        /// Include only inventory paths matching an anchored workspace-relative pattern.
        #[arg(long = "include", value_name = "PATTERN")]
        include: Vec<String>,
        /// Exclude inventory paths matching an anchored workspace-relative pattern.
        #[arg(long = "exclude", value_name = "PATTERN")]
        exclude: Vec<String>,
        /// Authorize the exact reviewed scope named by --upload-scope-fingerprint.
        #[arg(long)]
        allow_code_upload: bool,
        /// Exact `sha256:...` upload-scope fingerprint printed by a local preflight.
        #[arg(long, value_name = "SHA256")]
        upload_scope_fingerprint: Option<String>,
        /// Stop after Codex creates the plan; this still sends the allowlisted files.
        #[arg(long = "plan-only", alias = "dry-run")]
        dry_run: bool,
        /// Resume the latest incomplete run, reusing only fingerprint-matching artifacts.
        #[arg(long)]
        resume: bool,
        /// DESTRUCTIVE: replace live docs with only the generated set; a run backup is kept.
        #[arg(long)]
        prune_unmanaged: bool,
        /// Build the Starlight site after Markdown verification and publication succeed.
        #[arg(long)]
        site: bool,
    },
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
        /// Trust and reuse only an existing real node_modules directory.
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
        /// Trust and reuse only an existing real node_modules directory.
        #[arg(long)]
        skip_install: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DepthArg {
    Compact,
    Standard,
    Deep,
}

impl From<DepthArg> for DocumentationDepth {
    fn from(value: DepthArg) -> Self {
        match value {
            DepthArg::Compact => Self::Compact,
            DepthArg::Standard => Self::Standard,
            DepthArg::Deep => Self::Deep,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!(
                "\n\nGeneration is opt-in because allowlisted files may be sent to the installed CLI's built-in OpenAI Codex destination; custom providers are ignored. Run `codocia generate` for a local-only exact-scope preflight, then authorize that printed fingerprint as described by `codocia generate --help`."
            );
            Ok(())
        }
        Some(Command::Generate {
            goal,
            workspace,
            docs,
            language,
            secondary_language,
            audience,
            depth,
            model,
            include,
            exclude,
            allow_code_upload,
            upload_scope_fingerprint,
            dry_run,
            resume,
            prune_unmanaged,
            site,
        }) => generate_docs(&GenerateConfig {
            workspace,
            docs_root: docs,
            goal: (!goal.is_empty()).then(|| goal.join(" ")),
            language,
            secondary_language,
            audience,
            depth: depth.into(),
            include,
            exclude,
            model,
            allow_code_upload,
            upload_scope_fingerprint,
            max_repair_rounds: 2,
            plan_only: dry_run,
            resume,
            prune_unmanaged,
            generate_site: site,
        })
        .map(|_| ()),
        Some(Command::Skill) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_options_after_goal_are_not_consumed_as_goal_text() {
        let cli = Cli::try_parse_from([
            "codocia",
            "generate",
            "document the runtime",
            "--workspace",
            "/tmp/example",
            "--allow-code-upload",
            "--dry-run",
            "--secondary-language",
            "zh-CN",
            "--include",
            "src/**",
            "--exclude",
            "src/generated/**",
        ])
        .expect("generate command should parse");

        let Some(Command::Generate {
            goal,
            workspace,
            allow_code_upload,
            dry_run,
            secondary_language,
            include,
            exclude,
            ..
        }) = cli.command
        else {
            panic!("expected generate command");
        };

        assert_eq!(goal, vec!["document the runtime"]);
        assert_eq!(workspace, PathBuf::from("/tmp/example"));
        assert!(allow_code_upload);
        assert!(dry_run);
        assert_eq!(secondary_language.as_deref(), Some("zh-CN"));
        assert_eq!(include, vec!["src/**"]);
        assert_eq!(exclude, vec!["src/generated/**"]);
    }

    #[test]
    fn generate_help_explains_stateful_options_without_exposing_fresh() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("generate")
            .expect("generate subcommand")
            .render_long_help()
            .to_string();

        assert!(help.contains("--resume"));
        assert!(help.contains("latest incomplete run"));
        assert!(help.contains("--prune-unmanaged"));
        assert!(help.contains("DESTRUCTIVE"));
        assert!(help.contains("run backup is kept"));
        assert!(help.contains("--upload-scope-fingerprint"));
        assert!(help.contains("Exact `sha256:...` upload-scope fingerprint"));
        assert!(help.contains("--site"));
        assert!(help.contains("after Markdown verification"));
        assert!(help.contains("--secondary-language"));
        assert!(help.contains("docs/<normalized-locale>/"));
        assert!(!help.contains("--fresh"));
    }
}
