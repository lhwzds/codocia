use anyhow::Result;
use clap::{Parser, Subcommand};
use codocia::book;
use codocia::{
    CheckConfig, PlainServeConfig, SiteBuildConfig, SiteConfig, SiteServeConfig, SnapshotConfig,
    check, generate_starlight_site, init, serve_plain_docs, serve_starlight_site, skill_prompt,
    snapshot, starlight_build,
};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "codocia")]
#[command(about = "Turn what an agent learns about a codebase into small, source-grounded docs.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a book (notes + outline) for a repository.
    Init {
        /// Repository to document.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Book directory to create (default: <repo>/.codocia).
        #[arg(long)]
        book: Option<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        /// Output language for generated labels: en or zh.
        #[arg(long, default_value = "en")]
        lang: String,
    },
    /// Show pages, budgets, coverage areas, and check results.
    Status {
        #[arg(long)]
        book: Option<PathBuf>,
        /// Maximum number of coverage areas to show.
        #[arg(long, default_value_t = 30)]
        areas: usize,
        /// Also list the files under this directory with their cited state.
        #[arg(long)]
        area: Option<String>,
    },
    /// Manage notes.
    Note {
        #[command(subcommand)]
        command: NoteCommand,
    },
    /// Pin new evidence and follow moved code; --force re-pins reviewed stale notes.
    Pin {
        #[arg(long)]
        book: Option<PathBuf>,
        /// Notes (`page/id` or `id`) whose stale evidence was reviewed.
        #[arg(long = "force")]
        force: Vec<String>,
    },
    /// Move the book's pinned commit to the repository HEAD once check passes.
    Rebase {
        #[arg(long)]
        book: Option<PathBuf>,
    },
    /// Record that translations match their current source (see `langs:` and `translations:`).
    Sync {
        #[arg(long)]
        book: Option<PathBuf>,
        /// A language, `<lang>/book`, `<lang>/<page>/<id>`, or a file from `translations:`.
        targets: Vec<String>,
    },
    /// Validate evidence, links, and budgets.
    Check {
        #[arg(long)]
        book: Option<PathBuf>,
        /// Only report problems of this page.
        #[arg(long)]
        page: Option<String>,
    },
    /// Mine git history for hot files, structural commits, and design changes.
    History {
        #[arg(long)]
        book: Option<PathBuf>,
        #[arg(long, default_value_t = 15)]
        limit: usize,
        /// Recent commits scanned for hot files and wide commits.
        #[arg(long, default_value_t = 1000)]
        window: usize,
    },
    /// Print the generated code structure: areas, dependencies, pages.
    Map {
        #[arg(long)]
        book: Option<PathBuf>,
        #[arg(long, default_value_t = 30)]
        areas: usize,
    },
    /// Print one rendered page to stdout (`structure` for the generated map).
    Show {
        #[arg(long)]
        book: Option<PathBuf>,
        page: String,
    },
    /// Compile notes into Markdown pages, index.md, and all.md.
    Build {
        #[arg(long)]
        book: Option<PathBuf>,
        /// Output directory (default: <repo>/docs for .codocia, otherwise <book>/out).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Build even when check reports errors.
        #[arg(long)]
        force: bool,
    },
    /// Legacy in-repository drift checks for docs/ with `covers` frontmatter.
    Drift {
        #[command(subcommand)]
        command: DriftCommand,
    },
    /// Generate, build, or serve a Starlight site from a Markdown directory.
    Site {
        #[command(subcommand)]
        command: Option<SiteCommand>,
    },
    /// Serve a Markdown directory (Starlight, or --plain without Node).
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
    /// Print the agent workflow guide.
    Skill,
}

#[derive(Debug, Subcommand)]
enum NoteCommand {
    /// Save a note. Body comes from --body or stdin.
    Add {
        #[arg(long)]
        book: Option<PathBuf>,
        #[arg(long)]
        page: String,
        #[arg(long)]
        id: String,
        /// Section heading; empty for a page introduction. Kept on --replace if omitted.
        #[arg(long)]
        title: Option<String>,
        /// Sort key inside the page (default 50).
        #[arg(long)]
        order: Option<i64>,
        /// path[:start-end], commit:<sha>, pr:<n>, or issue:<n>. Repeatable.
        #[arg(long = "evidence", short = 'e')]
        evidence: Vec<String>,
        /// Evidence appended to the note's existing evidence. Repeatable.
        #[arg(long = "add-evidence")]
        add_evidence: Vec<String>,
        #[arg(long)]
        body: Option<String>,
        /// Update an existing note; omitted title, order, evidence, and body are kept.
        #[arg(long)]
        replace: bool,
    },
    /// List notes with order, length, and file path.
    List {
        #[arg(long)]
        book: Option<PathBuf>,
        #[arg(long)]
        page: Option<String>,
    },
    /// Delete a note (`page/id`).
    Rm {
        #[arg(long)]
        book: Option<PathBuf>,
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum DriftCommand {
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
    reset_sigpipe();
    let cli = Cli::parse();
    match cli.command {
        None | Some(Command::Skill) => {
            print!("{}", skill_prompt());
            Ok(())
        }
        Some(Command::Init {
            repo,
            book,
            title,
            lang,
        }) => book::init(&book::InitOptions {
            repo: match repo {
                Some(repo) => repo,
                None => book::default_repo()?,
            },
            book,
            title,
            lang,
        }),
        Some(Command::Status { book, areas, area }) => {
            book::status(&book::find_book(book.as_deref())?, areas, area.as_deref())
        }
        Some(Command::Note {
            command:
                NoteCommand::Add {
                    book,
                    page,
                    id,
                    title,
                    order,
                    evidence,
                    add_evidence,
                    body,
                    replace,
                },
        }) => {
            let body = match body {
                Some(body) => body,
                None if std::io::stdin().is_terminal() => String::new(),
                None => {
                    let mut body = String::new();
                    std::io::stdin().read_to_string(&mut body)?;
                    body
                }
            };
            book::note_add(&book::NoteAddOptions {
                book: book::find_book(book.as_deref())?,
                page,
                id,
                title,
                order,
                evidence,
                add_evidence,
                body,
                replace,
            })
        }
        Some(Command::Note {
            command: NoteCommand::List { book, page },
        }) => book::note_list(&book::find_book(book.as_deref())?, page.as_deref()),
        Some(Command::Note {
            command: NoteCommand::Rm { book, name },
        }) => book::note_rm(&book::find_book(book.as_deref())?, &name),
        Some(Command::Pin { book, force }) => book::pin(&book::find_book(book.as_deref())?, &force),
        Some(Command::Rebase { book }) => book::rebase(&book::find_book(book.as_deref())?),
        Some(Command::Sync { book, targets }) => {
            codocia::translate::sync(&book::find_book(book.as_deref())?, &targets)
        }
        Some(Command::Check { book, page }) => {
            book::check(&book::find_book(book.as_deref())?, page.as_deref())
        }
        Some(Command::History {
            book,
            limit,
            window,
        }) => book::history(&book::find_book(book.as_deref())?, limit, window),
        Some(Command::Map { book, areas }) => book::map(&book::find_book(book.as_deref())?, areas),
        Some(Command::Show { book, page }) => book::show(&book::find_book(book.as_deref())?, &page),
        Some(Command::Build { book, out, force }) => {
            book::build(&book::find_book(book.as_deref())?, out, force)
        }
        Some(Command::Drift { command }) => match command {
            DriftCommand::Init { path } => init(path),
            DriftCommand::Snapshot { workspace, docs } => {
                snapshot(&SnapshotConfig { workspace, docs })
            }
            DriftCommand::Check {
                workspace,
                docs,
                base,
            } => check(&CheckConfig {
                workspace,
                docs,
                base,
            }),
        },
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

/// Exit quietly when stdout is closed early (for example `codocia map | head`)
/// instead of panicking on a broken pipe.
#[cfg(unix)]
fn reset_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}
