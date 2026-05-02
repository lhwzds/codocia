//! Library API for Codocia.

use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DIFF_LINE_LIMIT: usize = 80;

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

    let config_path = root.join("codocia.toml");
    if !config_path.exists() {
        fs::write(
            &config_path,
            "[docs]\nroot = \"docs\"\n\n[check]\nbase = \"main\"\n",
        )?;
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

fn is_code_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("rs" | "py")
    )
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git" | ".codocia" | "target" | "node_modules" | "__pycache__" | ".venv" | "dist"
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
