//! Deterministic repository inventory for documentation generation.
//!
//! Git repositories are discovered with `git ls-files --cached --others
//! --exclude-standard`, so both tracked and untracked working-tree content is
//! represented. Repositories without Git fall back to a sorted recursive walk.

use crate::generation_model::{
    FileDisposition, FileEntry, FileKind, GENERATION_SCHEMA_VERSION, Inventory,
    validate_portable_relative_path,
};
use crate::integrity::{Sha256Fingerprint, sha256_fingerprint};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_MAX_FILE_BYTES: u64 = 1_048_576;
const BINARY_SAMPLE_BYTES: usize = 8 * 1024;

const EXCLUDED_DIRECTORIES: &[&str] = &[".git", ".codocia", "target", "node_modules", "vendor"];

const BINARY_EXTENSIONS: &[&str] = &[
    "7z",
    "a",
    "avi",
    "bin",
    "bmp",
    "class",
    "dylib",
    "exe",
    "gif",
    "gguf",
    "gz",
    "ico",
    "jar",
    "jpeg",
    "jpg",
    "mov",
    "mp3",
    "mp4",
    "o",
    "obj",
    "onnx",
    "otf",
    "pdf",
    "png",
    "pyc",
    "safetensors",
    "so",
    "tar",
    "tflite",
    "ttf",
    "wav",
    "webp",
    "woff",
    "woff2",
    "xz",
    "zip",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryOptions {
    pub max_file_bytes: u64,
}

impl Default for InventoryOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

/// Anchored, workspace-relative inventory filters.
///
/// `*` and `?` match within one path segment. A segment equal to `**` matches
/// zero or more complete segments. An empty include list includes every safe
/// text candidate. Exclude patterns take precedence over include patterns and
/// cannot override Codocia's built-in safety exclusions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventoryPatterns {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl InventoryPatterns {
    pub fn validate(&self) -> Result<()> {
        for (label, patterns) in [("include", &self.include), ("exclude", &self.exclude)] {
            for pattern in patterns {
                validate_pattern(pattern)
                    .with_context(|| format!("invalid {label} pattern `{pattern}`"))?;
            }
        }
        Ok(())
    }
}

pub fn build_inventory(workspace: impl AsRef<Path>) -> Result<Inventory> {
    build_inventory_with_options_and_patterns(
        workspace,
        InventoryOptions::default(),
        &InventoryPatterns::default(),
    )
}

pub fn build_inventory_with_options(
    workspace: impl AsRef<Path>,
    options: InventoryOptions,
) -> Result<Inventory> {
    build_inventory_with_options_and_patterns(workspace, options, &InventoryPatterns::default())
}

pub fn build_inventory_with_patterns(
    workspace: impl AsRef<Path>,
    patterns: &InventoryPatterns,
) -> Result<Inventory> {
    build_inventory_with_options_and_patterns(workspace, InventoryOptions::default(), patterns)
}

pub fn build_inventory_with_options_and_patterns(
    workspace: impl AsRef<Path>,
    options: InventoryOptions,
    patterns: &InventoryPatterns,
) -> Result<Inventory> {
    if options.max_file_bytes == 0 {
        bail!("max_file_bytes must be greater than zero");
    }
    patterns.validate()?;

    let requested_root = workspace.as_ref().canonicalize().with_context(|| {
        format!(
            "failed to resolve inventory workspace {}",
            workspace.as_ref().display()
        )
    })?;
    if !requested_root.is_dir() {
        bail!(
            "inventory workspace is not a directory: {}",
            requested_root.display()
        );
    }

    let git = discover_git_repository(&requested_root)?;
    let (source_revision, paths) = match git {
        Some(repository) => {
            let paths = git_paths(&requested_root, &repository.prefix)?;
            (repository.revision, paths)
        }
        None => {
            let mut paths = BTreeSet::new();
            collect_fallback_paths(&requested_root, &requested_root, &mut paths)?;
            (None, paths)
        }
    };

    let mut files = Vec::new();
    for path in paths {
        let absolute = requested_root.join(&path);
        let mut entry = inspect_path(&absolute, path, options)?;
        apply_patterns(&mut entry, patterns)?;
        files.push(entry);
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let languages = files
        .iter()
        .filter(|file| matches!(&file.disposition, FileDisposition::Include))
        .filter_map(|file| file.language.clone())
        .collect::<BTreeSet<_>>();
    let source_fingerprint = inventory_fingerprint(&files);
    let project_name = requested_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("repository")
        .to_string();

    let inventory = Inventory {
        schema_version: GENERATION_SCHEMA_VERSION,
        project_name,
        source_revision,
        source_fingerprint,
        languages,
        files,
    };
    inventory
        .validate()
        .map_err(|errors| anyhow::anyhow!(errors.join("\n")))?;
    Ok(inventory)
}

#[derive(Debug)]
struct GitRepository {
    prefix: PathBuf,
    revision: Option<String>,
}

fn discover_git_repository(workspace: &Path) -> Result<Option<GitRepository>> {
    let output = match Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to inspect Git repository"),
    };
    if !output.status.success() {
        return Ok(None);
    }
    if String::from_utf8_lossy(&output.stdout).trim() != "true" {
        return Ok(None);
    }

    let prefix_output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("rev-parse")
        .arg("--show-prefix")
        .output()
        .context("failed to inspect Git workspace prefix")?;
    if !prefix_output.status.success() {
        bail!("git rev-parse --show-prefix failed");
    }
    let raw_prefix = String::from_utf8(prefix_output.stdout)
        .context("Git workspace prefix is not valid UTF-8")?;
    let prefix = portable_path(raw_prefix.trim_end_matches(['/', '\r', '\n']));
    if !prefix.as_os_str().is_empty() {
        validate_portable_relative_path(&prefix)
            .map_err(|error| anyhow::anyhow!("unsafe Git workspace prefix: {error}"))?;
    }

    let revision_output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .context("failed to inspect Git revision")?;
    let revision = revision_output.status.success().then(|| {
        String::from_utf8_lossy(&revision_output.stdout)
            .trim()
            .to_string()
    });

    Ok(Some(GitRepository { prefix, revision }))
}

fn git_paths(workspace: &Path, prefix: &Path) -> Result<BTreeSet<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("ls-files")
        .arg("--cached")
        .arg("--others")
        .arg("--exclude-standard")
        .arg("--full-name")
        .arg("-z")
        .arg("--")
        .arg(".")
        .output()
        .context("failed to run git ls-files")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git ls-files failed: {}", stderr.trim());
    }

    let mut paths = BTreeSet::new();
    for raw in output.stdout.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let text = String::from_utf8(raw.to_vec()).with_context(|| {
            format!(
                "git ls-files returned a non-UTF-8 path: {:?}",
                String::from_utf8_lossy(raw)
            )
        })?;
        let repository_relative = portable_path(&text);
        validate_portable_relative_path(&repository_relative)
            .map_err(|error| anyhow::anyhow!("unsafe path returned by Git: {error}"))?;
        let path = if prefix.as_os_str().is_empty() {
            repository_relative
        } else {
            repository_relative
                .strip_prefix(prefix)
                .with_context(|| {
                    format!(
                        "Git returned `{}` outside requested workspace prefix `{}`",
                        repository_relative.display(),
                        prefix.display()
                    )
                })?
                .to_path_buf()
        };
        if path.as_os_str().is_empty() {
            continue;
        }
        validate_portable_relative_path(&path).map_err(|error| {
            anyhow::anyhow!("unsafe path relative to requested workspace: {error}")
        })?;
        paths.insert(path);
    }
    Ok(paths)
}

fn collect_fallback_paths(root: &Path, dir: &Path, out: &mut BTreeSet<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("{} is outside inventory root", path.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if file_type.is_dir() {
            if excluded_directory(relative).is_none() {
                collect_fallback_paths(root, &path, out)?;
            }
        } else {
            let portable = portable_path(&relative.to_string_lossy());
            validate_portable_relative_path(&portable)
                .map_err(|error| anyhow::anyhow!("unsafe fallback path: {error}"))?;
            out.insert(portable);
        }
    }
    Ok(())
}

fn inspect_path(absolute: &Path, path: PathBuf, options: InventoryOptions) -> Result<FileEntry> {
    let kind = classify_path(&path);
    let language = language_for_path(&path).map(str::to_string);
    let metadata = match fs::symlink_metadata(absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(excluded_entry(
                path,
                kind,
                language,
                0,
                None,
                "file does not exist in the working tree",
            ));
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", absolute.display()));
        }
    };
    let size_bytes = metadata.len();

    if metadata.file_type().is_symlink() {
        return Ok(excluded_entry(
            path,
            kind,
            language,
            size_bytes,
            None,
            "symbolic links are excluded to keep inventory inside the workspace",
        ));
    }
    if !metadata.is_file() {
        return Ok(excluded_entry(
            path,
            kind,
            language,
            size_bytes,
            None,
            "path is not a regular file",
        ));
    }
    if let Some(directory) = excluded_directory(&path) {
        return Ok(excluded_entry(
            path,
            kind,
            language,
            size_bytes,
            None,
            format!("path is under excluded directory `{directory}`"),
        ));
    }
    if is_secret_path(&path) {
        return Ok(excluded_entry(
            path,
            kind,
            language,
            size_bytes,
            None,
            "potential secret file is excluded",
        ));
    }
    if size_bytes > options.max_file_bytes {
        return Ok(excluded_entry(
            path,
            kind,
            language,
            size_bytes,
            None,
            format!(
                "file is larger than the {} byte inventory limit",
                options.max_file_bytes
            ),
        ));
    }

    let bytes = fs::read(absolute)
        .with_context(|| format!("failed to read inventory file {}", absolute.display()))?;
    let content_hash = Some(sha256_fingerprint(&bytes));
    if is_lockfile(&path) {
        return Ok(excluded_entry(
            path,
            kind,
            language,
            size_bytes,
            content_hash,
            "dependency lockfile is excluded",
        ));
    }
    if kind == FileKind::Documentation {
        return Ok(excluded_entry(
            path,
            kind,
            language,
            size_bytes,
            content_hash,
            "existing documentation is migration context, not source coverage",
        ));
    }
    if has_binary_extension(&path) || looks_binary(&bytes) {
        return Ok(excluded_entry(
            path,
            kind,
            language,
            size_bytes,
            content_hash,
            "binary file is excluded",
        ));
    }
    Ok(FileEntry {
        path,
        kind,
        language,
        size_bytes,
        content_hash,
        disposition: FileDisposition::Include,
    })
}

fn excluded_entry(
    path: PathBuf,
    kind: FileKind,
    language: Option<String>,
    size_bytes: u64,
    content_hash: Option<String>,
    reason: impl Into<String>,
) -> FileEntry {
    FileEntry {
        path,
        kind,
        language,
        size_bytes,
        content_hash,
        disposition: FileDisposition::Exclude {
            reason: reason.into(),
        },
    }
}

fn apply_patterns(entry: &mut FileEntry, patterns: &InventoryPatterns) -> Result<()> {
    if let Some(pattern) = first_matching_pattern(&patterns.exclude, &entry.path)? {
        entry.disposition = FileDisposition::Exclude {
            reason: format!("matched exclude pattern `{pattern}`"),
        };
        return Ok(());
    }
    if !matches!(&entry.disposition, FileDisposition::Include) {
        return Ok(());
    }
    if !path_selected_by_patterns(&entry.path, patterns)? {
        entry.disposition = FileDisposition::Exclude {
            reason: "did not match any include pattern".to_string(),
        };
    }
    Ok(())
}

/// Return whether a path is selected by explicit include/exclude patterns.
///
/// This deliberately ignores built-in file-safety disposition. It can also be
/// used to filter migration-context Markdown before mirroring it into an
/// isolated Codex workspace. Callers must still honor the inventory entry's
/// disposition before using a file as source coverage or model input.
pub fn path_selected_by_patterns(path: &Path, patterns: &InventoryPatterns) -> Result<bool> {
    patterns.validate()?;
    validate_portable_relative_path(path)
        .map_err(|error| anyhow::anyhow!("invalid inventory path: {error}"))?;
    if first_matching_pattern(&patterns.exclude, path)?.is_some() {
        return Ok(false);
    }
    Ok(patterns.include.is_empty() || first_matching_pattern(&patterns.include, path)?.is_some())
}

fn first_matching_pattern<'a>(patterns: &'a [String], path: &Path) -> Result<Option<&'a str>> {
    for pattern in patterns {
        if path_matches_pattern(pattern, path)? {
            return Ok(Some(pattern));
        }
    }
    Ok(None)
}

/// Match an anchored pattern against a portable workspace-relative path.
///
/// `*` matches zero or more characters inside one segment, `?` matches one
/// character inside one segment, and a complete `**` segment matches zero or
/// more path segments. Matching is case-sensitive and covers the whole path.
pub fn path_matches_pattern(pattern: &str, path: &Path) -> Result<bool> {
    validate_pattern(pattern)?;
    validate_portable_relative_path(path)
        .map_err(|error| anyhow::anyhow!("invalid inventory path: {error}"))?;
    let value = path.to_string_lossy();
    let pattern_segments = pattern.split('/').collect::<Vec<_>>();
    let path_segments = value.split('/').collect::<Vec<_>>();
    Ok(match_segments(&pattern_segments, &path_segments))
}

fn validate_pattern(pattern: &str) -> Result<()> {
    if pattern.trim() != pattern || pattern.is_empty() {
        bail!("pattern must be non-empty and have no surrounding whitespace");
    }
    if pattern.starts_with('/') || pattern.contains('\\') {
        bail!("pattern must be a portable workspace-relative path");
    }
    if pattern
        .split('/')
        .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        bail!("pattern contains an empty, parent, or current-directory segment");
    }
    Ok(())
}

fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }
    if pattern[0] == "**" {
        return match_segments(&pattern[1..], path)
            || (!path.is_empty() && match_segments(pattern, &path[1..]));
    }
    if path.is_empty() {
        return false;
    }
    match_segment(pattern[0], path[0]) && match_segments(&pattern[1..], &path[1..])
}

fn match_segment(pattern: &str, value: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star_index = None;
    let mut star_value_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
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

    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

pub fn classify_path(path: &Path) -> FileKind {
    let file_name = file_name_lower(path);
    let extension = extension_lower(path);

    if is_test_path(path) {
        return FileKind::Test;
    }
    if is_build_manifest(&file_name) {
        return FileKind::BuildManifest;
    }
    if is_documentation_path(path) {
        return FileKind::Documentation;
    }
    if matches!(
        extension.as_deref(),
        Some("sh" | "bash" | "zsh" | "fish" | "nu" | "ps1")
    ) {
        return FileKind::Script;
    }
    if matches!(
        extension.as_deref(),
        Some("json" | "json5" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf")
    ) || is_configuration_name(&file_name)
    {
        return FileKind::Configuration;
    }
    if language_for_path(path).is_some() {
        return FileKind::Source;
    }
    if has_binary_extension(path) {
        return FileKind::Asset;
    }
    FileKind::Unknown
}

pub fn language_for_path(path: &Path) -> Option<&'static str> {
    let file_name = file_name_lower(path);
    if file_name == "dockerfile" {
        return Some("Dockerfile");
    }
    if file_name == "makefile" || file_name == "gnumakefile" {
        return Some("Make");
    }

    match extension_lower(path).as_deref() {
        Some("rs") => Some("Rust"),
        Some("metal") => Some("Metal"),
        Some("cu" | "cuh") => Some("CUDA"),
        Some("c") => Some("C"),
        Some("cc" | "cpp" | "cxx") => Some("C++"),
        Some("h" | "hh" | "hpp" | "hxx") => Some("C/C++ Header"),
        Some("py" | "pyi") => Some("Python"),
        Some("ts" | "tsx" | "mts" | "cts") => Some("TypeScript"),
        Some("js" | "jsx" | "mjs" | "cjs") => Some("JavaScript"),
        Some("go") => Some("Go"),
        Some("java") => Some("Java"),
        Some("kt" | "kts") => Some("Kotlin"),
        Some("swift") => Some("Swift"),
        Some("cs") => Some("C#"),
        Some("rb") => Some("Ruby"),
        Some("php") => Some("PHP"),
        Some("scala" | "sc") => Some("Scala"),
        Some("dart") => Some("Dart"),
        Some("zig") => Some("Zig"),
        Some("lua") => Some("Lua"),
        Some("r") => Some("R"),
        Some("sql") => Some("SQL"),
        Some("proto") => Some("Protocol Buffers"),
        Some("graphql" | "gql") => Some("GraphQL"),
        Some("vue") => Some("Vue"),
        Some("svelte") => Some("Svelte"),
        Some("html" | "htm") => Some("HTML"),
        Some("css" | "scss" | "sass" | "less") => Some("CSS"),
        Some("json" | "json5") => Some("JSON"),
        Some("toml") => Some("TOML"),
        Some("yaml" | "yml") => Some("YAML"),
        Some("sh" | "bash" | "zsh" | "fish" | "nu") => Some("Shell"),
        Some("ps1") => Some("PowerShell"),
        _ => None,
    }
}

fn is_test_path(path: &Path) -> bool {
    let segments = path
        .iter()
        .filter_map(|segment| segment.to_str())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if segments.iter().any(|segment| {
        matches!(
            segment.as_str(),
            "test" | "tests" | "spec" | "specs" | "__tests__"
        )
    }) {
        return true;
    }

    let name = file_name_lower(path);
    name.starts_with("test_")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name
            .split_once('.')
            .is_some_and(|(stem, _)| stem.ends_with("_test"))
}

fn is_build_manifest(file_name: &str) -> bool {
    matches!(
        file_name,
        "cargo.toml"
            | "build.gradle"
            | "build.gradle.kts"
            | "cmakelists.txt"
            | "dockerfile"
            | "gemfile"
            | "go.mod"
            | "gradle.properties"
            | "gnumakefile"
            | "makefile"
            | "meson.build"
            | "package.json"
            | "pom.xml"
            | "pyproject.toml"
            | "setup.cfg"
            | "setup.py"
    )
}

pub fn is_documentation_path(path: &Path) -> bool {
    if matches!(
        extension_lower(path).as_deref(),
        Some("md" | "mdx" | "rst" | "adoc")
    ) {
        return true;
    }

    let file_name = file_name_lower(path);
    if matches!(
        file_name.as_str(),
        "readme"
            | "readme.txt"
            | "readme.markdown"
            | "changelog"
            | "changelog.txt"
            | "changelog.markdown"
            | "contributing"
            | "contributing.txt"
            | "contributing.markdown"
            | "license"
            | "license.txt"
            | "license.markdown"
            | "copying"
            | "copying.txt"
            | "notice"
            | "notice.txt"
    ) {
        return true;
    }

    matches!(
        file_name.as_str(),
        "license-0bsd"
            | "license-agpl"
            | "license-agpl-3.0"
            | "license-apache"
            | "license-apache-2.0"
            | "license-bsd"
            | "license-bsd-2-clause"
            | "license-bsd-3-clause"
            | "license-cc0"
            | "license-gpl"
            | "license-gpl-2.0"
            | "license-gpl-3.0"
            | "license-isc"
            | "license-lgpl"
            | "license-lgpl-2.1"
            | "license-lgpl-3.0"
            | "license-mit"
            | "license-mpl"
            | "license-mpl-2.0"
            | "license-unlicense"
    )
}

fn is_configuration_name(file_name: &str) -> bool {
    file_name.starts_with(".gitignore")
        || file_name.starts_with(".dockerignore")
        || file_name == "justfile"
        || file_name == "procfile"
}

fn excluded_directory(path: &Path) -> Option<&'static str> {
    if path
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
        .is_some_and(|segment| segment.eq_ignore_ascii_case("models"))
    {
        return Some("models");
    }
    path.iter()
        .filter_map(|segment| segment.to_str())
        .find_map(|segment| {
            EXCLUDED_DIRECTORIES
                .iter()
                .copied()
                .find(|excluded| segment.eq_ignore_ascii_case(excluded))
        })
}

fn is_lockfile(path: &Path) -> bool {
    let name = file_name_lower(path);
    name.ends_with(".lock")
        || matches!(
            name.as_str(),
            "bun.lockb"
                | "composer.lock"
                | "go.sum"
                | "package-lock.json"
                | "packages.lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
        )
}

fn is_secret_path(path: &Path) -> bool {
    let name = file_name_lower(path);
    if matches!(
        name.as_str(),
        ".env.example" | ".env.sample" | ".env.template"
    ) {
        return false;
    }
    name == ".env"
        || name.starts_with(".env.")
        || name.ends_with(".tfstate")
        || name.ends_with(".tfstate.backup")
        || matches!(
            name.as_str(),
            ".boto"
                | ".netrc"
                | ".npmrc"
                | ".pypirc"
                | ".s3cfg"
                | ".secrets"
                | "access_tokens.db"
                | "accesstokens.json"
                | "application_default_credentials.json"
                | "auth.json"
                | "azureprofile.json"
                | "credentials"
                | "credentials.cfg"
                | "credentials.conf"
                | "credentials.db"
                | "credentials.ini"
                | "credentials.json"
                | "credentials.toml"
                | "credentials.yaml"
                | "credentials.yml"
                | "google_application_credentials.json"
                | "id_dsa"
                | "id_ed25519"
                | "id_rsa"
                | "msal_token_cache.json"
                | "secrets"
                | "secrets.json"
                | "secrets.toml"
                | "secrets.yaml"
                | "secrets.yml"
                | "service-account.json"
                | "service_account.json"
        )
        || (name.starts_with("client_secret") && name.ends_with(".json"))
        || (name.contains("firebase-adminsdk") && name.ends_with(".json"))
        || is_cloud_credential_path(path)
        || matches!(
            extension_lower(path).as_deref(),
            Some("key" | "p12" | "pfx" | "pem")
        )
}

fn is_cloud_credential_path(path: &Path) -> bool {
    let segments = path
        .iter()
        .filter_map(|segment| segment.to_str())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let has_suffix = |suffix: &[&str]| {
        segments.len() >= suffix.len()
            && segments[segments.len() - suffix.len()..]
                .iter()
                .map(String::as_str)
                .eq(suffix.iter().copied())
    };

    has_suffix(&[".docker", "config.json"])
        || has_suffix(&[".kube", "config"])
        || has_suffix(&[".oci", "config"])
        || segments.windows(3).any(|window| {
            window[0] == ".aws"
                && matches!(window[1].as_str(), "cli" | "sso")
                && window[2] == "cache"
        })
}

fn has_binary_extension(path: &Path) -> bool {
    extension_lower(path).is_some_and(|extension| {
        BINARY_EXTENSIONS
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    })
}

fn looks_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(BINARY_SAMPLE_BYTES)];
    sample.contains(&0) || std::str::from_utf8(sample).is_err()
}

fn inventory_fingerprint(files: &[FileEntry]) -> String {
    let mut hash = Sha256Fingerprint::new(b"codocia.inventory.v2");
    hash.field(GENERATION_SCHEMA_VERSION.to_string().as_bytes());
    for file in files {
        hash.field(file.path.to_string_lossy().replace('\\', "/").as_bytes());
        hash.field(file_kind_name(file.kind).as_bytes());
        hash.field(file.size_bytes.to_string().as_bytes());
        hash.field(
            file.content_hash
                .as_deref()
                .unwrap_or("excluded")
                .as_bytes(),
        );
        match &file.disposition {
            FileDisposition::Include => hash.field(b"include"),
            FileDisposition::Exclude { reason } => {
                hash.field(b"exclude");
                hash.field(reason.as_bytes());
            }
        }
    }
    hash.finish()
}

fn file_kind_name(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Source => "source",
        FileKind::Test => "test",
        FileKind::Configuration => "configuration",
        FileKind::BuildManifest => "build_manifest",
        FileKind::Script => "script",
        FileKind::Documentation => "documentation",
        FileKind::Asset => "asset",
        FileKind::Generated => "generated",
        FileKind::Unknown => "unknown",
    }
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn extension_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

fn portable_path(value: &str) -> PathBuf {
    PathBuf::from(value.replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "codocia-inventory-{name}-{}-{counter}",
                std::process::id()
            ));
            if path.exists() {
                fs::remove_dir_all(&path).unwrap();
            }
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, bytes).unwrap();
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn git_inventory_uses_tracked_and_untracked_working_tree_content() {
        let repo = TestDir::new("git");
        run_git(repo.path(), &["init", "--quiet"]);
        repo.write(".gitignore", b"target/\n");
        repo.write("src/lib.rs", b"pub fn tracked() {}\n");
        repo.write("src/new.rs", b"pub fn untracked() {}\n");
        repo.write("target/ignored.rs", b"ignored\n");
        repo.write("Cargo.lock", b"lock-v1\n");
        repo.write(".env", b"TOKEN=do-not-read\n");
        run_git(repo.path(), &["add", ".gitignore", "src/lib.rs"]);

        let first = build_inventory(repo.path()).unwrap();
        let tracked = entry(&first, "src/lib.rs");
        let untracked = entry(&first, "src/new.rs");
        assert!(tracked.content_hash.is_some());
        assert!(untracked.content_hash.is_some());
        assert!(
            first
                .files
                .iter()
                .all(|file| file.path != Path::new("target/ignored.rs"))
        );
        assert!(excluded_reason(entry(&first, "Cargo.lock")).contains("lockfile"));
        assert!(excluded_reason(entry(&first, ".env")).contains("secret"));

        repo.write("src/new.rs", b"pub fn untracked_changed() {}\n");
        let second = build_inventory(repo.path()).unwrap();
        assert_ne!(
            untracked.content_hash,
            entry(&second, "src/new.rs").content_hash
        );
        assert_ne!(first.source_fingerprint, second.source_fingerprint);
    }

    #[test]
    fn git_subdirectory_is_a_hard_inventory_root() {
        let repo = TestDir::new("git-subdirectory");
        run_git(repo.path(), &["init", "--quiet"]);
        repo.write("outside.rs", b"pub fn outside() {}\n");
        repo.write("outside-untracked.rs", b"pub fn outside_untracked() {}\n");
        repo.write("subproject/src/lib.rs", b"pub fn inside() {}\n");
        repo.write("subproject/src/new.rs", b"pub fn inside_untracked() {}\n");
        run_git(repo.path(), &["add", "outside.rs", "subproject/src/lib.rs"]);

        let inventory = build_inventory(repo.path().join("subproject")).unwrap();
        let paths = inventory
            .files
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<BTreeSet<_>>();

        assert_eq!(inventory.project_name, "subproject");
        assert!(paths.contains(Path::new("src/lib.rs")));
        assert!(paths.contains(Path::new("src/new.rs")));
        assert!(!paths.contains(Path::new("subproject/src/lib.rs")));
        assert!(!paths.contains(Path::new("outside.rs")));
        assert!(!paths.contains(Path::new("outside-untracked.rs")));
    }

    #[test]
    fn non_git_inventory_is_sorted_stable_and_classifies_exclusions() {
        let repo = TestDir::new("plain");
        repo.write("src/main.py", b"def main():\n    pass\n");
        repo.write("kernel.metal", b"kernel void add() {}\n");
        repo.write("config.yaml", b"enabled: true\n");
        repo.write("Makefile", b"all:\n\t@true\n");
        repo.write("image.bin", [0, 1, 2, 3]);
        repo.write("private.pem", b"private material\n");
        repo.write("package-lock.json", b"{}\n");
        repo.write("large.txt", vec![b'x'; 33]);
        repo.write("vendor/vendored.rs", b"pub fn vendored() {}\n");

        let options = InventoryOptions { max_file_bytes: 32 };
        let first = build_inventory_with_options(repo.path(), options).unwrap();
        let second = build_inventory_with_options(repo.path(), options).unwrap();

        assert_eq!(first.source_fingerprint, second.source_fingerprint);
        assert_eq!(first.files, second.files);
        assert_eq!(entry(&first, "src/main.py").kind, FileKind::Source);
        assert_eq!(
            entry(&first, "kernel.metal").language.as_deref(),
            Some("Metal")
        );
        assert_eq!(entry(&first, "config.yaml").kind, FileKind::Configuration);
        assert_eq!(entry(&first, "Makefile").kind, FileKind::BuildManifest);
        assert!(excluded_reason(entry(&first, "image.bin")).contains("binary"));
        assert!(excluded_reason(entry(&first, "private.pem")).contains("secret"));
        assert!(excluded_reason(entry(&first, "package-lock.json")).contains("lockfile"));
        assert!(excluded_reason(entry(&first, "large.txt")).contains("larger"));
        assert!(
            first
                .files
                .iter()
                .all(|file| !file.path.starts_with("vendor"))
        );

        let paths = first
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
    }

    #[test]
    fn excludes_only_the_top_level_model_cache_not_source_model_modules() {
        assert_eq!(
            excluded_directory(Path::new("models/qwen/model.gguf")),
            Some("models")
        );
        assert_eq!(excluded_directory(Path::new("src/models/qwen.rs")), None);
    }

    #[test]
    fn secret_path_denylist_covers_state_credentials_and_cloud_files() {
        let secret_paths = [
            "secrets.toml",
            "config/SECRETS.yaml",
            "config/secrets.yml",
            "config/secrets.json",
            ".pypirc",
            "terraform.tfstate",
            "environments/prod.tfstate.backup",
            "config/credentials",
            "config/credentials.toml",
            "config/credentials.yaml",
            ".aws/credentials",
            ".aws/sso/cache/session.json",
            ".config/gcloud/application_default_credentials.json",
            ".azure/accessTokens.json",
            ".azure/azureProfile.json",
            ".azure/msal_token_cache.json",
            ".docker/config.json",
            ".kube/config",
            ".oci/config",
            "oauth/client_secret_web.json",
            "keys/project-firebase-adminsdk-abcde.json",
        ];

        for path in secret_paths {
            assert!(
                is_secret_path(Path::new(path)),
                "expected {path} to be secret"
            );
        }
    }

    #[test]
    fn secret_path_denylist_keeps_only_explicit_env_templates_safe() {
        for path in [".env.example", "config/.ENV.SAMPLE", ".env.template"] {
            assert!(
                !is_secret_path(Path::new(path)),
                "expected {path} to remain a safe template"
            );
        }

        for path in [".env", ".env.local", ".env.production"] {
            assert!(
                is_secret_path(Path::new(path)),
                "expected {path} to be secret"
            );
        }

        for path in [
            "src/credentials.rs",
            "fixtures/secrets.example.toml",
            ".pypirc.example",
            "terraform/main.tf",
        ] {
            assert!(
                !is_secret_path(Path::new(path)),
                "expected {path} not to match the denylist"
            );
        }
    }

    #[test]
    fn inventory_patterns_cannot_reinclude_secret_files() {
        let repo = TestDir::new("secret-patterns");
        repo.write("secrets.toml", b"token = 'do-not-upload'\n");
        repo.write("infra/prod.tfstate", b"{\"private\": true}\n");
        repo.write(".env.example", b"TOKEN=replace-me\n");

        let patterns = InventoryPatterns {
            include: vec!["**".to_string()],
            exclude: Vec::new(),
        };
        let inventory = build_inventory_with_patterns(repo.path(), &patterns).unwrap();

        for path in ["secrets.toml", "infra/prod.tfstate"] {
            let file = entry(&inventory, path);
            assert!(excluded_reason(file).contains("secret"));
            assert_eq!(file.content_hash, None, "must not read or hash {path}");
        }
        assert!(is_included(entry(&inventory, ".env.example")));
    }

    #[test]
    fn recognizes_required_language_and_special_file_names() {
        let cases = [
            ("src/lib.rs", Some("Rust"), FileKind::Source),
            ("shader.metal", Some("Metal"), FileKind::Source),
            ("kernel.cu", Some("CUDA"), FileKind::Source),
            ("native.cpp", Some("C++"), FileKind::Source),
            ("native.h", Some("C/C++ Header"), FileKind::Source),
            ("tool.py", Some("Python"), FileKind::Source),
            ("web.ts", Some("TypeScript"), FileKind::Source),
            ("web.js", Some("JavaScript"), FileKind::Source),
            ("server.go", Some("Go"), FileKind::Source),
            ("Server.java", Some("Java"), FileKind::Source),
            ("Server.kt", Some("Kotlin"), FileKind::Source),
            ("App.swift", Some("Swift"), FileKind::Source),
            ("Program.cs", Some("C#"), FileKind::Source),
            ("tool.rb", Some("Ruby"), FileKind::Source),
            ("index.php", Some("PHP"), FileKind::Source),
            ("data.json", Some("JSON"), FileKind::Configuration),
            ("Cargo.toml", Some("TOML"), FileKind::BuildManifest),
            ("config.yaml", Some("YAML"), FileKind::Configuration),
            ("run.sh", Some("Shell"), FileKind::Script),
            ("Dockerfile", Some("Dockerfile"), FileKind::BuildManifest),
            ("Makefile", Some("Make"), FileKind::BuildManifest),
        ];

        for (path, language, kind) in cases {
            let path = Path::new(path);
            assert_eq!(language_for_path(path), language, "{path:?}");
            assert_eq!(classify_path(path), kind, "{path:?}");
        }
    }

    #[test]
    fn documentation_names_do_not_hide_source_files_from_coverage() {
        assert_eq!(
            classify_path(Path::new("license_verifier.rs")),
            FileKind::Source
        );
        assert_eq!(
            classify_path(Path::new("license-verifier.rs")),
            FileKind::Source
        );
        assert_eq!(classify_path(Path::new("license-key.py")), FileKind::Source);
        assert_eq!(classify_path(Path::new("README_tool.py")), FileKind::Source);
        assert_eq!(
            classify_path(Path::new("LICENSE-MIT")),
            FileKind::Documentation
        );
        assert_eq!(
            classify_path(Path::new("README.md")),
            FileKind::Documentation
        );
    }

    #[test]
    fn include_and_exclude_patterns_are_anchored_and_exclude_wins() {
        let repo = TestDir::new("patterns");
        repo.write("src/lib.rs", b"pub fn library() {}\n");
        repo.write("src/nested/runtime.rs", b"pub fn runtime() {}\n");
        repo.write("src/generated/bindings.rs", b"pub fn generated() {}\n");
        repo.write("other/src/outside.rs", b"pub fn outside() {}\n");
        repo.write("tests/root.rs", b"#[test] fn root() {}\n");
        repo.write("tests/nested/deep.rs", b"#[test] fn deep() {}\n");
        repo.write("notes.txt", b"safe UTF-8 text\n");
        repo.write("docs/legacy.md", b"# Legacy docs\n");

        let patterns = InventoryPatterns {
            include: vec!["src/**".to_string(), "tests/*.rs".to_string()],
            exclude: vec!["src/generated/**".to_string(), "docs/**".to_string()],
        };
        let inventory = build_inventory_with_patterns(repo.path(), &patterns).unwrap();

        assert!(is_included(entry(&inventory, "src/lib.rs")));
        assert!(is_included(entry(&inventory, "src/nested/runtime.rs")));
        assert!(is_included(entry(&inventory, "tests/root.rs")));
        assert!(
            excluded_reason(entry(&inventory, "src/generated/bindings.rs"))
                .contains("matched exclude pattern")
        );
        assert!(
            excluded_reason(entry(&inventory, "tests/nested/deep.rs")).contains("did not match")
        );
        assert!(
            excluded_reason(entry(&inventory, "other/src/outside.rs")).contains("did not match")
        );
        assert!(excluded_reason(entry(&inventory, "notes.txt")).contains("did not match"));
        assert!(
            excluded_reason(entry(&inventory, "docs/legacy.md"))
                .contains("matched exclude pattern")
        );
        assert!(!path_selected_by_patterns(Path::new("docs/legacy.md"), &patterns).unwrap());
        assert!(path_selected_by_patterns(Path::new("src/lib.rs"), &patterns).unwrap());

        let migration_patterns = InventoryPatterns {
            include: vec!["docs/**".to_string()],
            exclude: Vec::new(),
        };
        assert!(
            path_selected_by_patterns(Path::new("docs/legacy.md"), &migration_patterns).unwrap()
        );
        assert!(!path_selected_by_patterns(Path::new("src/lib.rs"), &migration_patterns).unwrap());
    }

    #[test]
    fn pattern_matcher_has_portable_whole_path_semantics() {
        assert!(path_matches_pattern("src/**", Path::new("src/lib.rs")).unwrap());
        assert!(path_matches_pattern("src/**", Path::new("src/a/b.rs")).unwrap());
        assert!(!path_matches_pattern("src/**", Path::new("other/src/lib.rs")).unwrap());
        assert!(path_matches_pattern("tests/*.rs", Path::new("tests/a.rs")).unwrap());
        assert!(!path_matches_pattern("tests/*.rs", Path::new("tests/unit/a.rs")).unwrap());
        assert!(path_matches_pattern("**/*.rs", Path::new("lib.rs")).unwrap());
        assert!(path_matches_pattern("src/?ain.rs", Path::new("src/main.rs")).unwrap());
        assert!(path_matches_pattern("docs/?.md", Path::new("docs/算.md")).unwrap());
        assert!(path_matches_pattern("/src/**", Path::new("src/lib.rs")).is_err());
        assert!(path_matches_pattern("../src/**", Path::new("src/lib.rs")).is_err());
        assert!(path_matches_pattern("src\\**", Path::new("src/lib.rs")).is_err());
    }

    #[test]
    fn safe_unknown_utf8_text_is_included_without_patterns() {
        let repo = TestDir::new("unknown-text");
        repo.write("design.custom", b"plain UTF-8 design language\n");

        let inventory = build_inventory(repo.path()).unwrap();
        let file = entry(&inventory, "design.custom");
        assert_eq!(file.kind, FileKind::Unknown);
        assert!(is_included(file));
        assert!(file.content_hash.is_some());
    }

    fn entry<'a>(inventory: &'a Inventory, path: &str) -> &'a FileEntry {
        inventory
            .files
            .iter()
            .find(|entry| entry.path == Path::new(path))
            .unwrap_or_else(|| panic!("missing inventory entry {path}"))
    }

    fn excluded_reason(entry: &FileEntry) -> &str {
        match &entry.disposition {
            FileDisposition::Exclude { reason } => reason,
            FileDisposition::Include => panic!("expected excluded entry {}", entry.path.display()),
        }
    }

    fn is_included(entry: &FileEntry) -> bool {
        matches!(&entry.disposition, FileDisposition::Include)
    }

    fn run_git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }
}
