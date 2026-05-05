//! Library API for Codocia.

use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const DIFF_LINE_LIMIT: usize = 80;
const DEFAULT_CODOCIA_POLICY: &str = r#"# Codocia Documentation Policy

Use this file to guide AI coding agents that update Markdown docs in this
repository. Codocia does not parse this file as machine config; agents read it
before editing docs.

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    commit: String,
    docs: BTreeMap<PathBuf, DocSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    let workspace = normalize_dir(&config.workspace)?;
    let docs_dir = resolve_path(&workspace, &config.docs);
    let pages = read_doc_pages(&docs_dir)?;
    let snapshot = read_snapshot_file(&docs_dir)?;
    let code_files = collect_code_files(&workspace, &docs_dir)?;
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

    let uncovered = code_files
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
            "codocia check passed: {} code file(s), {} covered file(s)",
            code_files.len(),
            covered_by_file.len()
        );
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

pub fn generate_starlight_site(config: &SiteConfig) -> Result<PathBuf> {
    let workspace = normalize_dir(&config.workspace)?;
    let docs_dir = resolve_path(&workspace, &config.docs);
    if !docs_dir.is_dir() {
        bail!("docs directory does not exist: {}", docs_dir.display());
    }

    let output = resolve_path(&workspace, &config.output);
    let output = if output.exists() {
        normalize_dir(&output)?
    } else {
        output
    };
    if output == workspace {
        bail!("site output must not be the workspace root");
    }
    if same_path_if_exists(&output, &docs_dir)? {
        bail!("site output must not be the docs directory");
    }

    let site_docs_dir = output.join("src/content/docs");
    let raw_docs_dir = output.join("public/md");
    if site_docs_dir.exists() {
        fs::remove_dir_all(&site_docs_dir)
            .with_context(|| format!("remove {}", site_docs_dir.display()))?;
    }
    if raw_docs_dir.exists() {
        fs::remove_dir_all(&raw_docs_dir)
            .with_context(|| format!("remove {}", raw_docs_dir.display()))?;
    }

    fs::create_dir_all(&site_docs_dir)
        .with_context(|| format!("create {}", site_docs_dir.display()))?;
    fs::create_dir_all(&raw_docs_dir)
        .with_context(|| format!("create {}", raw_docs_dir.display()))?;
    fs::create_dir_all(output.join("src"))
        .with_context(|| format!("create {}", output.join("src").display()))?;
    fs::create_dir_all(output.join("public"))
        .with_context(|| format!("create {}", output.join("public").display()))?;

    let mut markdown_files = Vec::new();
    collect_markdown_files(&docs_dir, &mut markdown_files)?;
    markdown_files.sort();
    if markdown_files.is_empty() {
        bail!(
            "docs directory contains no Markdown files: {}",
            docs_dir.display()
        );
    }

    let mut copied_docs = Vec::new();
    for source in markdown_files {
        let relative = relative_path(&docs_dir, &source);
        let content = fs::read_to_string(&source)
            .with_context(|| format!("failed to read {}", source.display()))?;
        let site_content = ensure_starlight_title(&content, &relative);
        write_file(site_docs_dir.join(&relative), site_content.as_bytes())?;
        write_file(raw_docs_dir.join(&relative), content.as_bytes())?;
        copied_docs.push(relative);
    }

    write_file(
        output.join("package.json"),
        render_site_package_json(&config.title).as_bytes(),
    )?;
    write_file(
        output.join("astro.config.mjs"),
        render_astro_config(&config.title).as_bytes(),
    )?;
    write_file(output.join("tsconfig.json"), SITE_TSCONFIG.as_bytes())?;
    write_file(
        output.join("src/content.config.ts"),
        SITE_CONTENT_CONFIG.as_bytes(),
    )?;
    write_file(output.join("README.md"), render_site_readme().as_bytes())?;
    write_file(
        output.join("public/llms.txt"),
        render_llms_index(&config.title, &copied_docs).as_bytes(),
    )?;
    write_file(
        output.join("public/llms-full.txt"),
        render_llms_full(&config.title, &docs_dir, &copied_docs).as_bytes(),
    )?;

    println!(
        "generated Starlight docs site at {} from {} Markdown page(s)",
        output.display(),
        copied_docs.len()
    );
    println!("next steps:");
    println!("- cd {}", output.display());
    println!("- npm install");
    println!("- npm run dev");

    Ok(output)
}

pub fn starlight_build(config: &SiteBuildConfig) -> Result<()> {
    let output = generate_starlight_site(&config.site)?;
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
    let output = generate_starlight_site(&config.site)?;
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
    let docs_dir = resolve_path(&workspace, &config.docs);
    if !docs_dir.is_dir() {
        bail!("docs directory does not exist: {}", docs_dir.display());
    }
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
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, out)?;
        } else if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("md" | "mdx")
        ) {
            out.push(path);
        }
    }
    Ok(())
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
    let mut output = String::new();
    output.push_str("{\n");
    output.push_str(&format!(
        "  \"commit\": \"{}\",\n",
        escape_json(&snapshot.commit)
    ));
    output.push_str("  \"docs\": {\n");
    for (index, (doc, doc_snapshot)) in snapshot.docs.iter().enumerate() {
        output.push_str(&format!("    \"{}\": {{\n", escape_json_path(doc)));
        output.push_str("      \"covers\": [");
        for (cover_index, cover) in doc_snapshot.covers.iter().enumerate() {
            if cover_index > 0 {
                output.push_str(", ");
            }
            output.push_str(&format!("\"{}\"", escape_json(cover)));
        }
        output.push_str("],\n");
        output.push_str("      \"files\": {\n");
        for (file_index, (file, hash)) in doc_snapshot.files.iter().enumerate() {
            output.push_str(&format!(
                "        \"{}\": \"{}\"",
                escape_json_path(file),
                escape_json(hash)
            ));
            if file_index + 1 < doc_snapshot.files.len() {
                output.push(',');
            }
            output.push('\n');
        }
        output.push_str("      }\n");
        output.push_str("    }");
        if index + 1 < snapshot.docs.len() {
            output.push(',');
        }
        output.push('\n');
    }
    output.push_str("  }\n");
    output.push_str("}\n");
    Ok(output)
}

fn parse_snapshot(content: &str) -> Result<Snapshot> {
    let commit = parse_json_string_field(content, "commit").unwrap_or_default();
    let mut docs = BTreeMap::new();
    let docs_body = json_object_body(content, "docs").context("snapshot is missing docs object")?;
    let mut cursor = 0;

    while let Some((doc, body, next_cursor)) = next_named_object(docs_body, cursor) {
        let covers = parse_json_string_array(body, "covers");
        let files_body = json_object_body(body, "files").unwrap_or("");
        let files = parse_json_string_map(files_body);
        docs.insert(PathBuf::from(doc), DocSnapshot { covers, files });
        cursor = next_cursor;
    }

    Ok(Snapshot { commit, docs })
}

fn parse_json_string_field(content: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\": \"");
    let start = content.find(&marker)? + marker.len();
    let end = content[start..].find('"')? + start;
    Some(unescape_json(&content[start..end]))
}

fn parse_json_string_array(content: &str, key: &str) -> Vec<String> {
    let marker = format!("\"{key}\": [");
    let Some(start) = content.find(&marker).map(|index| index + marker.len()) else {
        return Vec::new();
    };
    let Some(end) = content[start..].find(']').map(|index| index + start) else {
        return Vec::new();
    };
    content[start..end]
        .split(',')
        .filter_map(|part| {
            let trimmed = part.trim();
            trimmed
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .map(unescape_json)
        })
        .collect()
}

fn parse_json_string_map(content: &str) -> BTreeMap<PathBuf, String> {
    let mut map = BTreeMap::new();
    for line in content.lines() {
        let trimmed = line.trim().trim_end_matches(',');
        if !trimmed.starts_with('"') {
            continue;
        }
        let Some(split) = trimmed.find("\": \"") else {
            continue;
        };
        let key = &trimmed[1..split];
        let value_start = split + "\": \"".len();
        let Some(value) = trimmed[value_start..].strip_suffix('"') else {
            continue;
        };
        map.insert(PathBuf::from(unescape_json(key)), unescape_json(value));
    }
    map
}

fn json_object_body<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\": {{");
    let start = content.find(&marker)? + marker.len() - 1;
    let end = matching_brace(content, start)?;
    Some(&content[start + 1..end])
}

fn next_named_object(content: &str, start: usize) -> Option<(String, &str, usize)> {
    let mut cursor = content[start..].find('"')? + start;
    let key_start = cursor + 1;
    let key_end = content[key_start..].find('"')? + key_start;
    let key = unescape_json(&content[key_start..key_end]);
    cursor = key_end + 1;
    let object_start = content[cursor..].find('{')? + cursor;
    let object_end = matching_brace(content, object_start)?;
    Some((key, &content[object_start + 1..object_end], object_end + 1))
}

fn matching_brace(content: &str, start: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        if *byte == b'"' {
            in_string = true;
        } else if *byte == b'{' {
            depth += 1;
        } else if *byte == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(start + offset);
            }
        }
    }
    None
}

fn escape_json_path(path: &Path) -> String {
    escape_json(&path.to_string_lossy().replace('\\', "/"))
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape_json(value: &str) -> String {
    value.replace("\\\"", "\"").replace("\\\\", "\\")
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
    Ok(stable_hash_hex(&bytes))
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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
    "bash", "c", "cc", "cjs", "cpp", "cs", "cts", "cxx", "fish", "go", "h", "hh", "hpp", "java",
    "js", "jsx", "kt", "kts", "lua", "mjs", "mts", "php", "py", "pyi", "r", "rb", "rs", "sh",
    "sql", "svelte", "swift", "ts", "tsx", "vue", "zsh",
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
    if site_dir.join("node_modules").is_dir() {
        return Ok(());
    }
    run_status_command(
        Command::new("npm").arg("install").current_dir(site_dir),
        "npm install",
    )
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
        paths.sort();
        return Ok(("200 OK", "text/html", render_plain_index(docs_dir, &paths)));
    }
    let Some(relative) = plain_doc_path(clean_path) else {
        return Ok(("404 Not Found", "text/plain", "not found\n".to_string()));
    };
    let path = docs_dir.join(&relative);
    if !path.is_file() {
        return Ok(("404 Not Found", "text/plain", "not found\n".to_string()));
    }
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
    if value.contains("..") || value.starts_with('/') || value.is_empty() {
        return None;
    }
    let mut path = PathBuf::from(value);
    if path.extension().is_none() {
        path.set_extension("md");
    }
    Some(path)
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

fn same_path_if_exists(left: &Path, right: &Path) -> Result<bool> {
    if !left.exists() || !right.exists() {
        return Ok(false);
    }
    Ok(left.canonicalize()? == right.canonicalize()?)
}

fn write_file(path: PathBuf, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn ensure_starlight_title(content: &str, path: &Path) -> String {
    if let Some((frontmatter, body)) = split_frontmatter(content) {
        let title = title_from_frontmatter(frontmatter)
            .or_else(|| title_from_markdown(body))
            .unwrap_or_else(|| title_from_path(path));
        return render_starlight_markdown(&title, &parse_covers(frontmatter), body);
    }
    let title = title_from_markdown(content).unwrap_or_else(|| title_from_path(path));
    render_starlight_markdown(&title, &[], content)
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

fn render_starlight_markdown(title: &str, covers: &[String], body: &str) -> String {
    let mut output = format!("---\ntitle: {}\n", yaml_string(title));
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
    "@astrojs/starlight": "^0.35.0",
    "astro": "^5.0.0",
    "typescript": "^5.0.0"
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

fn render_astro_config(title: &str) -> String {
    format!(
        r#"import {{ defineConfig }} from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({{
  integrations: [
    starlight({{
      title: {},
    }}),
  ],
}});
"#,
        js_string(title)
    )
}

fn js_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

const SITE_TSCONFIG: &str = r#"{
  "extends": "astro/tsconfigs/strict"
}
"#;

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
npm install
npm run dev
```

Source docs remain the source of truth. Regenerate the site with `codocia site`
after changing the source docs.
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
                    "abc".to_string(),
                )]),
            },
        );
        let snapshot = Snapshot {
            commit: "abc123".to_string(),
            docs,
        };

        let rendered = render_snapshot(&snapshot).unwrap();
        let parsed = parse_snapshot(&rendered).unwrap();

        assert_eq!(parsed, snapshot);
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
        assert!(output.join("public/llms.txt").is_file());
        assert!(output.join("public/llms-full.txt").is_file());

        let generated =
            fs::read_to_string(output.join("src/content/docs/guides/usage.md")).unwrap();
        assert!(generated.contains("title: \"Usage\""));
        assert!(generated.contains("# Usage"));

        let config = fs::read_to_string(output.join("src/content.config.ts")).unwrap();
        assert!(config.contains("docsLoader()"));
        assert!(config.contains("covers: z.array(z.string()).optional()"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn site_generation_preserves_existing_frontmatter_title() {
        let content = "---\ntitle: Existing\ncovers:\n  - src/**\n---\n\n# Other\n";
        let generated = ensure_starlight_title(content, Path::new("index.md"));

        assert!(generated.contains("title: \"Existing\""));
        assert!(generated.contains("  - \"src/**\""));
        assert!(generated.contains("# Other"));
    }

    #[test]
    fn site_generation_sanitizes_invalid_frontmatter_lines() {
        let content = "---\ntitle: Hooks\ncovers:\n  - src/**/*.rs\n.rs\n---\n\n# Hooks\n";
        let generated = ensure_starlight_title(content, Path::new("hooks.md"));

        assert!(generated.contains("title: \"Hooks\""));
        assert!(generated.contains("  - \"src/**/*.rs\""));
        assert!(!generated.contains("\n.rs\n"));
        assert!(generated.contains("# Hooks"));
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

    fn temp_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("codocia-{name}-{suffix}"))
    }
}
