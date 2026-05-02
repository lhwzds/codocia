//! Library API for Codocia.

use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocPage {
    path: PathBuf,
    relative_path: PathBuf,
    content: String,
    frontmatter: Option<Frontmatter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Frontmatter {
    text: String,
    body: String,
    covers: Vec<String>,
    files: BTreeMap<PathBuf, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshot {
    path: PathBuf,
    hash: String,
}

pub fn init(path: impl AsRef<Path>) -> Result<()> {
    let root = path.as_ref();
    let docs_dir = root.join("docs");
    fs::create_dir_all(&docs_dir)?;

    let config_path = root.join("codocia.toml");
    if !config_path.exists() {
        fs::write(
            &config_path,
            "[docs]\nroot = \"docs\"\n\n[check]\nbase = \"main\"\n",
        )?;
    }

    let index_path = docs_dir.join("index.md");
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
    let mut updated = Vec::new();

    for page in read_doc_pages(&docs_dir)? {
        let Some(frontmatter) = &page.frontmatter else {
            continue;
        };
        if frontmatter.covers.is_empty() {
            continue;
        }

        let files = snapshot_files(&workspace, &frontmatter.covers)?;
        let content = write_snapshot(&page.content, &files, &commit)?;
        fs::write(&page.path, content)
            .with_context(|| format!("failed to write {}", page.path.display()))?;
        updated.push(page.relative_path);
    }

    println!("snapshot updated {} doc page(s)", updated.len());
    for path in updated {
        println!("- {}", path.display());
    }

    Ok(())
}

pub fn check(config: &CheckConfig) -> Result<()> {
    let workspace = normalize_dir(&config.workspace)?;
    let docs_dir = resolve_path(&workspace, &config.docs);
    let pages = read_doc_pages(&docs_dir)?;
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

        for (path, expected_hash) in &frontmatter.files {
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

    if broken_covers.is_empty()
        && stale_docs.is_empty()
        && missing_files.is_empty()
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

    let mut message = String::new();
    message.push_str("codocia check failed");
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

    bail!(message);
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
            let frontmatter = parse_frontmatter(&content);
            let relative_path = docs_dir
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(relative_path(docs_dir, &path));
            Ok(DocPage {
                relative_path,
                path,
                content,
                frontmatter,
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
    let text = rest[..end].to_string();
    let body = rest[end + "\n---\n".len()..].to_string();
    Some(Frontmatter {
        covers: parse_covers(&text),
        files: parse_snapshot_files(&text),
        text,
        body,
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

fn parse_snapshot_files(text: &str) -> BTreeMap<PathBuf, String> {
    let mut files = BTreeMap::new();
    let mut in_codocia = false;
    let mut in_files = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "codocia:" {
            in_codocia = true;
            in_files = false;
            continue;
        }
        if in_codocia && is_top_level_key(line) {
            break;
        }
        if in_codocia && trimmed == "files:" {
            in_files = true;
            continue;
        }
        if in_files {
            if trimmed.is_empty() {
                continue;
            }
            if line.starts_with("    ")
                && let Some((path, hash)) = trimmed.split_once(": ")
            {
                files.insert(PathBuf::from(path), hash.to_string());
            }
        }
    }
    files
}

fn write_snapshot(content: &str, files: &[FileSnapshot], commit: &str) -> Result<String> {
    let frontmatter = parse_frontmatter(content).context("snapshot requires frontmatter")?;
    let mut lines = Vec::new();
    let mut skip_codocia = false;

    for line in frontmatter.text.lines() {
        if line.trim() == "codocia:" {
            skip_codocia = true;
            continue;
        }
        if skip_codocia && is_top_level_key(line) {
            skip_codocia = false;
        }
        if !skip_codocia {
            lines.push(line.to_string());
        }
    }

    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.push("codocia:".to_string());
    lines.push(format!("  commit: {commit}"));
    lines.push("  files:".to_string());
    for file in files {
        lines.push(format!("    {}: {}", file.path.display(), file.hash));
    }

    Ok(format!(
        "---\n{}\n---\n{}",
        lines.join("\n"),
        frontmatter.body
    ))
}

fn snapshot_files(workspace: &Path, covers: &[String]) -> Result<Vec<FileSnapshot>> {
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
            Ok(FileSnapshot {
                path,
                hash: hash_file(&absolute_path)?,
            })
        })
        .collect()
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
                ".git" | "target" | "node_modules" | "__pycache__" | ".venv" | "dist"
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
    fn snapshot_replaces_existing_codocia_block() {
        let content = "---\ntitle: Runtime\ncovers:\n  - crates/runtime/**\ncodocia:\n  commit: old\n  files:\n    crates/runtime/src/lib.rs: old\n---\n\n# Runtime\n";
        let files = vec![FileSnapshot {
            path: PathBuf::from("crates/runtime/src/lib.rs"),
            hash: "new".to_string(),
        }];

        let output = write_snapshot(content, &files, "abc123").unwrap();

        assert!(output.contains("commit: abc123"));
        assert!(output.contains("crates/runtime/src/lib.rs: new"));
        assert!(!output.contains("commit: old"));
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
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("codocia-{name}-{suffix}"))
    }
}
