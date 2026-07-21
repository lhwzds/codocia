//! Library API for Codocia.

#![recursion_limit = "256"]

pub mod codex_runner;
pub mod generate;
pub mod generation_model;
pub mod generation_verify;
mod integrity;
pub mod inventory;

use anyhow::{Context, Result, bail};
use integrity::{SHA256_ALGORITHM, is_sha256_fingerprint, sha256_fingerprint};
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const DIFF_LINE_LIMIT: usize = 80;
const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const SITE_OWNER_MARKER: &str = ".codocia-site.json";
static SITE_STAGING_NONCE: AtomicU64 = AtomicU64::new(0);
const CHECK_PASS_QUALITY_NOTE: &str = "quality note: check passed only means covers and snapshots are current. Do not bulk-generate templated, low-information docs just to satisfy coverage; read codocia.md, keep Markdown as the source of truth, and only refresh snapshots after reviewing or improving the human-readable prose.";
const DEFAULT_CODOCIA_POLICY: &str = r#"# Codocia Documentation Policy

Use this file to guide AI coding agents that update Markdown docs in this
repository. Codocia does not parse this file as machine config; generation,
review, and maintenance agents read it before authoring docs.

## Generation Safety

- `codocia generate` invokes the installed CLI's built-in OpenAI Codex service and can send it
  repository source. Run it only after reviewing scope and explicitly passing
  `--allow-code-upload`.
- Codex generation turns stay read-only. Codocia validates structured drafts
  and is the only process that publishes Markdown.
- Never include credentials, private keys, model weights, generated dependency
  trees, or other excluded content in documentation prompts.

## Defaults

- density: `standard`
- docs root: `docs/`
- source of truth: Markdown docs
- code defaults live in the CLI and library, not in a TOML file.

## Density

- `compact`: behavior delta only. Use for formatting-only, comment-only,
  test-only, or very small internal changes.
- `standard`: purpose, workflow, commands or APIs, examples, constraints,
  failure modes, and validation.
- `dense`: public contracts, invariants, edge cases, schemas, operational
  checks, compatibility notes, and maintenance rules.

## Metrics

- behavior coverage: the page explains behavior that users or agents can
  observe.
- operational completeness: the page includes commands, expected output,
  validation, and recovery steps when relevant.
- contract precision: the page defines inputs, outputs, config, schemas, APIs,
  or CLI flags exactly when they are part of the documented surface.
- maintenance context: the page records ownership, invariants, boundaries, and
  when prose should not change.
- agent usability: a coding agent can follow the page without guessing the next
  inspection, edit, command, or evidence to report.

## Anti-Template Rule

Do not create bulk, template-shaped source-file pages just to make coverage
look complete. A page that only lists modules, structs, functions, line counts,
or generic summaries is not sufficient documentation.

Generated source indexes are allowed only when they are clearly marked as
indexes and do not replace human-readable docs. Real docs should explain
behavior, contracts, invariants, failure modes, validation, and maintenance
context at the density tier chosen for the page.

## Page Defaults

- CLI and workflow pages: `standard`, prioritize operational completeness and
  agent usability.
- API, config, and schema pages: `dense`, prioritize contract precision.
- Architecture and maintenance pages: `dense`, prioritize maintenance context.
- Review notes and small behavior updates: `compact`, prioritize behavior
  coverage.
"#;

#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    pub workspace: PathBuf,
    pub docs: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CheckConfig {
    pub workspace: PathBuf,
    pub docs: PathBuf,
    pub base: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SiteConfig {
    pub workspace: PathBuf,
    pub docs: PathBuf,
    pub output: PathBuf,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct SiteBuildConfig {
    pub site: SiteConfig,
    pub skip_install: bool,
}

#[derive(Debug, Clone)]
pub struct SiteServeConfig {
    pub site: SiteConfig,
    pub host: String,
    pub port: u16,
    pub skip_install: bool,
}

#[derive(Debug, Clone)]
pub struct PlainServeConfig {
    pub workspace: PathBuf,
    pub docs: PathBuf,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct CodexAgentConfig {
    pub workspace: PathBuf,
    pub docs: PathBuf,
    pub base: Option<String>,
    pub model: Option<String>,
    pub apply: bool,
    pub goal: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GoalConfig {
    pub workspace: PathBuf,
    pub docs: PathBuf,
    pub base: Option<String>,
    pub model: Option<String>,
    pub goal: String,
    pub dry_run: bool,
}

pub fn skill_prompt() -> &'static str {
    include_str!("../SKILL.md")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocPage {
    relative_path: PathBuf,
    frontmatter: Option<Frontmatter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Frontmatter {
    covers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema_version: u32,
    hash_algorithm: String,
    commit: String,
    docs: BTreeMap<PathBuf, DocSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DocSnapshot {
    covers: Vec<String>,
    files: BTreeMap<PathBuf, String>,
}

pub fn init(path: impl AsRef<Path>) -> Result<()> {
    let root = path.as_ref();
    fs::create_dir_all(root.join("docs"))?;

    let policy_path = root.join("codocia.md");
    if !policy_path.exists() {
        fs::write(policy_path, DEFAULT_CODOCIA_POLICY)?;
    }

    let index_path = root.join("docs/index.md");
    if !index_path.exists() {
        fs::write(
            index_path,
            "---\ntitle: Documentation\n---\n\n# Documentation\n\nStart here.\n",
        )?;
    }

    Ok(())
}

pub fn snapshot(config: &SnapshotConfig) -> Result<()> {
    let workspace = normalize_dir(&config.workspace)?;
    let docs_dir = resolve_path(&workspace, &config.docs);
    let commit = current_commit(&workspace).unwrap_or_else(|_| "unknown".to_string());
    let mut snapshot = Snapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        hash_algorithm: SHA256_ALGORITHM.to_string(),
        commit,
        docs: BTreeMap::new(),
    };

    for page in read_doc_pages(&docs_dir)? {
        let Some(frontmatter) = page.frontmatter else {
            continue;
        };
        if frontmatter.covers.is_empty() {
            continue;
        }

        let files = snapshot_files(&workspace, &frontmatter.covers)?;
        snapshot.docs.insert(
            page.relative_path,
            DocSnapshot {
                covers: frontmatter.covers,
                files,
            },
        );
    }

    write_snapshot_file(&docs_dir, &snapshot)?;

    println!("snapshot updated {} doc page(s)", snapshot.docs.len());
    for path in snapshot.docs.keys() {
        println!("- {}", path.display());
    }

    Ok(())
}

pub fn check(config: &CheckConfig) -> Result<()> {
    check_with_required_files(config, None)
}

pub(crate) fn check_inventory_scope(
    config: &CheckConfig,
    included_paths: &BTreeSet<PathBuf>,
) -> Result<()> {
    check_with_required_files(config, Some(included_paths))
}

fn check_with_required_files(
    config: &CheckConfig,
    included_paths: Option<&BTreeSet<PathBuf>>,
) -> Result<()> {
    let workspace = normalize_dir(&config.workspace)?;
    let docs_dir = resolve_path(&workspace, &config.docs);
    let pages = read_doc_pages(&docs_dir)?;
    let snapshot = read_snapshot_file(&docs_dir)?;
    let workspace_code_files = if included_paths.is_none() {
        Some(collect_code_files(&workspace, &docs_dir)?)
    } else {
        None
    };
    let required_files = included_paths
        .or(workspace_code_files.as_ref())
        .expect("either an inventory scope or workspace code scan is available");
    let required_file_label = if included_paths.is_some() {
        "inventory"
    } else {
        "code"
    };
    let changed_files = if let Some(base) = &config.base {
        changed_files(&workspace, base)?
    } else {
        BTreeSet::new()
    };

    let mut covered_by_file: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    let mut broken_covers = Vec::new();
    let mut stale_docs = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    let mut missing_files = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    let mut missing_snapshots = Vec::new();

    for page in &pages {
        let Some(frontmatter) = &page.frontmatter else {
            continue;
        };
        for pattern in &frontmatter.covers {
            let matches = files_matching(&workspace, pattern)?;
            if matches.is_empty() {
                broken_covers.push((page.relative_path.clone(), pattern.clone()));
            }
            for path in matches {
                covered_by_file
                    .entry(path)
                    .or_default()
                    .push(page.relative_path.clone());
            }
        }

        if frontmatter.covers.is_empty() {
            continue;
        }

        let Some(doc_snapshot) = snapshot.docs.get(&page.relative_path) else {
            missing_snapshots.push(page.relative_path.clone());
            continue;
        };

        for (path, expected_hash) in &doc_snapshot.files {
            let absolute_path = workspace.join(path);
            if !absolute_path.exists() {
                missing_files
                    .entry(page.relative_path.clone())
                    .or_default()
                    .push(path.clone());
                continue;
            }
            let current_hash = hash_file(&absolute_path)?;
            if current_hash != *expected_hash {
                stale_docs
                    .entry(page.relative_path.clone())
                    .or_default()
                    .push(path.clone());
            }
        }
    }

    let uncovered = required_files
        .iter()
        .filter(|path| !covered_by_file.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    let uncovered_changed = changed_files
        .iter()
        .filter(|path| is_code_file(path))
        .filter(|path| !covered_by_file.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    let changed_stale = changed_files
        .iter()
        .filter_map(|path| {
            covered_by_file.get(path).map(|docs| {
                (
                    path.clone(),
                    docs.iter()
                        .filter(|doc| stale_docs.contains_key(*doc))
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            })
        })
        .filter(|(_, docs)| !docs.is_empty())
        .collect::<Vec<_>>();
    let diff_review_files = diff_review_files(&stale_docs, &uncovered_changed);

    if broken_covers.is_empty()
        && stale_docs.is_empty()
        && missing_files.is_empty()
        && missing_snapshots.is_empty()
        && uncovered.is_empty()
        && uncovered_changed.is_empty()
    {
        println!(
            "codocia check passed: {} {required_file_label} file(s), {} covered file(s)",
            required_files.len(),
            covered_by_file.len()
        );
        println!("{CHECK_PASS_QUALITY_NOTE}");
        return Ok(());
    }

    let mut message = String::from("codocia check failed");
    if !broken_covers.is_empty() {
        message.push_str("\n\nbroken covers:");
        for (doc, pattern) in broken_covers {
            message.push_str(&format!(
                "\n- {} covers `{}` but matched no files",
                doc.display(),
                pattern
            ));
        }
    }
    if !missing_snapshots.is_empty() {
        message.push_str("\n\nmissing snapshots:");
        for doc in missing_snapshots {
            message.push_str(&format!("\n- {}", doc.display()));
        }
    }
    if !stale_docs.is_empty() {
        message.push_str("\n\nstale docs:");
        for (doc, files) in stale_docs {
            message.push_str(&format!("\n- {}", doc.display()));
            for file in files {
                message.push_str(&format!("\n  changed: {}", file.display()));
            }
        }
    }
    if !missing_files.is_empty() {
        message.push_str("\n\nmissing covered files:");
        for (doc, files) in missing_files {
            message.push_str(&format!("\n- {}", doc.display()));
            for file in files {
                message.push_str(&format!("\n  missing: {}", file.display()));
            }
        }
    }
    if !uncovered_changed.is_empty() {
        message.push_str("\n\nchanged code without docs coverage:");
        for path in uncovered_changed {
            message.push_str(&format!("\n- {}", path.display()));
        }
    }
    if !changed_stale.is_empty() {
        message.push_str("\n\nchanged code with stale docs:");
        for (path, docs) in changed_stale {
            message.push_str(&format!("\n- {}", path.display()));
            for doc in docs {
                message.push_str(&format!("\n  stale doc: {}", doc.display()));
            }
        }
    }
    if !uncovered.is_empty() {
        message.push_str("\n\nuncovered code files:");
        for path in uncovered {
            message.push_str(&format!("\n- {}", path.display()));
        }
    }
    append_diff_review(
        &mut message,
        &workspace,
        config.base.as_deref(),
        &diff_review_files,
    );

    bail!(message);
}

pub fn codex_agent(config: &CodexAgentConfig) -> Result<()> {
    let _ = config;
    bail!(
        "the legacy `agent codex` API is disabled because it bypassed Codocia's upload acknowledgement, isolated analysis mirror, and transactional publication; use `generate_docs` with an explicit `allow_code_upload` acknowledgement"
    )
}

pub fn goal(config: &GoalConfig) -> Result<()> {
    let _ = config;
    bail!(
        "the legacy `goal` API is disabled because it bypassed Codocia's upload acknowledgement, isolated analysis mirror, and transactional publication; use `generate_docs` and set its `goal` field instead"
    )
}

pub fn generate_starlight_site(config: &SiteConfig) -> Result<PathBuf> {
    generate_starlight_site_inner(config, false)
}

fn generate_starlight_site_inner(
    config: &SiteConfig,
    preserve_dependency_cache: bool,
) -> Result<PathBuf> {
    let workspace = normalize_dir(&config.workspace)?;
    let docs_relative = site_workspace_relative_path(&workspace, &config.docs, "docs directory")?;
    let docs_dir = generate::secure_workspace_target(&workspace, &docs_relative, "docs directory")?;
    if !docs_dir.is_dir() {
        bail!("docs directory does not exist: {}", docs_dir.display());
    }

    let requested_output_relative =
        site_workspace_relative_path(&workspace, &config.output, "site output")?;
    let requested_output =
        generate::secure_workspace_target(&workspace, &requested_output_relative, "site output")?;
    if requested_output == workspace {
        bail!("site output must not be the workspace root");
    }
    if requested_output.exists() && !requested_output.is_dir() {
        bail!(
            "site output is not a directory: {}",
            requested_output.display()
        );
    }
    if requested_output.starts_with(&docs_dir) || docs_dir.starts_with(&requested_output) {
        bail!(
            "site output and docs directory must not contain one another: output={}, docs={}",
            requested_output.display(),
            docs_dir.display()
        );
    }
    validate_existing_site_output(&requested_output)?;

    let output_relative = unique_site_sibling(&requested_output_relative, "staging");
    let output =
        generate::secure_workspace_target(&workspace, &output_relative, "site staging output")?;
    if output.exists() {
        bail!("site staging output already exists: {}", output.display());
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create site staging parent {}", parent.display()))?;
    }
    fs::create_dir(&output)
        .with_context(|| format!("claim site staging output {}", output.display()))?;
    let mut staging_guard = SiteStagingGuard::new(output.clone());

    let site_docs_dir = generate::secure_workspace_target(
        &workspace,
        &output_relative.join("src/content/docs"),
        "site content output",
    )?;
    let raw_docs_dir = generate::secure_workspace_target(
        &workspace,
        &output_relative.join("public/md"),
        "raw Markdown output",
    )?;
    fs::create_dir_all(&site_docs_dir)
        .with_context(|| format!("create {}", site_docs_dir.display()))?;
    fs::create_dir_all(&raw_docs_dir)
        .with_context(|| format!("create {}", raw_docs_dir.display()))?;
    fs::create_dir_all(output.join("src"))
        .with_context(|| format!("create {}", output.join("src").display()))?;
    fs::create_dir_all(output.join("public"))
        .with_context(|| format!("create {}", output.join("public").display()))?;

    let mut site_sources = StarlightSources::default();
    collect_starlight_sources(&docs_dir, &mut site_sources)?;
    site_sources.markdown.sort();
    site_sources.images.sort();
    let markdown_files = site_sources.markdown;
    if markdown_files.is_empty() {
        bail!(
            "docs directory contains no Markdown files: {}",
            docs_dir.display()
        );
    }

    let known_docs = markdown_files
        .iter()
        .map(|source| relative_path(&docs_dir, source))
        .collect::<BTreeSet<_>>();
    let has_index = known_docs.iter().any(|path| is_starlight_index(path, None));
    let locales = detect_starlight_locales(&docs_dir, &known_docs)?;
    let has_root_locale = locales.is_some();
    let mut routing_docs = known_docs.clone();
    if !has_index {
        routing_docs.insert(PathBuf::from("index.md"));
    }
    validate_starlight_slugs(&routing_docs)?;
    let mut copied_docs = Vec::new();
    for source in markdown_files {
        let relative = relative_path(&docs_dir, &source);
        let content = fs::read_to_string(&source)
            .with_context(|| format!("failed to read {}", source.display()))?;
        if let Some(reason) =
            generation_verify::unsafe_renderable_markdown(strip_frontmatter(&content))
        {
            bail!(
                "refusing unsafe renderable Markdown in {}: {reason}",
                source.display()
            );
        }
        if let Some(target) = starlight_link_escaping_docs(strip_frontmatter(&content), &relative) {
            bail!(
                "refusing local Markdown target that escapes the documentation tree in {}: `{target}`",
                source.display()
            );
        }
        let site_content =
            ensure_starlight_title(&content, &relative, &routing_docs, has_root_locale);
        write_file(site_docs_dir.join(&relative), site_content.as_bytes())?;
        write_file(raw_docs_dir.join(&relative), content.as_bytes())?;
        copied_docs.push(relative);
    }
    for source in site_sources.images {
        let relative = relative_path(&docs_dir, &source);
        copy_starlight_asset(&source, &site_docs_dir.join(&relative))?;
        copy_starlight_asset(&source, &raw_docs_dir.join(&relative))?;
    }
    if !has_index {
        write_file(
            site_docs_dir.join("index.md"),
            render_synthetic_site_index(&config.title, &copied_docs).as_bytes(),
        )?;
    }

    write_file(
        secure_site_output_file(&workspace, &output_relative, Path::new("package.json"))?,
        render_site_package_json(&config.title).as_bytes(),
    )?;
    write_file(
        secure_site_output_file(&workspace, &output_relative, Path::new("astro.config.mjs"))?,
        render_astro_config(&config.title, &copied_docs, locales.as_ref()).as_bytes(),
    )?;
    write_file(
        secure_site_output_file(&workspace, &output_relative, Path::new("tsconfig.json"))?,
        SITE_TSCONFIG.as_bytes(),
    )?;
    write_file(
        secure_site_output_file(
            &workspace,
            &output_relative,
            Path::new("public/favicon.svg"),
        )?,
        SITE_FAVICON_SVG.as_bytes(),
    )?;
    write_file(
        secure_site_output_file(
            &workspace,
            &output_relative,
            Path::new("src/content.config.ts"),
        )?,
        SITE_CONTENT_CONFIG.as_bytes(),
    )?;
    write_file(
        secure_site_output_file(&workspace, &output_relative, Path::new("README.md"))?,
        render_site_readme().as_bytes(),
    )?;
    write_file(
        secure_site_output_file(&workspace, &output_relative, Path::new("public/llms.txt"))?,
        render_llms_index(&config.title, &copied_docs).as_bytes(),
    )?;
    write_file(
        secure_site_output_file(
            &workspace,
            &output_relative,
            Path::new("public/llms-full.txt"),
        )?,
        render_llms_full(&config.title, &docs_dir, &copied_docs).as_bytes(),
    )?;
    write_file(
        secure_site_output_file(&workspace, &output_relative, Path::new(SITE_OWNER_MARKER))?,
        b"{\n  \"schema_version\": 1,\n  \"kind\": \"codocia_starlight_site\"\n}\n",
    )?;

    replace_starlight_site_output(
        &workspace,
        &requested_output_relative,
        &requested_output,
        &output,
        preserve_dependency_cache,
    )?;
    staging_guard.disarm();

    println!(
        "generated Starlight docs site at {} from {} Markdown page(s)",
        requested_output.display(),
        copied_docs.len()
    );
    println!("next steps:");
    println!("- cd {}", requested_output.display());
    println!("- npm install --ignore-scripts --no-audit --no-fund");
    println!("- npm run dev");

    Ok(requested_output)
}

pub fn starlight_build(config: &SiteBuildConfig) -> Result<()> {
    let output = generate_starlight_site_inner(&config.site, config.skip_install)?;
    ensure_npm_available()?;
    if !config.skip_install {
        npm_install_if_needed(&output)?;
    }
    run_status_command(
        Command::new("npm")
            .arg("run")
            .arg("build")
            .current_dir(&output),
        "npm run build",
    )?;
    Ok(())
}

pub fn serve_starlight_site(config: &SiteServeConfig) -> Result<()> {
    let output = generate_starlight_site_inner(&config.site, config.skip_install)?;
    ensure_npm_available()?;
    if !config.skip_install {
        npm_install_if_needed(&output)?;
    }
    println!(
        "serving Starlight docs at http://{}:{}/",
        config.host, config.port
    );
    let status = Command::new("npm")
        .arg("run")
        .arg("dev")
        .arg("--")
        .arg("--host")
        .arg(&config.host)
        .arg("--port")
        .arg(config.port.to_string())
        .current_dir(&output)
        .status()
        .context("failed to run npm run dev")?;
    if !status.success() {
        bail!("npm run dev failed with status {status}");
    }
    Ok(())
}

pub fn serve_plain_docs(config: &PlainServeConfig) -> Result<()> {
    let workspace = normalize_dir(&config.workspace)?;
    let docs_relative =
        site_workspace_relative_path(&workspace, &config.docs, "plain docs directory")?;
    let docs_dir =
        generate::secure_workspace_target(&workspace, &docs_relative, "plain docs directory")?;
    if !docs_dir.is_dir() {
        bail!("docs directory does not exist: {}", docs_dir.display());
    }
    let mut startup_pages = Vec::new();
    collect_markdown_files(&docs_dir, &mut startup_pages)?;
    let address = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&address).with_context(|| format!("bind {address}"))?;
    println!("serving plain Codocia docs at http://{address}/");
    println!("press Ctrl-C to stop");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_plain_request(stream, &docs_dir) {
                    eprintln!("request failed: {error:#}");
                }
            }
            Err(error) => eprintln!("connection failed: {error}"),
        }
    }
    Ok(())
}

fn diff_review_files(
    stale_docs: &BTreeMap<PathBuf, Vec<PathBuf>>,
    uncovered_changed: &[PathBuf],
) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    for covered_files in stale_docs.values() {
        files.extend(covered_files.iter().cloned());
    }
    files.extend(uncovered_changed.iter().cloned());
    files
}

fn append_diff_review(
    message: &mut String,
    workspace: &Path,
    base: Option<&str>,
    files: &BTreeSet<PathBuf>,
) {
    if files.is_empty() {
        return;
    }

    message.push_str("\n\ngit diff review:");
    message.push_str(
        "\nHash changes mean the docs need review. Update prose only when the diff changes documented behavior.",
    );
    for path in files {
        message.push_str(&format!("\n- {}", path.display()));
        let sections = match git_diff_sections(workspace, base, path) {
            Ok(sections) => sections,
            Err(error) => {
                message.push_str(&format!("\n  diff unavailable: {error}"));
                continue;
            }
        };
        if sections.is_empty() {
            message.push_str(
                "\n  no git diff output; the file hash changed outside the selected diff range",
            );
            continue;
        }
        for section in sections {
            message.push_str(&format!("\n  {} ({})", section.label, section.command));
            push_indented_diff(message, &diff_excerpt(&section.diff), "    ");
        }
    }
}

struct DiffSection {
    label: &'static str,
    command: String,
    diff: String,
}

fn read_doc_pages(docs_dir: &Path) -> Result<Vec<DocPage>> {
    let mut paths = Vec::new();
    collect_markdown_files(docs_dir, &mut paths)?;
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let relative_path = docs_dir
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(relative_path(docs_dir, &path));
            Ok(DocPage {
                relative_path,
                frontmatter: parse_frontmatter(&content),
            })
        })
        .collect()
}

fn collect_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let root_metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", dir.display()));
        }
    };
    if root_metadata.file_type().is_symlink() {
        bail!("refusing symbolic-link Markdown root: {}", dir.display());
    }
    if !root_metadata.is_dir() {
        bail!("Markdown root is not a directory: {}", dir.display());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            bail!(
                "refusing symbolic link in Markdown tree: {}",
                path.display()
            );
        }
        if metadata.is_dir() {
            collect_markdown_files(&path, out)?;
        } else if metadata.is_file()
            && matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("md" | "mdx")
            )
        {
            out.push(path);
        }
    }
    Ok(())
}

#[derive(Default)]
struct StarlightSources {
    markdown: Vec<PathBuf>,
    images: Vec<PathBuf>,
}

fn collect_starlight_sources(dir: &Path, out: &mut StarlightSources) -> Result<()> {
    let metadata = fs::symlink_metadata(dir)
        .with_context(|| format!("failed to inspect {}", dir.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "refusing symbolic link in Starlight documentation tree: {}",
            dir.display()
        );
    }
    if !metadata.is_dir() {
        bail!(
            "Starlight documentation path is not a directory: {}",
            dir.display()
        );
    }

    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if file_type.is_symlink() {
            bail!(
                "refusing symbolic link in Starlight documentation tree: {}",
                path.display()
            );
        }
        if file_type.is_dir() {
            collect_starlight_sources(&path, out)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if extension.eq_ignore_ascii_case("md") {
            out.markdown.push(path);
        } else if extension.eq_ignore_ascii_case("mdx") {
            bail!(
                "refusing executable MDX in Starlight documentation tree: {}; use inert .md Markdown",
                path.display()
            );
        } else if extension.eq_ignore_ascii_case("svg") {
            bail!(
                "refusing active SVG asset in Starlight documentation tree: {}; use an inert raster image",
                path.display()
            );
        } else if is_starlight_image_extension(extension) {
            out.images.push(path);
        }
    }
    Ok(())
}

pub(crate) fn is_starlight_image_extension(extension: &str) -> bool {
    ["png", "jpg", "jpeg", "gif", "webp", "avif", "ico"]
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
}

fn parse_frontmatter(content: &str) -> Option<Frontmatter> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let text = &rest[..end];
    Some(Frontmatter {
        covers: parse_covers(text),
    })
}

fn parse_covers(text: &str) -> Vec<String> {
    let mut covers = Vec::new();
    let mut in_covers = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "covers:" {
            in_covers = true;
            continue;
        }
        if in_covers && is_top_level_key(line) {
            break;
        }
        if in_covers && let Some(value) = trimmed.strip_prefix("- ") {
            covers.push(value.trim_matches('"').trim_matches('\'').to_string());
        }
    }
    covers
}

fn snapshot_files(workspace: &Path, covers: &[String]) -> Result<BTreeMap<PathBuf, String>> {
    let mut paths = BTreeSet::new();
    for pattern in covers {
        let matches = files_matching(workspace, pattern)?;
        if matches.is_empty() {
            bail!("cover pattern `{pattern}` matched no files");
        }
        paths.extend(matches);
    }

    paths
        .into_iter()
        .map(|path| {
            let absolute_path = workspace.join(&path);
            Ok((path, hash_file(&absolute_path)?))
        })
        .collect()
}

fn snapshot_path(docs_dir: &Path) -> PathBuf {
    docs_dir.join(".codocia-snapshot.json")
}

fn write_snapshot_file(docs_dir: &Path, snapshot: &Snapshot) -> Result<()> {
    let path = snapshot_path(docs_dir);
    fs::write(path, render_snapshot(snapshot)?)?;
    Ok(())
}

fn read_snapshot_file(docs_dir: &Path) -> Result<Snapshot> {
    let path = snapshot_path(docs_dir);
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_snapshot(&content)
}

fn render_snapshot(snapshot: &Snapshot) -> Result<String> {
    validate_snapshot(snapshot)?;
    let mut output = serde_json::to_string_pretty(snapshot)?;
    output.push('\n');
    Ok(output)
}

fn parse_snapshot(content: &str) -> Result<Snapshot> {
    let snapshot: Snapshot = serde_json::from_str(content).context("invalid snapshot JSON")?;
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<()> {
    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
        bail!(
            "unsupported snapshot schema_version {}; expected {}",
            snapshot.schema_version,
            SNAPSHOT_SCHEMA_VERSION
        );
    }
    if snapshot.hash_algorithm != SHA256_ALGORITHM {
        bail!(
            "unsupported snapshot hash_algorithm `{}`; expected `{}`",
            snapshot.hash_algorithm,
            SHA256_ALGORITHM
        );
    }
    for (doc, page) in &snapshot.docs {
        for (file, fingerprint) in &page.files {
            if !is_sha256_fingerprint(fingerprint) {
                bail!(
                    "snapshot `{}` has a non-canonical SHA-256 fingerprint for `{}`; regenerate the snapshot",
                    doc.display(),
                    file.display()
                );
            }
        }
    }
    Ok(())
}

fn collect_code_files(workspace: &Path, docs_dir: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut out = BTreeSet::new();
    collect_code_files_inner(workspace, workspace, docs_dir, &mut out)?;
    Ok(out)
}

fn collect_code_files_inner(
    workspace: &Path,
    dir: &Path,
    docs_dir: &Path,
    out: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if should_skip_dir(dir) || dir == docs_dir {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_code_files_inner(workspace, &path, docs_dir, out)?;
        } else {
            let relative = relative_path(workspace, &path);
            if is_code_file(&relative) {
                out.insert(relative);
            }
        }
    }
    Ok(())
}

fn files_matching(workspace: &Path, pattern: &str) -> Result<BTreeSet<PathBuf>> {
    let mut out = BTreeSet::new();
    collect_matching_files(workspace, workspace, pattern, &mut out)?;
    Ok(out)
}

fn collect_matching_files(
    workspace: &Path,
    dir: &Path,
    pattern: &str,
    out: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if should_skip_dir(dir) {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_matching_files(workspace, &path, pattern, out)?;
        } else {
            let relative = relative_path(workspace, &path);
            if glob_matches(pattern, &relative) {
                out.insert(relative);
            }
        }
    }
    Ok(())
}

fn changed_files(workspace: &Path, base: &str) -> Result<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::new();
    let committed_range = format!("{base}...HEAD");
    files.extend(git_diff_names(workspace, &[committed_range.as_str()])?);
    files.extend(git_diff_names(workspace, &["--cached"])?);
    files.extend(git_diff_names(workspace, &[])?);
    Ok(files)
}

fn git_diff_names(workspace: &Path, args: &[&str]) -> Result<BTreeSet<PathBuf>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(workspace)
        .arg("diff")
        .arg("--name-only");
    for arg in args {
        command.arg(arg);
    }
    let output = command
        .output()
        .with_context(|| format!("failed to run git diff from {}", workspace.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff failed: {stderr}");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(PathBuf::from)
        .collect())
}

fn git_diff_sections(
    workspace: &Path,
    base: Option<&str>,
    path: &Path,
) -> Result<Vec<DiffSection>> {
    let mut sections = Vec::new();
    if let Some(base) = base {
        let range = format!("{base}...HEAD");
        let diff = git_diff_for_file(workspace, &[range.as_str()], path)?;
        if !diff.trim().is_empty() {
            sections.push(DiffSection {
                label: "committed",
                command: format!("git diff {range} -- {}", path.display()),
                diff,
            });
        }
    }

    let staged = git_diff_for_file(workspace, &["--cached"], path)?;
    if !staged.trim().is_empty() {
        sections.push(DiffSection {
            label: "staged",
            command: format!("git diff --cached -- {}", path.display()),
            diff: staged,
        });
    }

    let unstaged = git_diff_for_file(workspace, &[], path)?;
    if !unstaged.trim().is_empty() {
        sections.push(DiffSection {
            label: "unstaged",
            command: format!("git diff -- {}", path.display()),
            diff: unstaged,
        });
    }

    Ok(sections)
}

fn git_diff_for_file(workspace: &Path, args: &[&str], path: &Path) -> Result<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(workspace)
        .arg("diff")
        .arg("--unified=3");
    for arg in args {
        command.arg(arg);
    }
    command.arg("--").arg(path);

    let output = command
        .output()
        .with_context(|| format!("failed to run git diff from {}", workspace.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff failed: {stderr}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn diff_excerpt(diff: &str) -> String {
    let lines = diff.lines().collect::<Vec<_>>();
    if lines.len() <= DIFF_LINE_LIMIT {
        return diff.trim_end().to_string();
    }

    let mut output = lines
        .iter()
        .take(DIFF_LINE_LIMIT)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    output.push_str(&format!(
        "\n... truncated {} line(s); run the command above for the full diff",
        lines.len() - DIFF_LINE_LIMIT
    ));
    output
}

fn push_indented_diff(message: &mut String, diff: &str, indent: &str) {
    if diff.is_empty() {
        return;
    }
    for line in diff.lines() {
        message.push('\n');
        message.push_str(indent);
        message.push_str(line);
    }
}

fn current_commit(workspace: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .with_context(|| format!("failed to run git rev-parse from {}", workspace.display()))?;
    if !output.status.success() {
        bail!("git rev-parse failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(sha256_fingerprint(&bytes))
}

fn glob_matches(pattern: &str, path: &Path) -> bool {
    let value = path.to_string_lossy().replace('\\', "/");
    glob_match_segments(
        &pattern.split('/').collect::<Vec<_>>(),
        &value.split('/').collect::<Vec<_>>(),
    )
}

fn glob_match_segments(pattern: &[&str], path: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }
    if pattern[0] == "**" {
        return glob_match_segments(&pattern[1..], path)
            || (!path.is_empty() && glob_match_segments(pattern, &path[1..]));
    }
    if path.is_empty() {
        return false;
    }
    segment_matches(pattern[0], path[0]) && glob_match_segments(&pattern[1..], &path[1..])
}

fn segment_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star_index = None;
    let mut star_value_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(index) = star_index {
            pattern_index = index + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn is_top_level_key(line: &str) -> bool {
    !line.starts_with(' ') && line.trim_end().ends_with(':')
}

const CODE_FILE_EXTENSIONS: &[&str] = &[
    "bash", "c", "cc", "cjs", "cpp", "cs", "cts", "cu", "cuh", "cxx", "fish", "go", "h", "hh",
    "hpp", "java", "js", "jsx", "kt", "kts", "lua", "metal", "mjs", "mts", "php", "py", "pyi", "r",
    "rb", "rs", "sh", "sql", "svelte", "swift", "ts", "tsx", "vue", "zsh",
];

fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            CODE_FILE_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git"
                    | ".codocia"
                    | "target"
                    | "node_modules"
                    | "__pycache__"
                    | ".venv"
                    | "dist"
                    | ".astro"
                    | ".next"
                    | ".nuxt"
                    | ".svelte-kit"
                    | "build"
                    | "coverage"
                    | "playwright-report"
                    | "test-results"
            )
        })
}

fn normalize_dir(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))
}

fn site_workspace_relative_path(workspace: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    if path.is_absolute() {
        return path
            .strip_prefix(workspace)
            .map(Path::to_path_buf)
            .with_context(|| {
                format!(
                    "{label} must stay inside workspace {}: {}",
                    workspace.display(),
                    path.display()
                )
            });
    }
    Ok(path.to_path_buf())
}

fn unique_site_sibling(output_relative: &Path, label: &str) -> PathBuf {
    let nonce = SITE_STAGING_NONCE.fetch_add(1, Ordering::Relaxed);
    let name = format!(".codocia-site-{label}-{}-{nonce}", std::process::id());
    output_relative
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(name)
}

struct SiteStagingGuard {
    path: PathBuf,
    armed: bool,
}

impl SiteStagingGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SiteStagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = remove_path_without_following(&self.path, "failed site staging output");
        }
    }
}

fn validate_existing_site_output(output: &Path) -> Result<()> {
    if !output.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(output)
        .with_context(|| format!("failed to inspect site output {}", output.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("site output is not a real directory: {}", output.display());
    }
    if fs::read_dir(output)?.next().is_none() {
        return Ok(());
    }

    let marker = output.join(SITE_OWNER_MARKER);
    if marker.exists() {
        let value: serde_json::Value = serde_json::from_slice(&read_site_control_file(&marker)?)
            .with_context(|| format!("invalid Codocia site marker {}", marker.display()))?;
        if value.get("schema_version").and_then(|value| value.as_u64()) == Some(1)
            && value.get("kind").and_then(|value| value.as_str()) == Some("codocia_starlight_site")
        {
            return Ok(());
        }
        bail!("invalid Codocia site marker {}", marker.display());
    }

    // Accept sites emitted by pre-marker Codocia releases after checking only
    // inert control files. Unknown directories are never treated as disposable.
    let package_path = output.join("package.json");
    let Ok(package_bytes) = read_site_control_file(&package_path) else {
        bail!(
            "refusing to replace unowned non-empty site output {}; choose an empty output directory or remove it after reviewing its contents",
            output.display()
        );
    };
    let package: serde_json::Value = serde_json::from_slice(&package_bytes)
        .with_context(|| format!("invalid legacy site package {}", package_path.display()))?;
    let legacy_signature = package
        .get("name")
        .and_then(|value| value.as_str())
        .is_some_and(|name| name.starts_with("codocia-docs-site"))
        && package.get("private").and_then(|value| value.as_bool()) == Some(true)
        && package
            .pointer("/scripts/build")
            .and_then(|value| value.as_str())
            == Some("astro build")
        && read_site_control_file(&output.join("astro.config.mjs")).is_ok()
        && read_site_control_file(&output.join("src/content.config.ts")).is_ok();
    if legacy_signature {
        return Ok(());
    }
    bail!(
        "refusing to replace unowned non-empty site output {}; choose an empty output directory or remove it after reviewing its contents",
        output.display()
    )
}

fn read_site_control_file(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect site control file {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "site control path is not a regular file: {}",
            path.display()
        );
    }
    if metadata.len() > 64 * 1024 {
        bail!(
            "site control file is unexpectedly large: {}",
            path.display()
        );
    }
    fs::read(path).with_context(|| format!("failed to read site control file {}", path.display()))
}

fn replace_starlight_site_output(
    workspace: &Path,
    output_relative: &Path,
    output: &Path,
    staging: &Path,
    preserve_dependency_cache: bool,
) -> Result<()> {
    validate_existing_site_output(output)?;
    let backup_relative = unique_site_sibling(output_relative, "backup");
    let backup =
        generate::secure_workspace_target(workspace, &backup_relative, "site output backup")?;
    if backup.exists() {
        bail!("site output backup already exists: {}", backup.display());
    }

    let had_output = output.exists();
    if had_output {
        fs::rename(output, &backup).with_context(|| {
            format!(
                "failed to move previous site output {} to {}",
                output.display(),
                backup.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(staging, output) {
        if had_output {
            let _ = fs::rename(&backup, output);
        }
        return Err(error).with_context(|| {
            format!(
                "failed to install fresh site output {} from {}",
                output.display(),
                staging.display()
            )
        });
    }

    if had_output {
        if preserve_dependency_cache
            && let Err(cache_error) = move_explicitly_trusted_node_modules(&backup, output)
        {
            let displaced = fs::rename(output, staging);
            let restored = if displaced.is_ok() {
                fs::rename(&backup, output)
            } else {
                Err(std::io::Error::other(
                    "could not displace the fresh site before restoring the previous site",
                ))
            };
            if displaced.is_ok() && restored.is_ok() {
                return Err(cache_error).context(
                    "failed to preserve the explicitly trusted dependency cache; restored the previous site output",
                );
            }
            bail!(
                "failed to preserve the explicitly trusted dependency cache ({cache_error:#}) and could not fully restore the previous site (displace={:?}, restore={:?})",
                displaced.err(),
                restored.err()
            );
        }
        remove_site_directory(&backup, "previous disposable site output")?;
    }
    Ok(())
}

fn move_explicitly_trusted_node_modules(previous: &Path, current: &Path) -> Result<()> {
    let source = previous.join("node_modules");
    let metadata = match fs::symlink_metadata(&source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect cached dependencies {}", source.display())
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "--skip-install refuses a cached dependency path that is not a real directory: {}",
            source.display()
        );
    }
    // Astro stores its content collection data below `node_modules/.astro`.
    // Reusing that generated state after replacing the Markdown tree makes the
    // loader see both the previous and current versions of changed pages,
    // producing duplicate IDs. Preserve installed packages, not build state.
    remove_path_without_following(&source.join(".astro"), "stale Astro content cache")?;
    fs::rename(&source, current.join("node_modules")).with_context(|| {
        format!(
            "failed to preserve explicitly trusted dependency cache {}",
            source.display()
        )
    })
}

fn remove_site_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {label} {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        bail!("refusing symbolic-link {label}: {}", path.display());
    }
    if !metadata.is_dir() {
        bail!("{label} is not a directory: {}", path.display());
    }
    fs::remove_dir_all(path).with_context(|| format!("remove {label} {}", path.display()))
}

fn secure_site_output_file(
    workspace: &Path,
    output_relative: &Path,
    relative: &Path,
) -> Result<PathBuf> {
    generate::secure_workspace_target(
        workspace,
        &output_relative.join(relative),
        "site output file",
    )
}

fn resolve_path(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn ensure_npm_available() -> Result<()> {
    let output = Command::new("npm")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context(
            "npm is required for Starlight commands; install Node.js/npm or use `codocia serve --plain`",
        )?;
    if !output.success() {
        bail!("npm is required for Starlight commands; use `codocia serve --plain` without npm");
    }
    Ok(())
}

fn npm_install_if_needed(site_dir: &Path) -> Result<()> {
    remove_path_without_following(&site_dir.join("node_modules"), "site dependency tree")?;
    remove_path_without_following(&site_dir.join("package-lock.json"), "site dependency lock")?;
    run_status_command(
        Command::new("npm")
            .arg("install")
            .arg("--ignore-scripts")
            .arg("--no-audit")
            .arg("--no-fund")
            .current_dir(site_dir),
        "clean npm install --ignore-scripts",
    )
}

fn remove_path_without_following(path: &Path, label: &str) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {label} {}", path.display()));
        }
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path).with_context(|| format!("remove {label} {}", path.display()))
    } else {
        fs::remove_file(path).with_context(|| format!("remove {label} {}", path.display()))
    }
}

fn run_status_command(command: &mut Command, label: &str) -> Result<()> {
    println!("running {label}");
    let status = command
        .status()
        .with_context(|| format!("failed to run {label}"))?;
    if !status.success() {
        bail!("{label} failed with status {status}");
    }
    Ok(())
}

fn handle_plain_request(mut stream: TcpStream, docs_dir: &Path) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
    }

    let (status, content_type, body) = plain_response_for_path(docs_dir, &path)?;
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    Ok(())
}

fn plain_response_for_path(
    docs_dir: &Path,
    request_path: &str,
) -> Result<(&'static str, &'static str, String)> {
    let clean_path = request_path.split('?').next().unwrap_or("/");
    if clean_path == "/" {
        let mut paths = Vec::new();
        collect_markdown_files(docs_dir, &mut paths)?;
        paths.retain(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        });
        paths.sort();
        return Ok(("200 OK", "text/html", render_plain_index(docs_dir, &paths)));
    }
    let Some(relative) = plain_doc_path(clean_path) else {
        return Ok(("404 Not Found", "text/plain", "not found\n".to_string()));
    };
    let Some(path) = secure_plain_doc_file(docs_dir, &relative)? else {
        return Ok(("404 Not Found", "text/plain", "not found\n".to_string()));
    };
    let content = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    if clean_path.ends_with(".md") || clean_path.starts_with("/md/") {
        return Ok(("200 OK", "text/markdown", content));
    }
    Ok((
        "200 OK",
        "text/html",
        render_plain_doc(&relative, strip_frontmatter(&content)),
    ))
}

fn plain_doc_path(request_path: &str) -> Option<PathBuf> {
    let value = request_path
        .trim_start_matches('/')
        .strip_prefix("md/")
        .unwrap_or_else(|| request_path.trim_start_matches('/'));
    if value.contains("..") || value.contains('\\') || value.starts_with('/') || value.is_empty() {
        return None;
    }
    let mut path = PathBuf::from(value);
    if path
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    match path.extension().and_then(|extension| extension.to_str()) {
        None => {
            path.set_extension("md");
        }
        Some(extension) if extension.eq_ignore_ascii_case("md") => {}
        Some(_) => return None,
    }
    Some(path)
}

fn secure_plain_doc_file(docs_dir: &Path, relative: &Path) -> Result<Option<PathBuf>> {
    let root_metadata = fs::symlink_metadata(docs_dir)
        .with_context(|| format!("failed to inspect plain docs root {}", docs_dir.display()))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        bail!(
            "plain docs root is not a real directory: {}",
            docs_dir.display()
        );
    }
    let docs_root = fs::canonicalize(docs_dir)
        .with_context(|| format!("failed to resolve plain docs root {}", docs_dir.display()))?;
    let mut current = docs_root.clone();
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Ok(None);
    }
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Ok(None);
        };
        current.push(name);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect plain doc {}", current.display()));
            }
        };
        if metadata.file_type().is_symlink() {
            bail!(
                "refusing symbolic link in plain docs: {}",
                current.display()
            );
        }
        if index + 1 < components.len() {
            if !metadata.is_dir() {
                return Ok(None);
            }
        } else if !metadata.is_file() {
            return Ok(None);
        }
    }
    let resolved = fs::canonicalize(&current)
        .with_context(|| format!("failed to resolve plain doc {}", current.display()))?;
    if !resolved.starts_with(&docs_root) {
        bail!(
            "plain doc escapes documentation root {}: {}",
            docs_root.display(),
            resolved.display()
        );
    }
    Ok(Some(resolved))
}

fn render_plain_index(docs_dir: &Path, paths: &[PathBuf]) -> String {
    let mut output = String::from(
        "<!doctype html><meta charset=\"utf-8\"><title>Docs</title><main><h1>Docs</h1><ul>",
    );
    for path in paths {
        let relative = relative_path(docs_dir, path);
        let href = html_escape(
            &relative
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/"),
        );
        output.push_str(&format!(
            "<li><a href=\"/{href}\">{}</a></li>",
            html_escape(&relative.display().to_string())
        ));
    }
    output.push_str("</ul></main>");
    output
}

fn render_plain_doc(path: &Path, markdown: &str) -> String {
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{}</title><main><p><a href=\"/\">Docs</a></p><pre>{}</pre></main>",
        html_escape(&path.display().to_string()),
        html_escape(markdown)
    )
}

fn strip_frontmatter(content: &str) -> &str {
    split_frontmatter(content)
        .map(|(_, body)| body)
        .unwrap_or(content)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn write_file(path: PathBuf, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn copy_starlight_asset(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect {}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "refusing non-regular Starlight image asset: {}",
            source.display()
        );
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "copy Starlight image asset {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn ensure_starlight_title(
    content: &str,
    path: &Path,
    known_docs: &BTreeSet<PathBuf>,
    has_root_locale: bool,
) -> String {
    let slug = site_slug(path);
    if let Some((frontmatter, body)) = split_frontmatter(content) {
        let title = title_from_frontmatter(frontmatter)
            .or_else(|| title_from_markdown(body))
            .unwrap_or_else(|| title_from_path(path));
        let body = prepare_starlight_body(body, &title, path, known_docs, has_root_locale);
        return render_starlight_markdown(&title, &slug, &parse_covers(frontmatter), &body);
    }
    let title = title_from_markdown(content).unwrap_or_else(|| title_from_path(path));
    let body = prepare_starlight_body(content, &title, path, known_docs, has_root_locale);
    render_starlight_markdown(&title, &slug, &[], &body)
}

fn prepare_starlight_body(
    body: &str,
    title: &str,
    current_path: &Path,
    known_docs: &BTreeSet<PathBuf>,
    has_root_locale: bool,
) -> String {
    let body = remove_duplicate_leading_title(body, title);
    rewrite_starlight_markdown_links(&body, current_path, known_docs, has_root_locale)
}

fn starlight_link_escaping_docs(body: &str, current_path: &Path) -> Option<String> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    Parser::new_ext(body, options).find_map(|event| {
        let Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) = event else {
            return None;
        };
        site_relative_target_escapes_docs(dest_url.as_ref(), current_path)
            .then(|| dest_url.into_string())
    })
}

fn site_relative_target_escapes_docs(target: &str, current_path: &Path) -> bool {
    let target = target.trim();
    if target.contains('\\') {
        return true;
    }
    if target.is_empty()
        || target.starts_with(['#', '?', '/'])
        || target.starts_with("//")
        || has_uri_scheme(target)
    {
        return false;
    }

    let path_end = target.find(['?', '#']).unwrap_or(target.len());
    let path = &target[..path_end];
    let mut depth = current_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .components()
        .filter(|component| matches!(component, std::path::Component::Normal(_)))
        .count();
    for raw_segment in path.split('/') {
        let Some(segment) = percent_decode_site_segment(raw_segment) else {
            return true;
        };
        if segment.contains(['/', '\\']) {
            return true;
        }
        match segment.as_str() {
            "" | "." => {}
            ".." if depth == 0 => return true,
            ".." => depth -= 1,
            _ => depth += 1,
        }
    }
    false
}

fn percent_decode_site_segment(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = site_hex_value(*bytes.get(index + 1)?)?;
        let low = site_hex_value(*bytes.get(index + 2)?)?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).ok()
}

fn site_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Starlight renders the frontmatter title as the page H1. Remove only the
/// first non-blank source line when it is a plain ATX H1 with the exact same
/// title; later headings, setext headings, and differently titled H1s remain.
fn remove_duplicate_leading_title(body: &str, title: &str) -> String {
    let mut offset = 0;
    for line_with_ending in body.split_inclusive('\n') {
        let line = line_with_ending
            .strip_suffix('\n')
            .unwrap_or(line_with_ending);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            offset += line_with_ending.len();
            continue;
        }
        if atx_h1_title(line) == Some(title) {
            let mut output = String::with_capacity(body.len() - line_with_ending.len());
            output.push_str(&body[..offset]);
            output.push_str(&body[offset + line_with_ending.len()..]);
            return output;
        }
        return body.to_string();
    }

    body.to_string()
}

fn atx_h1_title(line: &str) -> Option<&str> {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let rest = line.get(indent..)?.strip_prefix('#')?;
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }

    let mut title = rest.trim_matches([' ', '\t']);
    let without_closing_hashes = title.trim_end_matches('#');
    if without_closing_hashes.len() < title.len() && without_closing_hashes.ends_with([' ', '\t']) {
        title = without_closing_hashes.trim_end_matches([' ', '\t']);
    }
    Some(title)
}

#[derive(Debug)]
struct MarkdownSourceEdit {
    start: usize,
    end: usize,
    replacement: String,
}

/// Rewrite links by editing only the byte ranges pulldown-cmark identifies as
/// links. It therefore leaves images, inline code, and fenced code untouched.
fn rewrite_starlight_markdown_links(
    body: &str,
    current_path: &Path,
    known_docs: &BTreeSet<PathBuf>,
    has_root_locale: bool,
) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let mut edits = Vec::new();

    for (event, range) in Parser::new_ext(body, options).into_offset_iter() {
        let Event::Start(Tag::Link {
            link_type,
            dest_url,
            ..
        }) = event
        else {
            continue;
        };
        let Some(route) = starlight_route_for_markdown_link(
            dest_url.as_ref(),
            current_path,
            known_docs,
            has_root_locale,
        ) else {
            continue;
        };
        let Some(replacement) = rewrite_markdown_link_markup(
            body.get(range.clone()).unwrap_or_default(),
            link_type,
            &route,
        ) else {
            continue;
        };
        edits.push(MarkdownSourceEdit {
            start: range.start,
            end: range.end,
            replacement,
        });
    }

    edits.sort_by_key(|edit| edit.start);
    let mut output = body.to_string();
    for edit in edits.into_iter().rev() {
        output.replace_range(edit.start..edit.end, &edit.replacement);
    }
    output
}

fn starlight_route_for_markdown_link(
    target: &str,
    current_path: &Path,
    known_docs: &BTreeSet<PathBuf>,
    _has_root_locale: bool,
) -> Option<String> {
    if target.is_empty()
        || target.starts_with(['#', '?', '/', '\\'])
        || target.starts_with("//")
        || has_uri_scheme(target)
    {
        return None;
    }

    let suffix_start = target.find(['?', '#']).unwrap_or(target.len());
    let (target_path, suffix) = target.split_at(suffix_start);
    if target_path.contains('\\') {
        return None;
    }
    let extension = Path::new(target_path).extension()?.to_str()?;
    if !extension.eq_ignore_ascii_case("md") {
        return None;
    }

    let mut resolved = current_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_owned()),
            _ => None,
        })
        .collect::<PathBuf>();
    for component in target_path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if !resolved.pop() {
                    return None;
                }
            }
            component => resolved.push(component),
        }
    }
    if !known_docs.contains(&resolved) {
        return None;
    }

    if is_starlight_index(&resolved, None) {
        Some(format!("/{suffix}"))
    } else {
        Some(format!("/{}/{suffix}", site_slug(&resolved)))
    }
}

fn has_uri_scheme(target: &str) -> bool {
    let Some(colon) = target.find(':') else {
        return false;
    };
    let scheme = &target[..colon];
    !scheme.is_empty()
        && scheme.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic()
                || (index > 0 && matches!(byte, b'0'..=b'9' | b'+' | b'-' | b'.'))
        })
}

fn rewrite_markdown_link_markup(markup: &str, link_type: LinkType, route: &str) -> Option<String> {
    match link_type {
        LinkType::Inline => {
            let destination = inline_markdown_destination_span(markup)?;
            let mut rewritten = String::with_capacity(markup.len() + route.len());
            rewritten.push_str(&markup[..destination.start]);
            rewritten.push_str(route);
            rewritten.push_str(&markup[destination.end..]);
            Some(rewritten)
        }
        LinkType::Reference | LinkType::Collapsed | LinkType::Shortcut => {
            let label_end = markdown_link_label_end(markup)?;
            Some(format!("{}({route})", &markup[..label_end]))
        }
        LinkType::ReferenceUnknown
        | LinkType::CollapsedUnknown
        | LinkType::ShortcutUnknown
        | LinkType::Autolink
        | LinkType::Email
        | LinkType::WikiLink { .. } => None,
    }
}

fn markdown_link_label_end(markup: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (index, character) in markup.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index + character.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn inline_markdown_destination_span(markup: &str) -> Option<std::ops::Range<usize>> {
    let label_end = markdown_link_label_end(markup)?;
    let bytes = markup.as_bytes();
    if bytes.get(label_end) != Some(&b'(') {
        return None;
    }
    let mut index = label_end + 1;
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        index += 1;
    }
    if bytes.get(index) == Some(&b'<') {
        let start = index + 1;
        let mut escaped = false;
        for (relative, character) in markup[start..].char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match character {
                '\\' => escaped = true,
                '>' => return Some(start..start + relative),
                _ => {}
            }
        }
        return None;
    }

    let start = index;
    let mut nested_parentheses = 0usize;
    let mut escaped = false;
    for (relative, character) in markup[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '(' => nested_parentheses += 1,
            ')' if nested_parentheses == 0 => return Some(start..start + relative),
            ')' => nested_parentheses -= 1,
            character if character.is_whitespace() && nested_parentheses == 0 => {
                return Some(start..start + relative);
            }
            _ => {}
        }
    }
    None
}

fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + "\n---\n".len()..]))
}

fn title_from_frontmatter(frontmatter: &str) -> Option<String> {
    frontmatter.lines().find_map(|line| {
        let value = line.trim_start().strip_prefix("title:")?.trim();
        (!value.is_empty()).then(|| clean_frontmatter_scalar(value))
    })
}

fn clean_frontmatter_scalar(value: &str) -> String {
    value.trim_matches('"').trim_matches('\'').to_string()
}

fn render_starlight_markdown(title: &str, slug: &str, covers: &[String], body: &str) -> String {
    let mut output = format!(
        "---\ntitle: {}\nslug: {}\n",
        yaml_string(title),
        yaml_string(slug)
    );
    if !covers.is_empty() {
        output.push_str("covers:\n");
        for cover in covers {
            output.push_str(&format!("  - {}\n", yaml_string(cover)));
        }
    }
    output.push_str("---\n");
    if !body.starts_with('\n') {
        output.push('\n');
    }
    output.push_str(body);
    output
}

fn title_from_markdown(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let title = line.trim().strip_prefix("# ")?.trim();
        (!title.is_empty()).then(|| title.to_string())
    })
}

fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Documentation")
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn yaml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn render_site_package_json(title: &str) -> String {
    format!(
        r#"{{
  "name": "{}",
  "private": true,
  "type": "module",
  "scripts": {{
    "dev": "astro dev",
    "build": "astro build",
    "preview": "astro preview"
  }},
  "dependencies": {{
    "@astrojs/starlight": "0.35.3",
    "astro": "5.18.2",
    "typescript": "5.9.3"
  }}
}}
"#,
        package_name_from_title(title)
    )
}

fn package_name_from_title(title: &str) -> String {
    let mut name = String::from("codocia-docs-site");
    let suffix = title
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if !suffix.is_empty() {
        name.push('-');
        name.push_str(&suffix);
    }
    name
}

fn is_starlight_index(path: &Path, locale: Option<&str>) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if file_name != "index.md" {
        return false;
    }

    match locale {
        None => path
            .parent()
            .is_none_or(|parent| parent.as_os_str().is_empty()),
        Some(locale) => path.parent() == Some(Path::new(locale)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StarlightLocale {
    key: String,
    label: String,
    language: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StarlightLocales {
    root: StarlightLocale,
    localized: Vec<StarlightLocale>,
}

fn validate_starlight_slugs(known_docs: &BTreeSet<PathBuf>) -> Result<()> {
    let mut owners = BTreeMap::<String, &PathBuf>::new();
    for path in known_docs {
        let slug = site_slug(path);
        if let Some(previous) = owners.insert(slug.clone(), path) {
            bail!(
                "Starlight route collision: `{}` and `{}` both normalize to site slug `{}`; rename one source page before generating the site",
                previous.display(),
                path.display(),
                slug
            );
        }
    }
    Ok(())
}

/// Infer Starlight i18n only from explicit page metadata. The root index owns
/// the primary language. A first-level directory becomes a locale only when
/// its own index declares a language whose normalized tag exactly matches the
/// directory name. This keeps ordinary sections such as `runtime/index.md`
/// from being mistaken for locales merely because they contain an index page.
fn detect_starlight_locales(
    docs_dir: &Path,
    known_docs: &BTreeSet<PathBuf>,
) -> Result<Option<StarlightLocales>> {
    let Some(root_index) = known_docs
        .iter()
        .find(|path| is_starlight_index(path, None))
    else {
        return Ok(None);
    };
    let root_content = fs::read_to_string(docs_dir.join(root_index))
        .with_context(|| format!("failed to read {}", docs_dir.join(root_index).display()))?;
    let Some(root_language) = language_from_frontmatter(&root_content) else {
        return Ok(None);
    };
    let Ok(root_locale) = generation_model::normalized_locale(&root_language) else {
        return Ok(None);
    };

    let mut localized = BTreeMap::<String, StarlightLocale>::new();
    for index_path in known_docs
        .iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("index.md"))
    {
        let Some(parent) = index_path.parent() else {
            continue;
        };
        let mut components = parent.components();
        let Some(std::path::Component::Normal(directory)) = components.next() else {
            continue;
        };
        if components.next().is_some() {
            continue;
        }
        let Some(directory) = directory.to_str() else {
            continue;
        };

        let content = fs::read_to_string(docs_dir.join(index_path))
            .with_context(|| format!("failed to read {}", docs_dir.join(index_path).display()))?;
        let Some(language) = language_from_frontmatter(&content) else {
            continue;
        };
        let Ok(locale) = generation_model::normalized_locale(&language) else {
            continue;
        };
        if locale != directory || locale == root_locale || locale == "root" {
            continue;
        }

        localized
            .entry(locale.clone())
            .or_insert_with(|| StarlightLocale {
                key: locale,
                label: starlight_locale_label(&language),
                language: canonical_language_tag(&language),
            });
    }

    if localized.is_empty() {
        return Ok(None);
    }

    Ok(Some(StarlightLocales {
        root: StarlightLocale {
            key: "root".to_string(),
            label: starlight_locale_label(&root_language),
            language: canonical_language_tag(&root_language),
        },
        localized: localized.into_values().collect(),
    }))
}

fn language_from_frontmatter(content: &str) -> Option<String> {
    let (frontmatter, _) = split_frontmatter(content)?;
    frontmatter.lines().find_map(|line| {
        let value = line.trim_start().strip_prefix("language:")?.trim();
        if value.is_empty() {
            return None;
        }
        let language = clean_frontmatter_scalar(value);
        (!language.is_empty()).then_some(language)
    })
}

fn canonical_language_tag(language: &str) -> String {
    let locale = generation_model::normalized_locale(language)
        .expect("language tags used for Starlight locales were already validated");
    locale
        .split('-')
        .enumerate()
        .map(|(index, subtag)| {
            if index == 0 {
                subtag.to_string()
            } else if subtag.len() == 4 && subtag.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                let mut characters = subtag.chars();
                let first = characters
                    .next()
                    .expect("a four-byte ASCII subtag is non-empty")
                    .to_ascii_uppercase();
                format!("{first}{}", characters.as_str())
            } else if subtag.len() == 2 && subtag.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                subtag.to_ascii_uppercase()
            } else {
                subtag.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn starlight_locale_label(language: &str) -> String {
    let locale = generation_model::normalized_locale(language)
        .expect("language tags used for Starlight locales were already validated");
    let subtags = locale.split('-').collect::<BTreeSet<_>>();
    let primary = locale.split('-').next().unwrap_or(locale.as_str());
    match primary {
        "zh" if subtags.contains("hant")
            || subtags.contains("tw")
            || subtags.contains("hk")
            || subtags.contains("mo") =>
        {
            "繁體中文".to_string()
        }
        "zh" if subtags.contains("hans") || subtags.contains("cn") || subtags.contains("sg") => {
            "简体中文".to_string()
        }
        "zh" => "中文".to_string(),
        "en" => "English".to_string(),
        "ar" => "العربية".to_string(),
        "de" => "Deutsch".to_string(),
        "es" => "Español".to_string(),
        "fr" => "Français".to_string(),
        "hi" => "हिन्दी".to_string(),
        "it" => "Italiano".to_string(),
        "ja" => "日本語".to_string(),
        "ko" => "한국어".to_string(),
        "pt" => "Português".to_string(),
        "ru" => "Русский".to_string(),
        _ => canonical_language_tag(language),
    }
}

fn render_astro_config(
    title: &str,
    docs: &[PathBuf],
    locales: Option<&StarlightLocales>,
) -> String {
    if let Some(locales) = locales {
        let mut locale_entries = vec![format!(
            "        root: {{ label: {}, lang: {} }},",
            js_string(&locales.root.label),
            js_string(&locales.root.language)
        )];
        locale_entries.extend(locales.localized.iter().map(|locale| {
            format!(
                "        {}: {{ label: {}, lang: {} }},",
                js_string(&locale.key),
                js_string(&locale.label),
                js_string(&locale.language)
            )
        }));
        return format!(
            r#"import {{ defineConfig }} from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({{
  integrations: [
    starlight({{
      title: {},
      defaultLocale: 'root',
      locales: {{
{}
      }},
    }}),
  ],
}});
"#,
            js_string(title),
            locale_entries.join("\n")
        );
    }

    let mut sidebar_items = vec!["'index'".to_string()];
    sidebar_items.extend(
        docs.iter()
            .map(|path| site_slug(path))
            .filter(|slug| slug != "index")
            .map(|slug| js_string(&slug)),
    );
    format!(
        r#"import {{ defineConfig }} from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({{
  integrations: [
    starlight({{
      title: {},
      sidebar: [
        {{ label: 'Documentation', items: [{}] }},
      ],
    }}),
  ],
}});
"#,
        js_string(title),
        sidebar_items.join(", ")
    )
}

fn render_synthetic_site_index(title: &str, docs: &[PathBuf]) -> String {
    let mut output = format!(
        "---\ntitle: {}\ndescription: Generated navigation for repository Markdown documentation.\n---\n\nRepository Markdown remains the source of truth. Choose a page below.\n\n",
        yaml_string(title)
    );
    for doc in docs {
        output.push_str(&format!("- [{}](./{}/)\n", doc.display(), site_slug(doc)));
    }
    output
}

fn site_slug(path: &Path) -> String {
    let mut components = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(last) = components.last_mut()
        && let Some((stem, _)) = last.rsplit_once('.')
    {
        *last = stem.to_string();
    }
    if components.last().is_some_and(|value| value == "index") && components.len() > 1 {
        components.pop();
    }
    let slug = components
        .into_iter()
        .map(|component| slug_component(&component))
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if slug.is_empty() {
        "index".to_string()
    } else {
        slug
    }
}

fn slug_component(value: &str) -> String {
    let mut output = String::new();
    let mut separated = false;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if separated && !output.is_empty() {
                output.push('-');
            }
            output.extend(character.to_lowercase());
            separated = false;
        } else {
            separated = true;
        }
    }
    output
}

fn js_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

const SITE_TSCONFIG: &str = r#"{
  "extends": "astro/tsconfigs/strict"
}
"#;

const SITE_FAVICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="12" fill="#6366f1"/>
  <path fill="#fff" d="M17 16h30v7H24v7h19v7H24v7h23v7H17z"/>
</svg>
"##;

const SITE_CONTENT_CONFIG: &str = r#"import { defineCollection, z } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';

export const collections = {
  docs: defineCollection({
    loader: docsLoader(),
    schema: docsSchema({
      extend: z.object({
        covers: z.array(z.string()).optional(),
      }),
    }),
  }),
};
"#;

fn render_site_readme() -> &'static str {
    r#"# Codocia Starlight Docs Site

This site is generated from repository Markdown docs.

```bash
npm install --ignore-scripts --no-audit --no-fund
npm run dev
```

Source docs remain the source of truth. Regenerate the site with `codocia site`
after changing the source docs. Codocia's `site build` and `site serve` commands
normally perform a clean exact top-level-version install; `--skip-install`
explicitly trusts only an existing real `node_modules` tree. Every other site
file is regenerated from scratch.
"#
}

fn render_llms_index(title: &str, docs: &[PathBuf]) -> String {
    let mut output = format!("# {title}\n\n");
    output.push_str("Markdown docs generated by Codocia for AI readers.\n\n");
    for doc in docs {
        let href = format!("/md/{}", doc.to_string_lossy().replace('\\', "/"));
        output.push_str(&format!("- [{}]({})\n", doc.display(), href));
    }
    output
}

fn render_llms_full(title: &str, docs_dir: &Path, docs: &[PathBuf]) -> String {
    let mut output = format!("# {title}\n\n");
    for doc in docs {
        output.push_str(&format!("## {}\n\n", doc.display()));
        match fs::read_to_string(docs_dir.join(doc)) {
            Ok(content) => {
                output.push_str(content.trim());
                output.push_str("\n\n");
            }
            Err(error) => {
                output.push_str(&format!("Unable to read doc: {error}\n\n"));
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_covers_from_frontmatter() {
        let content = "---\ntitle: Runtime\ncovers:\n  - crates/runtime/**\n---\n\n# Runtime\n";
        let frontmatter = parse_frontmatter(content).unwrap();

        assert_eq!(frontmatter.covers, vec!["crates/runtime/**"]);
    }

    #[test]
    fn snapshot_json_round_trips() {
        let mut docs = BTreeMap::new();
        docs.insert(
            PathBuf::from("docs/runtime.md"),
            DocSnapshot {
                covers: vec!["crates/runtime/**".to_string()],
                files: BTreeMap::from([(
                    PathBuf::from("crates/runtime/src/lib.rs"),
                    sha256_fingerprint(b"source"),
                )]),
            },
        );
        let snapshot = Snapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            hash_algorithm: SHA256_ALGORITHM.to_string(),
            commit: "abc123".to_string(),
            docs,
        };

        let rendered = render_snapshot(&snapshot).unwrap();
        let parsed = parse_snapshot(&rendered).unwrap();

        assert_eq!(parsed, snapshot);
    }

    #[test]
    fn snapshot_rejects_legacy_and_unknown_integrity_contracts() {
        let legacy = r#"{"commit":"abc","docs":{}}"#;
        assert!(parse_snapshot(legacy).is_err());

        let unknown = r#"{
          "schema_version": 1,
          "hash_algorithm": "sha512",
          "commit": "abc",
          "docs": {}
        }"#;
        assert!(
            parse_snapshot(unknown)
                .unwrap_err()
                .to_string()
                .contains("unsupported snapshot hash_algorithm")
        );
    }

    #[test]
    fn glob_supports_recursive_matches() {
        assert!(glob_matches(
            "crates/runtime/**",
            Path::new("crates/runtime/src/lib.rs")
        ));
        assert!(glob_matches(
            "crates/**/src/*.rs",
            Path::new("crates/runtime/src/lib.rs")
        ));
        assert!(!glob_matches(
            "crates/skill/**",
            Path::new("crates/runtime/src/lib.rs")
        ));
    }

    #[test]
    fn recognizes_common_source_file_extensions() {
        for path in [
            "src/lib.rs",
            "python/skrun/runtime.py",
            "apps/web/src/App.tsx",
            "packages/core/src/index.ts",
            "site/vite.config.js",
            "components/Button.vue",
            "scripts/dev.sh",
        ] {
            assert!(is_code_file(Path::new(path)), "{path} should be code");
        }

        for path in ["README.md", "package.json", "docs/index.md"] {
            assert!(!is_code_file(Path::new(path)), "{path} should not be code");
        }
    }

    #[test]
    fn skips_common_generated_directories() {
        for path in [
            Path::new(".astro"),
            Path::new("website/.astro"),
            Path::new("coverage"),
            Path::new("playwright-report"),
            Path::new("test-results"),
        ] {
            assert!(
                should_skip_dir(path),
                "{} should be skipped",
                path.display()
            );
        }
    }

    #[test]
    fn init_creates_policy_file_without_overwriting_existing_content() {
        let root = temp_dir("init-policy");

        init(&root).unwrap();

        assert!(!root.join("codocia.toml").exists());
        let policy_path = root.join("codocia.md");
        let policy = fs::read_to_string(&policy_path).unwrap();
        assert!(policy.contains("# Codocia Documentation Policy"));
        assert!(policy.contains("## Metrics"));
        assert!(policy.contains("agent usability"));

        fs::write(&policy_path, "# Custom Policy\n").unwrap();
        init(&root).unwrap();

        assert_eq!(
            fs::read_to_string(&policy_path).unwrap(),
            "# Custom Policy\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn site_generation_copies_docs_into_starlight_project() {
        let root = temp_dir("site");
        fs::create_dir_all(root.join("docs/guides")).unwrap();
        fs::write(
            root.join("docs/index.md"),
            "---\ntitle: Home\ncovers:\n  - src/**\n---\n\n# Home\n",
        )
        .unwrap();
        fs::write(root.join("docs/guides/usage.md"), "# Usage\n").unwrap();

        generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        })
        .unwrap();

        let output = root.join(".codocia/starlight");
        assert!(output.join("package.json").is_file());
        assert!(output.join("astro.config.mjs").is_file());
        assert!(output.join("src/content.config.ts").is_file());
        assert!(output.join("src/content/docs/index.md").is_file());
        assert!(output.join("src/content/docs/guides/usage.md").is_file());
        assert!(output.join("public/md/guides/usage.md").is_file());
        assert!(output.join("public/favicon.svg").is_file());
        assert!(output.join("public/llms.txt").is_file());
        assert!(output.join("public/llms-full.txt").is_file());

        let generated =
            fs::read_to_string(output.join("src/content/docs/guides/usage.md")).unwrap();
        assert!(generated.contains("title: \"Usage\""));
        assert!(!generated.lines().any(|line| line.trim() == "# Usage"));
        assert_eq!(
            fs::read_to_string(output.join("public/md/guides/usage.md")).unwrap(),
            "# Usage\n"
        );

        let config = fs::read_to_string(output.join("src/content.config.ts")).unwrap();
        assert!(config.contains("docsLoader()"));
        assert!(config.contains("covers: z.array(z.string()).optional()"));
        let astro_config = fs::read_to_string(output.join("astro.config.mjs")).unwrap();
        assert!(astro_config.contains("items: ['index', 'guides/usage']"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn site_generation_rejects_normalized_slug_collisions_before_copying_pages() {
        for (case, first, second) in [
            ("separator-folding", "foo_bar.md", "foo-bar.md"),
            ("file-index-folding", "foo.md", "foo/index.md"),
            ("locale-namespace", "en.md", "en/index.md"),
        ] {
            let root = temp_dir(&format!("site-slug-collision-{case}"));
            fs::create_dir_all(root.join("docs")).unwrap();
            for path in [first, second] {
                let destination = root.join("docs").join(path);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                let content = if path == "en/index.md" {
                    "---\ntitle: English\nlanguage: en\n---\n\n# English\n"
                } else {
                    "# Page\n"
                };
                fs::write(destination, content).unwrap();
            }
            if case == "locale-namespace" {
                fs::write(
                    root.join("docs/index.md"),
                    "---\ntitle: 中文\nlanguage: zh-CN\n---\n\n# 中文\n",
                )
                .unwrap();
            }

            let error = generate_starlight_site(&SiteConfig {
                workspace: root.clone(),
                docs: PathBuf::from("docs"),
                output: PathBuf::from(".codocia/starlight"),
                title: "Collision".to_string(),
            })
            .unwrap_err();
            let message = error.to_string();
            assert!(message.contains("Starlight route collision"));
            assert!(message.contains(first));
            assert!(message.contains(second));

            assert!(!root.join(".codocia/starlight").exists());
            assert!(fs::read_dir(root.join(".codocia")).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".codocia-site-staging-")
            }));
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn bilingual_starlight_generation_keeps_locales_routes_and_raw_outputs_aligned() {
        let root = temp_dir("site-bilingual");
        for directory in [
            "docs/runtime",
            "docs/en/runtime",
            "docs/assets",
            "docs/en/assets",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }

        let chinese_index = "---\ntitle: MacLLM 文档\nlanguage: zh-CN\n---\n\n# MacLLM 文档\n\n[运行时](runtime/architecture.md)\n[English](en/index.md)\n";
        let chinese_runtime =
            "# 运行时架构\n\n[返回首页](../index.md)\n![架构图](../assets/runtime.png)\n";
        let english_index = "---\ntitle: MacLLM Documentation\nlanguage: en\n---\n\n# MacLLM Documentation\n\n[Runtime](runtime/architecture.md)\n[简体中文](../index.md)\n";
        let english_runtime =
            "# Runtime Architecture\n\n[Home](../index.md)\n![Diagram](../assets/runtime.png)\n";

        fs::write(root.join("docs/index.md"), chinese_index).unwrap();
        fs::write(root.join("docs/runtime/architecture.md"), chinese_runtime).unwrap();
        fs::write(root.join("docs/en/index.md"), english_index).unwrap();
        fs::write(
            root.join("docs/en/runtime/architecture.md"),
            english_runtime,
        )
        .unwrap();
        fs::write(root.join("docs/assets/runtime.png"), [1, 2, 3]).unwrap();
        fs::write(root.join("docs/en/assets/runtime.png"), [4, 5, 6]).unwrap();

        generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "MacLLM".to_string(),
        })
        .unwrap();

        let output = root.join(".codocia/starlight");
        let astro_config = fs::read_to_string(output.join("astro.config.mjs")).unwrap();
        assert!(astro_config.contains("defaultLocale: 'root'"));
        assert!(astro_config.contains("root: { label: '简体中文', lang: 'zh-CN' }"));
        assert!(astro_config.contains("'en': { label: 'English', lang: 'en' }"));
        assert!(
            !astro_config.contains("sidebar:"),
            "Starlight's locale-aware filesystem sidebar must own bilingual navigation"
        );

        let site_chinese_index =
            fs::read_to_string(output.join("src/content/docs/index.md")).unwrap();
        assert!(site_chinese_index.contains("slug: \"index\""));
        assert!(site_chinese_index.contains("[运行时](/runtime/architecture/)"));
        assert!(site_chinese_index.contains("[English](/en/)"));
        assert!(
            !site_chinese_index
                .lines()
                .any(|line| line.trim() == "# MacLLM 文档")
        );

        let site_english_index =
            fs::read_to_string(output.join("src/content/docs/en/index.md")).unwrap();
        assert!(site_english_index.contains("slug: \"en\""));
        assert!(site_english_index.contains("[Runtime](/en/runtime/architecture/)"));
        assert!(site_english_index.contains("[简体中文](/)"));

        let site_english_runtime =
            fs::read_to_string(output.join("src/content/docs/en/runtime/architecture.md")).unwrap();
        assert!(site_english_runtime.contains("slug: \"en/runtime/architecture\""));
        assert!(site_english_runtime.contains("[Home](/en/)"));

        assert_eq!(
            fs::read_to_string(output.join("public/md/index.md")).unwrap(),
            chinese_index
        );
        assert_eq!(
            fs::read_to_string(output.join("public/md/en/index.md")).unwrap(),
            english_index
        );
        assert_eq!(
            fs::read(output.join("src/content/docs/assets/runtime.png")).unwrap(),
            [1, 2, 3]
        );
        assert_eq!(
            fs::read(output.join("public/md/en/assets/runtime.png")).unwrap(),
            [4, 5, 6]
        );

        let llms_index = fs::read_to_string(output.join("public/llms.txt")).unwrap();
        assert!(llms_index.contains("/md/index.md"));
        assert!(llms_index.contains("/md/en/index.md"));
        let llms_full = fs::read_to_string(output.join("public/llms-full.txt")).unwrap();
        assert!(llms_full.contains("[运行时](runtime/architecture.md)"));
        assert!(llms_full.contains("[Runtime](runtime/architecture.md)"));
        assert!(!llms_full.contains("[Runtime](/en/runtime/architecture/)"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bilingual_starlight_generation_supports_english_root_and_chinese_locale() {
        let root = temp_dir("site-bilingual-reversed");
        for directory in ["docs/runtime", "docs/zh-cn/runtime"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }

        fs::write(
            root.join("docs/index.md"),
            "---\ntitle: Runtime Docs\nlanguage: en\n---\n\n# Runtime Docs\n\n[Runtime](runtime/architecture.md)\n[简体中文](zh-cn/index.md)\n",
        )
        .unwrap();
        fs::write(
            root.join("docs/runtime/architecture.md"),
            "# Runtime\n\n[Home](../index.md)\n",
        )
        .unwrap();
        fs::write(
            root.join("docs/zh-cn/index.md"),
            "---\ntitle: 运行时文档\nlanguage: zh-CN\n---\n\n# 运行时文档\n\n[架构](runtime/architecture.md)\n[English](../index.md)\n",
        )
        .unwrap();
        fs::write(
            root.join("docs/zh-cn/runtime/architecture.md"),
            "# 运行时\n\n[首页](../index.md)\n",
        )
        .unwrap();

        generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Runtime".to_string(),
        })
        .unwrap();

        let output = root.join(".codocia/starlight");
        let astro_config = fs::read_to_string(output.join("astro.config.mjs")).unwrap();
        assert!(astro_config.contains("defaultLocale: 'root'"));
        assert!(astro_config.contains("root: { label: 'English', lang: 'en' }"));
        assert!(astro_config.contains("'zh-cn': { label: '简体中文', lang: 'zh-CN' }"));
        assert!(!astro_config.contains("sidebar:"));

        let root_index = fs::read_to_string(output.join("src/content/docs/index.md")).unwrap();
        assert!(root_index.contains("[Runtime](/runtime/architecture/)"));
        assert!(root_index.contains("[简体中文](/zh-cn/)"));
        let chinese_index =
            fs::read_to_string(output.join("src/content/docs/zh-cn/index.md")).unwrap();
        assert!(chinese_index.contains("[架构](/zh-cn/runtime/architecture/)"));
        assert!(chinese_index.contains("[English](/)"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn locale_detection_requires_matching_language_metadata_and_directory() {
        let root = temp_dir("site-locale-directory-match");
        fs::create_dir_all(root.join("docs/guides")).unwrap();
        fs::create_dir_all(root.join("docs/zh-cn")).unwrap();
        fs::write(
            root.join("docs/index.md"),
            "---\ntitle: Home\nlanguage: en\n---\n\n# Home\n",
        )
        .unwrap();
        fs::write(
            root.join("docs/guides/index.md"),
            "---\ntitle: Guides\nlanguage: zh-CN\n---\n\n# Guides\n",
        )
        .unwrap();
        fs::write(
            root.join("docs/zh-cn/index.md"),
            "---\ntitle: Not Chinese\nlanguage: en\n---\n\n# Not Chinese\n",
        )
        .unwrap();

        generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        })
        .unwrap();

        let astro_config =
            fs::read_to_string(root.join(".codocia/starlight/astro.config.mjs")).unwrap();
        assert!(!astro_config.contains("locales:"));
        assert!(astro_config.contains("sidebar:"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_root_and_english_indexes_without_language_stay_monolingual() {
        let root = temp_dir("site-legacy-bilingual-shape");
        fs::create_dir_all(root.join("docs/en")).unwrap();
        fs::write(root.join("docs/index.md"), "# Home\n").unwrap();
        fs::write(root.join("docs/en/index.md"), "# English\n").unwrap();

        generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        })
        .unwrap();

        let astro_config =
            fs::read_to_string(root.join(".codocia/starlight/astro.config.mjs")).unwrap();
        assert!(!astro_config.contains("locales:"));
        assert!(astro_config.contains("sidebar:"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn english_directory_without_root_index_keeps_monolingual_site_behavior() {
        let root = temp_dir("site-en-without-root");
        fs::create_dir_all(root.join("docs/en")).unwrap();
        fs::write(root.join("docs/en/index.md"), "# English\n").unwrap();

        generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        })
        .unwrap();

        let output = root.join(".codocia/starlight");
        let astro_config = fs::read_to_string(output.join("astro.config.mjs")).unwrap();
        assert!(!astro_config.contains("locales:"));
        assert!(astro_config.contains("items: ['index', 'en']"));
        assert!(output.join("src/content/docs/index.md").is_file());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn site_generation_synthesizes_a_navigation_home_when_index_is_missing() {
        let root = temp_dir("site-synthetic-index");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/architecture.md"), "# Architecture\n").unwrap();

        generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        })
        .unwrap();

        let index =
            fs::read_to_string(root.join(".codocia/starlight/src/content/docs/index.md")).unwrap();
        assert!(index.contains("title: \"Example Docs\""));
        assert!(index.contains("[architecture.md](./architecture/)"));
        assert!(!root.join("docs/index.md").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn site_generation_preserves_existing_frontmatter_title() {
        let content = "---\ntitle: Existing\ncovers:\n  - src/**\n---\n\n# Other\n";
        let generated =
            ensure_starlight_title(content, Path::new("index.md"), &BTreeSet::new(), false);

        assert!(generated.contains("title: \"Existing\""));
        assert!(generated.contains("slug: \"index\""));
        assert!(generated.contains("  - \"src/**\""));
        assert!(generated.contains("# Other"));
    }

    #[test]
    fn site_generation_sanitizes_invalid_frontmatter_lines() {
        let content = "---\ntitle: Hooks\ncovers:\n  - src/**/*.rs\n.rs\n---\n\n# Hooks\n";
        let generated =
            ensure_starlight_title(content, Path::new("hooks.md"), &BTreeSet::new(), false);

        assert!(generated.contains("title: \"Hooks\""));
        assert!(generated.contains("  - \"src/**/*.rs\""));
        assert!(!generated.contains("\n.rs\n"));
        assert!(!generated.lines().any(|line| line.trim() == "# Hooks"));
    }

    #[test]
    fn starlight_copy_rewrites_only_internal_markdown_links_and_keeps_raw_outputs() {
        let root = temp_dir("site-link-rewrite");
        fs::create_dir_all(root.join("docs/guides")).unwrap();
        fs::create_dir_all(root.join("docs/models")).unwrap();
        fs::create_dir_all(root.join("docs/assets")).unwrap();
        fs::write(
            root.join("docs/index.md"),
            "---\ntitle: Home\n---\n\n# Home\n\nWelcome.\n",
        )
        .unwrap();
        let setup = r#"---
title: Setup
---

# Setup

[Model](../models/index.md?mode=full#loading)
[Angle](<../models/index.md#angle> "kept title")
[Peer](./peer.md)
[Home](../index.md#top)
[Reference][model-ref]
![Asset](../assets/diagram.png)
![Diagram](../models/index.md#diagram)
![Reference image][model-ref]
[External](https://example.com/file.md)
[Mail](mailto:docs@example.com)
[Fragment](#local)
`[Inline code](../models/index.md)`

```md
[Fenced code](../models/index.md)
```

[model-ref]: ../models/index.md#reference
"#;
        fs::write(root.join("docs/guides/setup.md"), setup).unwrap();
        fs::write(
            root.join("docs/guides/peer.md"),
            "---\ntitle: Peer\n---\n\n# Different heading\n",
        )
        .unwrap();
        fs::write(
            root.join("docs/models/index.md"),
            "---\ntitle: Models\n---\n\n# Models\n",
        )
        .unwrap();
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a];
        fs::write(root.join("docs/assets/diagram.png"), png).unwrap();
        fs::write(root.join("docs/assets/ignored.bmp"), "not copied\n").unwrap();

        generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        })
        .unwrap();

        let output = root.join(".codocia/starlight");
        let site_setup =
            fs::read_to_string(output.join("src/content/docs/guides/setup.md")).unwrap();
        assert!(!site_setup.lines().any(|line| line.trim() == "# Setup"));
        assert!(site_setup.contains("[Model](/models/?mode=full#loading)"));
        assert!(site_setup.contains("[Angle](</models/#angle> \"kept title\")"));
        assert!(site_setup.contains("[Peer](/guides/peer/)"));
        assert!(site_setup.contains("[Home](/#top)"));
        assert!(site_setup.contains("[Reference](/models/#reference)"));
        assert!(site_setup.contains("![Asset](../assets/diagram.png)"));
        assert!(site_setup.contains("![Diagram](../models/index.md#diagram)"));
        assert!(site_setup.contains("![Reference image][model-ref]"));
        assert!(site_setup.contains("[External](https://example.com/file.md)"));
        assert!(site_setup.contains("[Mail](mailto:docs@example.com)"));
        assert!(site_setup.contains("[Fragment](#local)"));
        assert!(site_setup.contains("`[Inline code](../models/index.md)`"));
        assert!(site_setup.contains("[Fenced code](../models/index.md)"));
        assert!(site_setup.contains("[model-ref]: ../models/index.md#reference"));

        assert_eq!(
            fs::read_to_string(output.join("public/md/guides/setup.md")).unwrap(),
            setup
        );
        assert_eq!(
            fs::read_to_string(root.join("docs/guides/setup.md")).unwrap(),
            setup
        );
        let llms_full = fs::read_to_string(output.join("public/llms-full.txt")).unwrap();
        assert!(llms_full.contains("[Model](../models/index.md?mode=full#loading)"));
        assert!(!llms_full.contains("[Model](/models/?mode=full#loading)"));
        let llms_index = fs::read_to_string(output.join("public/llms.txt")).unwrap();
        assert!(!llms_index.contains("assets/diagram.png"));
        let astro_config = fs::read_to_string(output.join("astro.config.mjs")).unwrap();
        assert!(!astro_config.contains("assets/diagram"));

        let asset = "assets/diagram.png";
        let expected = fs::read(root.join("docs").join(asset)).unwrap();
        assert_eq!(
            fs::read(output.join("src/content/docs").join(asset)).unwrap(),
            expected
        );
        assert_eq!(
            fs::read(output.join("public/md").join(asset)).unwrap(),
            expected
        );
        assert!(!output.join("src/content/docs/assets/ignored.bmp").exists());
        assert!(!output.join("public/md/assets/ignored.bmp").exists());

        let peer = fs::read_to_string(output.join("src/content/docs/guides/peer.md")).unwrap();
        assert!(
            peer.lines()
                .any(|line| line.trim() == "# Different heading")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn starlight_site_generation_rejects_symbolic_links_in_docs() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("site-symlink");
        fs::create_dir_all(root.join("docs/assets")).unwrap();
        fs::write(root.join("docs/index.md"), "# Home\n").unwrap();
        fs::write(root.join("outside.png"), "outside\n").unwrap();
        symlink(
            root.join("outside.png"),
            root.join("docs/assets/linked.png"),
        )
        .unwrap();

        let error = generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("refusing symbolic link in Starlight documentation tree"));
        assert!(
            !root
                .join(".codocia/starlight/src/content/docs/index.md")
                .exists()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn starlight_site_generation_rejects_mdx_before_copying_it() {
        let root = temp_dir("site-mdx");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/index.md"), "# Home\n").unwrap();
        fs::write(root.join("docs/component.mdx"), "# Executable\n").unwrap();

        let error = generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("refusing executable MDX"));
        assert!(
            !root
                .join(".codocia/starlight/src/content/docs/component.mdx")
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn starlight_site_generation_rejects_unsafe_markdown_and_active_svg() {
        for (label, markdown, asset, expected) in [
            (
                "raw-html",
                "# Home\n\n<script>alert(1)</script>\n",
                None,
                "raw HTML is not allowed",
            ),
            (
                "unsafe-url",
                "# Home\n\n[run](javascript:payload)\n",
                None,
                "unsafe URL",
            ),
            (
                "active-svg",
                "# Home\n",
                Some(("diagram.svg", "<svg><script>alert(1)</script></svg>\n")),
                "refusing active SVG asset",
            ),
        ] {
            let root = temp_dir(label);
            fs::create_dir_all(root.join("docs")).unwrap();
            fs::write(root.join("docs/index.md"), markdown).unwrap();
            if let Some((path, content)) = asset {
                fs::write(root.join("docs").join(path), content).unwrap();
            }

            let error = generate_starlight_site(&SiteConfig {
                workspace: root.clone(),
                docs: PathBuf::from("docs"),
                output: PathBuf::from(".codocia/starlight"),
                title: "Example Docs".to_string(),
            })
            .unwrap_err()
            .to_string();
            assert!(error.contains(expected), "{label}: {error}");
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn starlight_site_generation_rejects_links_escaping_the_docs_tree() {
        for (label, target) in [
            ("parent", "../README.md"),
            ("encoded-parent", "%2e%2e/README.md"),
            ("backslash", "\\\\evil.example/payload.md"),
            ("root-backslash", "/\\evil.example/payload.md"),
        ] {
            let root = temp_dir(label);
            fs::create_dir_all(root.join("docs")).unwrap();
            fs::write(
                root.join("docs/index.md"),
                format!("# Home\n\n[outside]({target})\n"),
            )
            .unwrap();

            let error = generate_starlight_site(&SiteConfig {
                workspace: root.clone(),
                docs: PathBuf::from("docs"),
                output: PathBuf::from(".codocia/starlight"),
                title: "Example Docs".to_string(),
            })
            .unwrap_err()
            .to_string();
            assert!(
                error.contains("escapes the documentation tree"),
                "{label}: {error}"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn starlight_site_generation_rejects_docs_output_nesting() {
        let root = temp_dir("site-output-nesting");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/index.md"), "# Home\n").unwrap();

        let error = generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from("docs/site"),
            title: "Example Docs".to_string(),
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("must not contain one another"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn starlight_regeneration_uses_a_fresh_owned_tree() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("site-fresh-tree");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/index.md"), "# Home\n").unwrap();
        let config = SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        };
        let output = generate_starlight_site(&config).unwrap();
        fs::create_dir_all(output.join("src/pages")).unwrap();
        fs::write(
            output.join("src/pages/pwn.astro"),
            "---\nthrow new Error('executed')\n---\n",
        )
        .unwrap();
        fs::write(output.join(".npmrc"), "script-shell=/tmp/evil\n").unwrap();
        fs::write(output.join("npm-shrinkwrap.json"), "{}\n").unwrap();
        fs::write(output.join("package-lock.json"), "{}\n").unwrap();
        fs::create_dir_all(output.join("node_modules/old")).unwrap();
        fs::write(output.join("node_modules/old/sentinel"), "old\n").unwrap();
        let outside = root.join("outside-dist");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep"), "safe\n").unwrap();
        symlink(&outside, output.join("dist")).unwrap();

        let regenerated = generate_starlight_site(&config).unwrap();

        for removed in [
            "src/pages/pwn.astro",
            ".npmrc",
            "npm-shrinkwrap.json",
            "package-lock.json",
            "node_modules",
            "dist",
        ] {
            assert!(!regenerated.join(removed).exists(), "retained {removed}");
        }
        assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "safe\n");
        assert!(regenerated.join(SITE_OWNER_MARKER).is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn starlight_regeneration_refuses_unowned_output_and_preserves_it() {
        let root = temp_dir("site-unowned-output");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/index.md"), "# Home\n").unwrap();
        fs::create_dir_all(root.join("custom-output")).unwrap();
        fs::write(root.join("custom-output/keep.txt"), "human data\n").unwrap();

        let error = generate_starlight_site(&SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from("custom-output"),
            title: "Example Docs".to_string(),
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("refusing to replace unowned non-empty site output"));
        assert_eq!(
            fs::read_to_string(root.join("custom-output/keep.txt")).unwrap(),
            "human data\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_starlight_regeneration_preserves_the_previous_site() {
        let root = temp_dir("site-failed-fresh-tree");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/index.md"), "# Home\n").unwrap();
        let config = SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        };
        let output = generate_starlight_site(&config).unwrap();
        fs::write(output.join("previous-sentinel"), "previous\n").unwrap();
        fs::write(root.join("docs/active.mdx"), "# Active\n").unwrap();

        let error = generate_starlight_site(&config).unwrap_err().to_string();

        assert!(error.contains("refusing executable MDX"));
        assert_eq!(
            fs::read_to_string(output.join("previous-sentinel")).unwrap(),
            "previous\n"
        );
        assert!(output.join(SITE_OWNER_MARKER).is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn skip_install_preserves_only_a_real_explicitly_trusted_dependency_cache() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("site-skip-install-cache");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/index.md"), "# Home\n").unwrap();
        let config = SiteConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            output: PathBuf::from(".codocia/starlight"),
            title: "Example Docs".to_string(),
        };
        let output = generate_starlight_site(&config).unwrap();
        fs::create_dir_all(output.join("node_modules/cache")).unwrap();
        fs::write(output.join("node_modules/cache/sentinel"), "trusted\n").unwrap();
        fs::create_dir_all(output.join("node_modules/.astro")).unwrap();
        fs::write(
            output.join("node_modules/.astro/data-store.json"),
            "stale\n",
        )
        .unwrap();
        fs::create_dir_all(output.join("src/pages")).unwrap();
        fs::write(output.join("src/pages/pwn.astro"), "malicious\n").unwrap();

        let regenerated = generate_starlight_site_inner(&config, true).unwrap();
        assert_eq!(
            fs::read_to_string(regenerated.join("node_modules/cache/sentinel")).unwrap(),
            "trusted\n"
        );
        assert!(!regenerated.join("node_modules/.astro").exists());
        assert!(!regenerated.join("src/pages/pwn.astro").exists());

        fs::remove_dir_all(regenerated.join("node_modules")).unwrap();
        let outside = root.join("outside-node-modules");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, regenerated.join("node_modules")).unwrap();
        fs::write(regenerated.join("previous-sentinel"), "previous\n").unwrap();
        let error = generate_starlight_site_inner(&config, true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("failed to preserve the explicitly trusted dependency cache"));
        assert_eq!(
            fs::read_to_string(regenerated.join("previous-sentinel")).unwrap(),
            "previous\n"
        );
        assert!(
            fs::symlink_metadata(regenerated.join("node_modules"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn plain_docs_reject_symlinks_and_non_markdown_direct_requests() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("plain-doc-symlinks");
        let docs = root.join("docs");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("index.md"), "# Home\n").unwrap();
        fs::write(docs.join("active.mdx"), "export const active = true\n").unwrap();
        fs::write(docs.join(".codocia-manifest.json"), "secret metadata\n").unwrap();
        fs::write(root.join("outside.md"), "outside secret\n").unwrap();

        let response = plain_response_for_path(&docs, "/index").unwrap();
        assert_eq!(response.0, "200 OK");
        let metadata_response =
            plain_response_for_path(&docs, "/md/.codocia-manifest.json").unwrap();
        assert_eq!(metadata_response.0, "404 Not Found");
        let index_response = plain_response_for_path(&docs, "/").unwrap();
        assert!(!index_response.2.contains("active.mdx"));

        symlink(root.join("outside.md"), docs.join("leak.md")).unwrap();
        let error = plain_response_for_path(&docs, "/md/leak.md")
            .unwrap_err()
            .to_string();
        assert!(error.contains("refusing symbolic link"));
        let mut pages = Vec::new();
        assert!(
            collect_markdown_files(&docs, &mut pages)
                .unwrap_err()
                .to_string()
                .contains("refusing symbolic link")
        );

        fs::remove_file(docs.join("leak.md")).unwrap();
        let linked_root = root.join("linked-docs");
        symlink(&docs, &linked_root).unwrap();
        assert!(
            collect_markdown_files(&linked_root, &mut Vec::new())
                .unwrap_err()
                .to_string()
                .contains("symbolic-link Markdown root")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn starlight_image_asset_allowlist_is_explicit_and_case_insensitive() {
        for extension in ["png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "JPG"] {
            assert!(is_starlight_image_extension(extension), "{extension}");
        }
        for extension in ["svg", "SVG", "bmp", "tiff", "md", "js", ""] {
            assert!(!is_starlight_image_extension(extension), "{extension}");
        }
    }

    #[test]
    fn starlight_copy_removes_only_an_exact_leading_atx_title() {
        assert_eq!(
            remove_duplicate_leading_title("\n# Exact\n\nBody\n", "Exact"),
            "\n\nBody\n"
        );
        assert_eq!(
            remove_duplicate_leading_title("# Exact ##\nBody\n", "Exact"),
            "Body\n"
        );
        assert_eq!(
            remove_duplicate_leading_title("# Different\n# Exact\n", "Exact"),
            "# Different\n# Exact\n"
        );
        assert_eq!(
            remove_duplicate_leading_title("Intro.\n\n# Exact\n", "Exact"),
            "Intro.\n\n# Exact\n"
        );
        assert_eq!(
            remove_duplicate_leading_title("Exact\n=====\n", "Exact"),
            "Exact\n=====\n"
        );
    }

    #[test]
    fn site_slugs_are_portable_and_nested_indexes_map_to_their_directory() {
        assert_eq!(
            site_slug(Path::new("architecture.zh-CN.md")),
            "architecture-zh-cn"
        );
        assert_eq!(site_slug(Path::new("guides/index.md")), "guides");
        assert_eq!(
            site_slug(Path::new("guides/GPU Setup.md")),
            "guides/gpu-setup"
        );
    }

    #[test]
    fn check_detects_stale_snapshot() {
        let root = temp_dir("stale");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("crates/runtime/src")).unwrap();
        fs::write(root.join("crates/runtime/src/lib.rs"), "pub fn old() {}\n").unwrap();
        fs::write(
            root.join("docs/runtime.md"),
            "---\ntitle: Runtime\ncovers:\n  - crates/runtime/**\n---\n\n# Runtime\n",
        )
        .unwrap();

        snapshot(&SnapshotConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
        })
        .unwrap();
        fs::write(root.join("crates/runtime/src/lib.rs"), "pub fn new() {}\n").unwrap();

        let error = check(&CheckConfig {
            workspace: root.clone(),
            docs: PathBuf::from("docs"),
            base: None,
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("stale docs"));
        assert!(error.contains("docs/runtime.md"));
        assert!(error.contains("git diff review"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diff_excerpt_truncates_long_output() {
        let diff = (0..(DIFF_LINE_LIMIT + 2))
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let excerpt = diff_excerpt(&diff);

        assert!(excerpt.contains("line 0"));
        assert!(!excerpt.contains(&format!("line {}", DIFF_LINE_LIMIT + 1)));
        assert!(excerpt.contains("truncated 2 line(s)"));
    }

    #[test]
    fn check_pass_quality_note_warns_against_template_docs() {
        assert!(CHECK_PASS_QUALITY_NOTE.contains("check passed only means"));
        assert!(CHECK_PASS_QUALITY_NOTE.contains("Do not bulk-generate templated"));
        assert!(CHECK_PASS_QUALITY_NOTE.contains("low-information docs"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("codocia-{name}-{suffix}"))
    }
}
