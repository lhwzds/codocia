//! Agent notebook: a source-grounded store of notes that compiles into a
//! small, readable documentation set.
//!
//! Codocia never calls a model. An agent reads the repository, records what
//! it learned as notes with evidence, and Codocia pins that evidence, tracks
//! coverage and budgets, and compiles the notes into Markdown.

use crate::structure::{self, STRUCTURE_SLUG};
use crate::{current_commit, glob_matches, stable_hash_hex, write_file};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const BOOK_FILE: &str = "book.md";
const NOTES_DIR: &str = "notes";
const BUILD_MARKER: &str = ".codocia-build";
const MAX_TEXT_FILE_BYTES: u64 = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Model

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub slug: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Book {
    pub dir: PathBuf,
    pub title: String,
    pub repo: String,
    /// Optional output directory relative to the repository root.
    pub out: Option<String>,
    pub commit: String,
    pub remote: Option<String>,
    pub lang: String,
    pub max_pages: usize,
    pub max_page_words: usize,
    pub max_note_words: usize,
    /// Whole-book budget; 0 means no limit (books written before it existed).
    pub max_words: usize,
    pub pages: Vec<Page>,
    pub ignore: Vec<String>,
    /// Languages the book is translated into (`<book>/<lang>/`).
    pub langs: Vec<String>,
    /// Translated repository files: `target: source #hash`.
    pub translations: Vec<String>,
    pub intro: String,
    /// Set by `build` when the output lives inside the repository: the
    /// relative path from the output directory to the repository root, used
    /// for portable source links. Not stored in book.md.
    pub link_base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceRef {
    File {
        path: String,
        lines: Option<(usize, usize)>,
    },
    Commit(String),
    Pr(u64),
    Issue(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub target: EvidenceRef,
    pub pin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub page: String,
    pub id: String,
    pub title: String,
    pub order: Option<i64>,
    pub evidence: Vec<Evidence>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EvidenceState {
    Ok,
    Unpinned,
    Missing,
    BadRange,
    Stale,
    Moved(usize, usize),
    BadRef(String),
}

#[derive(Debug, Default)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Options

pub struct InitOptions {
    pub repo: PathBuf,
    pub book: Option<PathBuf>,
    pub title: Option<String>,
    pub lang: String,
}

pub struct NoteAddOptions {
    pub book: PathBuf,
    pub page: String,
    pub id: String,
    /// `None` keeps the existing title when replacing.
    pub title: Option<String>,
    pub order: Option<i64>,
    /// Empty keeps the existing evidence when replacing.
    pub evidence: Vec<String>,
    /// Appended to the existing (or given) evidence.
    pub add_evidence: Vec<String>,
    /// Empty keeps the existing body when replacing.
    pub body: String,
    pub replace: bool,
}

// ---------------------------------------------------------------------------
// Commands

/// Default repository is the Git worktree root, or the current directory.
pub fn default_repo() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&cwd)
        .output();
    match output {
        Ok(output) if output.status.success() => Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        )),
        _ => Ok(cwd),
    }
}

/// Resolve an explicit book, or discover the nearest in-repository or legacy book.
pub fn find_book(arg: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = arg {
        return Ok(path.to_path_buf());
    }
    find_book_from(&std::env::current_dir()?)
}

// Check the directory entry itself, not its target, before using an implicit book.
fn validate_implicit_book(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!(
                "{}: .codocia is a symlink; use --book to choose a book outside the repository",
                path.display()
            );
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn find_book_from(start: &Path) -> Result<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join(".codocia");
        validate_implicit_book(&candidate)?;
        if candidate.join(BOOK_FILE).is_file() {
            return Ok(candidate);
        }
        if dir.join(BOOK_FILE).is_file() && dir.join(NOTES_DIR).is_dir() {
            return Ok(dir.to_path_buf());
        }
    }
    bail!("no Codocia book found; run `codocia init` in the repository")
}

pub fn init(options: &InitOptions) -> Result<()> {
    let repo = options
        .repo
        .canonicalize()
        .with_context(|| format!("repository not found: {}", options.repo.display()))?;
    let commit = current_commit(&repo)?;
    let requested_book = options
        .book
        .clone()
        .unwrap_or_else(|| repo.join(".codocia"));
    if options.book.is_none() {
        validate_implicit_book(&requested_book)?;
    }
    fs::create_dir_all(&requested_book)
        .with_context(|| format!("create {}", requested_book.display()))?;
    let book_dir = requested_book.canonicalize()?;
    let book_file = book_dir.join(BOOK_FILE);
    if book_file.exists() {
        bail!("{} already exists", book_file.display());
    }
    let title = options.title.clone().unwrap_or_else(|| {
        repo.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Project".to_string())
    });
    let overview = if options.lang == "zh" {
        "概览"
    } else {
        "Overview"
    };
    let budget = default_page_budget(&repo);
    let book = Book {
        dir: book_dir.clone(),
        title,
        out: None,
        repo: match book_dir.strip_prefix(&repo) {
            Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
            Ok(relative) => vec![".."; relative.components().count()].join("/"),
            Err(_) => repo.to_string_lossy().to_string(),
        },
        commit,
        remote: git_remote_url(&repo),
        lang: options.lang.clone(),
        max_pages: budget.0,
        max_words: budget.1,
        max_page_words: 900,
        max_note_words: 180,
        pages: vec![Page {
            slug: "overview".to_string(),
            title: overview.to_string(),
        }],
        ignore: Vec::new(),
        langs: Vec::new(),
        translations: Vec::new(),
        intro: String::new(),
        link_base: None,
    };
    safe_target(&book_dir, &book_file)?;
    write_file(book_file, render_book(&book).as_bytes())?;
    fs::create_dir_all(book_dir.join(NOTES_DIR))?;
    println!(
        "created book at {} for {} @ {}",
        book_dir.display(),
        repo.display(),
        short(&book.commit)
    );
    println!("next: edit {BOOK_FILE} (pages, ignore), then run `codocia status`");
    Ok(())
}

/// Page budget scaled to the code: small projects get short books.
fn default_page_budget(repo: &Path) -> (usize, usize) {
    let lines = Inventory::scan(repo, &[])
        .map(|i| i.total_lines)
        .unwrap_or(0);
    match lines {
        0..20_000 => (4, 2500),
        20_000..200_000 => (6, 4000),
        _ => (8, 6000),
    }
}

pub fn status(book_dir: &Path, max_buckets: usize, area: Option<&str>) -> Result<()> {
    let book = load_book(book_dir)?;
    let notes = load_notes(&book)?;
    let repo = book.repo_path();
    let head = current_commit(&repo).unwrap_or_default();

    println!("book: {}", book.title);
    println!(
        "repo: {} @ {}{}",
        book.repo,
        short(&book.commit),
        if !code_moved(&repo, &book.commit, &head, &book.effective_ignore()) {
            String::new()
        } else {
            format!(" (repo HEAD is now {}; run `codocia check`)", short(&head))
        }
    );

    println!();
    println!(
        "pages: {}/{} (budget {} words/page, {} words/note; words = Latin words + CJK characters/2, code blocks excluded)",
        book.pages.len(),
        book.max_pages,
        book.max_page_words,
        book.max_note_words
    );
    let mut total_words = 0;
    for page in &book.pages {
        let page_notes: Vec<&Note> = notes.iter().filter(|n| n.page == page.slug).collect();
        let words: usize = page_notes.iter().map(|n| word_units(&n.body)).sum();
        total_words += words;
        let flag = if page_notes.is_empty() {
            "  EMPTY"
        } else if words > book.max_page_words {
            "  OVER BUDGET"
        } else {
            ""
        };
        println!(
            "  {:<22} {} {:>2} notes {:>5}/{} words{}",
            page.slug,
            pad_display(&page.title, 24),
            page_notes.len(),
            words,
            book.max_page_words,
            flag
        );
    }
    println!(
        "  total: {} notes, {} words{}",
        notes.len(),
        total_words,
        if book.max_words > 0 {
            format!(
                " / {} book budget{}",
                book.max_words,
                if total_words > book.max_words {
                    "  OVER BUDGET"
                } else {
                    ""
                }
            )
        } else {
            String::new()
        }
    );

    let inventory = Inventory::scan(&repo, &book.effective_ignore())?;
    let cited = cited_paths(&notes);
    let buckets = inventory.buckets(max_buckets);
    let covered_lines: usize = buckets
        .iter()
        .filter(|b| b.is_covered(&cited))
        .map(|b| b.lines)
        .sum();
    println!();
    println!(
        "coverage: {}% of {} lines in {} files ({} areas cited){}",
        percent(covered_lines, inventory.total_lines),
        inventory.total_lines,
        inventory.files.len(),
        buckets.iter().filter(|b| b.is_covered(&cited)).count(),
        if book.ignore.is_empty() {
            String::new()
        } else {
            format!("; ignored: {}", book.ignore.join(", "))
        }
    );
    for bucket in &buckets {
        println!(
            "  {} {:<52} {:>8} lines {:>5} files",
            if bucket.is_covered(&cited) {
                "✓"
            } else {
                "·"
            },
            bucket.label(),
            bucket.lines,
            bucket.files
        );
    }

    if let Some(area) = area {
        let prefix = area.trim_end_matches('/');
        let mut files: Vec<(&String, &usize)> = inventory
            .files
            .iter()
            .filter(|(path, _)| prefix.is_empty() || path.starts_with(&format!("{prefix}/")))
            .collect();
        files.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        println!();
        println!("files in {prefix}/ ({} files, largest first):", files.len());
        for (path, lines) in files.iter().take(40) {
            println!(
                "  {} {:<72} {:>7} lines",
                if cited.contains(*path) { "✓" } else { "·" },
                path,
                lines
            );
        }
    }

    let report = check_book(&book, &notes)?;
    println!();
    println!(
        "check: {} error(s), {} warning(s){}",
        report.errors.len(),
        report.warnings.len(),
        if report.errors.is_empty() && report.warnings.is_empty() {
            ""
        } else {
            " — run `codocia check` for details"
        }
    );
    Ok(())
}

pub fn note_add(options: &NoteAddOptions) -> Result<()> {
    let book = load_book(&options.book)?;
    validate_slug(&options.page, "page slug")?;
    if !book.pages.iter().any(|p| p.slug == options.page) {
        bail!(
            "unknown page `{}`; add it to `pages:` in {BOOK_FILE} first (known: {})",
            options.page,
            book.pages
                .iter()
                .map(|p| p.slug.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    validate_slug(&options.id, "note id")?;
    let path = note_path(&book, &options.page, &options.id);
    let existing = if path.exists() {
        if !options.replace {
            bail!(
                "note {}/{} already exists; pass --replace to update it (omitted fields are kept)",
                options.page,
                options.id
            );
        }
        let content = fs::read_to_string(&path)?;
        Some(parse_note(&options.page, &options.id, &content)?)
    } else {
        None
    };
    let repo = book.repo_path();
    let mut evidence = if options.evidence.is_empty() {
        existing
            .as_ref()
            .map(|note| note.evidence.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    {
        for raw in options.evidence.iter().chain(&options.add_evidence) {
            let mut item = parse_evidence(raw)?;
            item.pin = None;
            match evidence_state(&repo, &item) {
                EvidenceState::Ok | EvidenceState::Unpinned => {}
                EvidenceState::Missing => bail!("evidence file not found: {raw}"),
                EvidenceState::BadRange => {
                    let lines = match &item.target {
                        EvidenceRef::File { path, .. } => count_text_lines(&repo.join(path)),
                        _ => None,
                    };
                    bail!(
                        "evidence line range is outside the file: {raw}{}",
                        lines
                            .map(|n| format!(" (file has {n} lines)"))
                            .unwrap_or_default()
                    )
                }
                EvidenceState::BadRef(reason) => bail!("invalid evidence {raw}: {reason}"),
                EvidenceState::Stale | EvidenceState::Moved(..) => unreachable!(),
            }
            item.pin = pin_for(&repo, &item.target)?;
            if !evidence.iter().any(|e| e.target == item.target) {
                evidence.push(item);
            }
        }
    }
    if evidence.is_empty() {
        bail!(
            "a note needs at least one --evidence (path[:start-end], commit:<sha>, pr:<n>, issue:<n>)"
        );
    }
    let body = match (options.body.trim(), &existing) {
        ("", Some(note)) => note.body.clone(),
        ("", None) => bail!("note body is empty"),
        (body, _) => body.to_string(),
    };
    let note = Note {
        page: options.page.clone(),
        id: options.id.clone(),
        title: options
            .title
            .clone()
            .or_else(|| existing.as_ref().map(|n| n.title.clone()))
            .unwrap_or_default(),
        // New notes without --order go after the page's existing notes, so
        // reading order follows insertion order instead of id order.
        order: match (&existing, options.order) {
            (_, Some(order)) => Some(order),
            (Some(note), None) => note.order,
            (None, None) => Some(
                load_notes(&book)?
                    .iter()
                    .filter(|n| n.page == options.page)
                    .map(|n| n.order.unwrap_or(50))
                    .max()
                    .map_or(10, |max| max + 10),
            ),
        },
        evidence,
        body,
    };
    safe_target(&book.dir, &path)?;
    write_file(path.clone(), render_note(&note).as_bytes())?;
    let words = word_units(&note.body);
    let page_words: usize = load_notes(&book)?
        .iter()
        .filter(|n| n.page == note.page)
        .map(|n| word_units(&n.body))
        .sum();
    println!(
        "{} {}/{} ({} words{}; page {}/{})",
        if existing.is_some() {
            "updated"
        } else {
            "saved"
        },
        note.page,
        note.id,
        words,
        if words > book.max_note_words {
            format!(", over the {}-word note budget", book.max_note_words)
        } else {
            String::new()
        },
        page_words,
        book.max_page_words
    );
    for line in evidence_preview(&repo, &note.evidence) {
        println!("  {line}");
    }
    let tracked = tracked_files(&repo).unwrap_or_default();
    for warning in symbol_warnings(&repo, &tracked, &note) {
        println!("  warning: {warning}");
    }
    for link in page_links(&note.body) {
        if link != STRUCTURE_SLUG && !book.pages.iter().any(|p| p.slug == link) {
            println!("  warning: link [[{link}]] names no page");
        }
    }
    Ok(())
}

/// One line per evidence item: size and the first non-blank cited line, so
/// the agent can sanity-check that it cited what it meant to.
fn evidence_preview(repo: &Path, evidence: &[Evidence]) -> Vec<String> {
    let mut out = Vec::new();
    for item in evidence {
        let shown = render_evidence(&Evidence {
            target: item.target.clone(),
            pin: None,
        });
        if let EvidenceRef::Commit(sha) = &item.target {
            let subject = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["log", "-1", "--date=short", "--format=%ad %s", sha])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            out.push(format!("{shown} | {subject}"));
            continue;
        }
        let EvidenceRef::File { path, lines } = &item.target else {
            out.push(shown);
            continue;
        };
        let text = fs::read_to_string(repo.join(path)).unwrap_or_default();
        let all: Vec<&str> = text.lines().collect();
        let (start, end) = lines.unwrap_or((1, all.len().max(1)));
        let first = all
            .get(start.saturating_sub(1)..end.min(all.len()))
            .unwrap_or(&[])
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .unwrap_or("");
        let first: String = first.chars().take(70).collect();
        let span = end + 1 - start;
        let wide = if span > 120 {
            "  (wide: cite the lines that prove the claim)"
        } else {
            ""
        };
        out.push(format!("{shown} ({span} lines) | {first}{wide}"));
    }
    out
}

/// Backticked identifiers in the body that appear in none of the cited
/// files, and backticked paths that do not exist. Cheap, deterministic hints
/// for misnamed symbols; not proof of correctness.
fn symbol_warnings(repo: &Path, tracked: &[String], note: &Note) -> Vec<String> {
    let files: Vec<String> = note
        .evidence
        .iter()
        .filter_map(|e| match &e.target {
            EvidenceRef::File { path, .. } => fs::read_to_string(repo.join(path)).ok(),
            _ => None,
        })
        .collect();
    let has_files = note
        .evidence
        .iter()
        .any(|e| matches!(e.target, EvidenceRef::File { .. }));
    let mut warnings = Vec::new();
    let mut seen = BTreeSet::new();
    for span in code_spans(&note.body) {
        if !seen.insert(span.clone()) {
            continue;
        }
        if span.contains('/') {
            let path = span.trim_end_matches('/');
            // Only a directory (`a/b/`) or a file name with an extension
            // (`a/b.py`) is checked; `sgl.system/user` is a list of symbols.
            let last = path.rsplit('/').next().unwrap_or(path);
            let named_like_file = span.ends_with('/')
                || last.rsplit_once('.').is_some_and(|(stem, ext)| {
                    !stem.is_empty()
                        && (1..=5).contains(&ext.len())
                        && ext.chars().all(|c| c.is_ascii_alphanumeric())
                });
            let looks_like_path = named_like_file
                && !span.contains(' ')
                && !span.starts_with("http")
                && !span.contains(['*', '{', '<']);
            if looks_like_path && !path.is_empty() && !repo.join(path).exists() {
                // Accept shortened paths such as `managers/scheduler.py`.
                let file_suffix = format!("/{path}");
                let dir_part = format!("/{path}/");
                let found = tracked.iter().any(|f| {
                    f.ends_with(&file_suffix)
                        || f.contains(&dir_part)
                        || f.starts_with(&format!("{path}/"))
                });
                if !found {
                    warnings.push(format!("path `{span}` does not exist in the repository"));
                }
            }
            continue;
        }
        if !has_files || !is_identifier(&span) {
            continue;
        }
        // Fully qualified module paths (`a.b.c`) are not symbols to look up.
        if span.matches('.').count() >= 2 && !span.chars().any(|c| c.is_ascii_uppercase()) {
            continue;
        }
        let symbol = span
            .trim_end_matches("()")
            .rsplit('.')
            .next()
            .unwrap_or(&span)
            .to_string();
        if symbol.len() < 3 {
            continue;
        }
        if !files.iter().any(|f| f.contains(&symbol)) {
            warnings.push(format!("`{span}` appears in none of the cited files"));
        }
    }
    warnings
}

pub(crate) fn code_spans(body: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut in_fence = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut parts = line.split('`');
        parts.next();
        while let Some(code) = parts.next() {
            if parts.next().is_none() && !line.ends_with('`') {
                break;
            }
            spans.push(code.to_string());
        }
    }
    spans
}

fn is_identifier(span: &str) -> bool {
    let core = span.trim_end_matches("()");
    let mut chars = core.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && core
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        && core
            .chars()
            .any(|c| c == '_' || c.is_ascii_uppercase() || c == '.')
}

/// Prints one rendered page to stdout, whatever the state of other pages.
pub fn show(book_dir: &Path, page: &str) -> Result<()> {
    let book = load_book(book_dir)?;
    if page != STRUCTURE_SLUG && !book.pages.iter().any(|p| p.slug == page) {
        bail!("unknown page `{page}`");
    }
    let notes = load_notes(&book)?;
    let file = page_file(&book, page);
    let map = if page == STRUCTURE_SLUG {
        Some(structure::build_map(&book, &notes, 30)?)
    } else {
        None
    };
    let (_, content) = render_site(&book, &notes, map.as_ref())
        .into_iter()
        .find(|(name, _)| *name == file)
        .context("page not rendered")?;
    print!("{content}");
    Ok(())
}

pub fn note_list(book_dir: &Path, page: Option<&str>) -> Result<()> {
    let book = load_book(book_dir)?;
    for note in load_notes(&book)? {
        if page.is_some_and(|p| p != note.page) {
            continue;
        }
        println!(
            "{}/{:<28} order {:>3} {:>4} words  {}  ({})",
            note.page,
            note.id,
            note.order.unwrap_or(50),
            word_units(&note.body),
            if note.title.is_empty() {
                "(intro)"
            } else {
                &note.title
            },
            note_path(&book, &note.page, &note.id).display()
        );
    }
    Ok(())
}

/// Deletes a note named `page/id`.
pub fn note_rm(book_dir: &Path, name: &str) -> Result<()> {
    let book = load_book(book_dir)?;
    let (page, id) = name.split_once('/').context("name the note as page/id")?;
    validate_slug(page, "page slug")?;
    validate_slug(id, "note id")?;
    let path = note_path(&book, page, id);
    safe_target(&book.dir, &path)?;
    if !path.is_file() {
        bail!("note {name} not found");
    }
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    println!("removed {name}");
    Ok(())
}

/// Pins unpinned evidence and relocates evidence whose content moved.
/// `force` re-pins the named notes (`page/id` or `id`) after the agent has
/// reviewed stale evidence.
pub fn pin(book_dir: &Path, force: &[String]) -> Result<()> {
    let book = load_book(book_dir)?;
    let repo = book.repo_path();
    let renames = git_renames(&repo, &book.commit);
    let mut changed = 0;
    for mut note in load_notes(&book)? {
        let forced = force
            .iter()
            .any(|f| *f == note.id || *f == format!("{}/{}", note.page, note.id));
        let mut dirty = false;
        for item in &mut note.evidence {
            // A file renamed since the pinned commit: follow it, then check
            // the cited lines at the new path as usual.
            if let EvidenceRef::File { path, .. } = &mut item.target
                && !repo.join(path.as_str()).exists()
                && let Some(new_path) = renames.get(path.as_str())
            {
                println!("  {path} -> {new_path}");
                *path = new_path.clone();
                dirty = true;
            }
            let state = evidence_state(&repo, item);
            match state {
                EvidenceState::Unpinned => {
                    item.pin = pin_for(&repo, &item.target)?;
                    dirty = true;
                }
                EvidenceState::Moved(start, end) => {
                    if let EvidenceRef::File { lines, .. } = &mut item.target {
                        *lines = Some((start, end));
                    }
                    dirty = true;
                }
                EvidenceState::Stale if forced => {
                    item.pin = pin_for(&repo, &item.target)?;
                    dirty = true;
                }
                _ => {}
            }
        }
        if dirty {
            safe_target(&book.dir, &note_path(&book, &note.page, &note.id))?;
            write_file(
                note_path(&book, &note.page, &note.id),
                render_note(&note).as_bytes(),
            )?;
            println!("updated {}/{}", note.page, note.id);
            changed += 1;
        }
    }
    let head = current_commit(&repo)?;
    if code_moved(&repo, &book.commit, &head, &book.effective_ignore()) && force.is_empty() {
        println!(
            "repo HEAD {} differs from the pinned commit {}",
            short(&head),
            short(&book.commit)
        );
    }
    println!("updated {changed} note(s)");
    Ok(())
}

/// Follow unchanged citations and pin new ones before checking or reviewing.
fn follow(book: &Book) -> Result<usize> {
    let repo = book.repo_path();
    let renames = git_renames(&repo, &book.commit);
    let mut moved = 0;
    for mut note in load_notes(book)? {
        let mut dirty = false;
        for item in &mut note.evidence {
            let mut item_moved = false;
            if let EvidenceRef::File { path, .. } = &mut item.target
                && !repo.join(path.as_str()).exists()
                && let Some(new_path) = renames.get(path.as_str())
            {
                *path = new_path.clone();
                dirty = true;
                item_moved = true;
            }
            match evidence_state(&repo, item) {
                EvidenceState::Unpinned => {
                    item.pin = pin_for(&repo, &item.target)?;
                    dirty = true;
                }
                EvidenceState::Moved(start, end) => {
                    if let EvidenceRef::File { lines, .. } = &mut item.target {
                        *lines = Some((start, end));
                    }
                    dirty = true;
                    item_moved = true;
                }
                _ => {}
            }
            moved += usize::from(item_moved);
        }
        if dirty {
            safe_target(&book.dir, &note_path(book, &note.page, &note.id))?;
            write_file(
                note_path(book, &note.page, &note.id),
                render_note(&note).as_bytes(),
            )?;
        }
    }
    Ok(moved)
}

fn follow_and_report(book: &Book) -> Result<()> {
    let moved = follow(book)?;
    if moved > 0 {
        println!("followed {moved} moved citation(s)");
    }
    Ok(())
}

/// Record review of note evidence or translations.
pub fn accept(book_dir: &Path, targets: &[String], force: bool) -> Result<()> {
    if targets.is_empty() {
        bail!(
            "specify a note (page/id or unique id), language, language/book, language/page/id, or translation file"
        );
    }
    let book = load_book(book_dir)?;
    follow_and_report(&book)?;
    let notes = load_notes(&book)?;
    let repo = book.repo_path();
    let renames = git_renames(&repo, &book.commit);
    let mut selected = BTreeSet::new();
    let mut translations = Vec::new();
    for target in targets {
        if crate::translate::is_target(&book, &notes, target)? {
            translations.push(target.clone());
            continue;
        }
        if let Some((page, id)) = target.split_once('/') {
            validate_slug(page, "page slug")?;
            validate_slug(id, "note id")?;
        }
        let matches: Vec<_> = notes
            .iter()
            .enumerate()
            .filter(|(_, n)| target == &format!("{}/{}", n.page, n.id) || target == &n.id)
            .map(|(i, _)| i)
            .collect();
        match matches.as_slice() {
            [i] => {
                selected.insert(*i);
            }
            [] => bail!(
                "unknown target `{target}`; use page/id, a unique id, language, language/book, language/page/id, or a translation file from translations:"
            ),
            _ => bail!("ambiguous note id `{target}`; use page/id"),
        }
    }
    let mut refused = false;
    for i in selected {
        let mut note = notes[i].clone();
        let name = format!("{}/{}", note.page, note.id);
        let mut bad = false;
        let mut dirty = false;
        for item in &mut note.evidence {
            match evidence_state(&repo, item) {
                EvidenceState::Stale => {}
                EvidenceState::Missing | EvidenceState::BadRange | EvidenceState::BadRef(_) => {
                    println!(
                        "{name}: cannot accept invalid evidence {}; fix it in the note first",
                        render_evidence(item)
                    );
                    bad = true;
                    continue;
                }
                _ => continue,
            }
            if let EvidenceRef::File { path, lines } = &item.target {
                let old_path = renames
                    .iter()
                    .find_map(|(old, new)| (new == path).then_some(old.as_str()))
                    .unwrap_or(path);
                let old = git_file_at(&repo, &book.commit, old_path);
                let new =
                    fs::read_to_string(repo.join(path)).with_context(|| format!("read {path}"))?;
                let old_lines = old
                    .as_deref()
                    .and_then(|text| pinned_lines(text, *lines, item.pin.as_deref()));
                let new_lines = cited_lines(&new, *lines);
                println!("{name}: {}", render_evidence(item));
                print_excerpt("before", old_lines.as_deref());
                print_excerpt("after", Some(&new_lines));
                if !force && old_lines.is_none() {
                    println!(
                        "{name}: {} cannot be compared with its pinned version; check the range, then accept --force",
                        render_evidence(&Evidence {
                            target: item.target.clone(),
                            pin: None
                        })
                    );
                    bad = true;
                } else if !force && let Some(old_lines) = &old_lines {
                    let old_nonblank: Vec<_> = old_lines
                        .iter()
                        .map(|s| s.trim())
                        .filter(|s| s.chars().any(char::is_alphanumeric))
                        .collect();
                    let mut new_counts = BTreeMap::new();
                    for line in &new_lines {
                        *new_counts.entry(line.trim()).or_insert(0usize) += 1;
                    }
                    let matches = old_nonblank
                        .iter()
                        .filter(|line| {
                            if let Some(count) = new_counts.get_mut(**line)
                                && *count > 0
                            {
                                *count -= 1;
                                true
                            } else {
                                false
                            }
                        })
                        .count();
                    if if old_nonblank.is_empty() {
                        old_lines.join("\n").trim() != new_lines.join("\n").trim()
                    } else {
                        matches * 10 < old_nonblank.len() * 3
                    } {
                        println!(
                            "{name}: {} now covers different code; fix the range in the note, then accept again (or pass --force)",
                            render_evidence(&Evidence {
                                target: item.target.clone(),
                                pin: None
                            })
                        );
                        bad = true;
                    }
                }
            }
            item.pin = pin_for(&repo, &item.target)?;
            dirty = true;
        }
        if bad {
            refused = true;
        } else if dirty {
            safe_target(&book.dir, &note_path(&book, &note.page, &note.id))?;
            write_file(
                note_path(&book, &note.page, &note.id),
                render_note(&note).as_bytes(),
            )?;
            println!("accepted {name}");
        }
    }
    if !translations.is_empty() {
        crate::translate::sync(book_dir, &translations)?;
    }
    if refused {
        bail!("some notes were not accepted");
    }
    Ok(())
}

fn git_file_at(repo: &Path, commit: &str, path: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{commit}:{path}")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn cited_lines(text: &str, range: Option<(usize, usize)>) -> Vec<&str> {
    let lines: Vec<_> = text.lines().collect();
    match range {
        Some((a, b)) if a <= lines.len() && b <= lines.len() => lines[a - 1..b].to_vec(),
        Some(_) => Vec::new(),
        None => lines,
    }
}

fn pinned_lines<'a>(
    text: &'a str,
    range: Option<(usize, usize)>,
    pin: Option<&str>,
) -> Option<Vec<&'a str>> {
    let pin = pin?;
    let all: Vec<_> = text.lines().collect();
    match range {
        Some((a, b)) => {
            let len = b.checked_sub(a)?.checked_add(1)?;
            if len > all.len() {
                return None;
            }
            all.windows(len)
                .find(|window| hash_lines(window) == pin)
                .map(|window| window.to_vec())
        }
        None => (short_hash(text.as_bytes()) == pin).then_some(all),
    }
}

fn print_excerpt(label: &str, lines: Option<&[&str]>) {
    let Some(lines) = lines else {
        println!("  {label}: (unavailable)");
        return;
    };
    println!("  {label}:");
    for line in lines.iter().take(12) {
        println!("    {line}");
    }
    if lines.len() > 12 {
        println!("    ...");
    }
}

/// Updates the pinned commit once every note is current again.
pub fn rebase(book_dir: &Path) -> Result<()> {
    let mut book = load_book(book_dir)?;
    let notes = load_notes(&book)?;
    let report = check_book(&book, &notes)?;
    if !report.errors.is_empty() {
        print_report(&report);
        bail!("resolve errors before moving the book to the new commit");
    }
    move_book_commit(&mut book)?;
    Ok(())
}

fn move_book_commit(book: &mut Book) -> Result<()> {
    let repo = book.repo_path();
    let mut ignore = book.effective_ignore();
    ignore.extend(crate::translate::paths(book));
    let mut target = current_commit(&repo)?;
    while let Some(parent) = git_parent(&repo, &target) {
        if code_moved(&repo, &parent, &target, &ignore) {
            break;
        }
        target = parent;
    }
    if book.commit != target {
        book.commit = target;
        safe_target(&book.dir, &book.dir.join(BOOK_FILE))?;
        write_file(book.dir.join(BOOK_FILE), render_book(book).as_bytes())?;
        println!("book pinned to {}", short(&book.commit));
    }
    Ok(())
}

/// Validates the book; with `page`, reports only that page's problems.
pub fn check(book_dir: &Path, page: Option<&str>) -> Result<()> {
    if let Some(page) = page {
        validate_slug(page, "page slug")?;
    }
    let book = load_book(book_dir)?;
    follow_and_report(&book)?;
    let notes = load_notes(&book)?;
    let mut report = check_book(&book, &notes)?;
    if let Some(page) = page {
        let own = |m: &String| {
            m.starts_with(&format!("{page}/"))
                || m.contains(&format!("page `{page}`"))
                || book
                    .langs
                    .iter()
                    .any(|lang| m.starts_with(&format!("{lang}/{page}/")))
        };
        report.errors.retain(own);
        report.warnings.retain(own);
    }
    print_report(&report);
    if !report.errors.is_empty() {
        bail!("check failed with {} error(s)", report.errors.len());
    }
    Ok(())
}

// Validate implicit output paths before following citations or making any writes.
fn validate_implicit_output(anchor: &Path, out: &Path, location: &str) -> Result<()> {
    let refuse = || {
        anyhow::anyhow!(
            "{}: output directory resolves outside the {location}; use --out to choose an external directory",
            out.display()
        )
    };
    let relative = out.strip_prefix(anchor).map_err(|_| refuse())?;
    let mut current = anchor.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => current.push(part),
            Component::CurDir => continue,
            _ => return Err(refuse()),
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !current.canonicalize()?.starts_with(anchor)
                {
                    return Err(refuse());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn build(
    book_dir: &Path,
    out: Option<PathBuf>,
    force: bool,
    skill_page: Option<&str>,
) -> Result<()> {
    let book = load_book(book_dir)?;
    // The page is checked before anything is written.
    if let Some(page) = skill_page
        && !book.pages.iter().any(|p| p.slug == page)
    {
        bail!("--skill-page: no page \"{page}\" in {BOOK_FILE}");
    }
    let repo = book.repo_path().canonicalize()?;
    let implicit = out.is_none();
    let repository_output = book.out.is_some() || book.dir == repo.join(".codocia");
    let out = out
        .or_else(|| book.out.as_ref().map(|path| repo.join(path)))
        .unwrap_or_else(|| {
            if book.dir == repo.join(".codocia") {
                repo.join("docs")
            } else {
                book.dir.join("out")
            }
        });
    if implicit {
        let (anchor, location) = if repository_output {
            (&repo, "repository")
        } else {
            (&book.dir, "book")
        };
        validate_implicit_output(anchor, &out, location)?;
    }
    follow_and_report(&book)?;
    let notes = load_notes(&book)?;
    let report = check_book(&book, &notes)?;
    if !report.errors.is_empty() {
        print_report(&report);
        if !force {
            bail!("check failed; fix the errors or pass --force");
        }
    }
    let mut book = book;
    move_book_commit(&mut book)?;
    fs::create_dir_all(&out).with_context(|| format!("create {}", out.display()))?;
    let out = out.canonicalize()?;
    // Inside the repository: link sources relatively and name the index
    // README.md so the folder renders on GitHub and in any checkout.
    let in_repo = out.starts_with(&repo);
    if let Ok(rel) = out.strip_prefix(&repo) {
        book.link_base = Some("../".repeat(rel.components().count()));
    }
    let map = structure::build_map(&book, &notes, 30)?;
    let mut files = render_site(&book, &notes, Some(&map));
    if in_repo {
        for (name, _) in &mut files {
            if name == "index.md" {
                *name = "README.md".to_string();
            }
        }
    }
    crate::translate::add_languages(&book, &notes, &map, &mut files, in_repo)?;
    if let Some(page) = skill_page {
        let (name, content) = render_skill(&book, &notes, page)?;
        files.push((name, content));
    }
    let names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    prepare_output(&out, &names)?;
    for (name, content) in &files {
        write_file(out.join(name), content.as_bytes())?;
    }
    write_file(
        out.join(BUILD_MARKER),
        format!("{MANIFEST_HEADER}\n{}\n", names.join("\n")).as_bytes(),
    )?;
    let words: usize = notes.iter().map(|n| word_units(&n.body)).sum();
    println!(
        "built {} page(s), {} notes, {} words into {}",
        book.pages.len(),
        notes.len(),
        words,
        out.display()
    );
    Ok(())
}

/// Prints history signals: design-shaping commits from the full log, and hot
/// files plus the widest commits from the most recent `window` commits.
pub fn history(book_dir: &Path, limit: usize, window: usize) -> Result<()> {
    let book = load_book(book_dir)?;
    let repo = book.repo_path();
    let all = git_log(&repo, None)?;
    if all.is_empty() {
        println!("no commits");
        return Ok(());
    }
    let ignore = book.effective_ignore();
    let ignored = |path: &str| ignore.iter().any(|g| glob_matches(g, Path::new(path)));
    println!(
        "{} commits from {} to {}",
        all.len(),
        all.last().map(|c| c.date.as_str()).unwrap_or(""),
        all.first().map(|c| c.date.as_str()).unwrap_or("")
    );

    const KEYWORDS: &[&str] = &[
        "refactor",
        "redesign",
        "rewrite",
        "revert",
        "deprecate",
        "remove ",
        "migrate",
        "breaking",
        "introduce",
        "initial",
        "架构",
        "重构",
    ];
    // Spread design-shaping commits over the years instead of listing only
    // the latest weeks of a busy project.
    let shaping: Vec<&GitCommit> = all
        .iter()
        .rev()
        .filter(|c| {
            let subject = c.subject.to_lowercase();
            KEYWORDS.iter().any(|k| subject.contains(k))
        })
        .collect();
    let mut by_year: BTreeMap<&str, Vec<&GitCommit>> = BTreeMap::new();
    for commit in &shaping {
        by_year
            .entry(commit.date.get(..4).unwrap_or(""))
            .or_default()
            .push(commit);
    }
    let quota = limit.div_ceil(by_year.len().max(1)).max(1);
    println!("\ndesign-shaping commits, spread by year (oldest first):");
    for (year, commits) in &by_year {
        let step = (commits.len() / quota).max(1);
        println!("  {year} ({} matching):", commits.len());
        for commit in commits.iter().step_by(step).take(quota) {
            println!(
                "    commit:{} {}  {}",
                commit.hash, commit.date, commit.subject
            );
        }
    }

    let releases = release_timeline(&repo);
    if !releases.is_empty() {
        println!("\nrelease timeline (first tag of each minor version):");
        for (tag, date) in &releases {
            println!("  {date}  {tag}");
        }
    }

    let recent = git_log(&repo, Some(window))?;
    let mut hot: BTreeMap<&str, usize> = BTreeMap::new();
    for commit in &recent {
        for path in &commit.files {
            if !ignored(path) {
                *hot.entry(path.as_str()).or_default() += 1;
            }
        }
    }
    let mut hot: Vec<_> = hot.into_iter().collect();
    hot.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    println!(
        "\nmost changed files in the last {} commits (where the design keeps moving):",
        recent.len()
    );
    for (path, count) in hot.iter().take(limit) {
        println!("  {count:>5}  {path}");
    }

    let mut wide: Vec<(usize, &GitCommit)> = recent
        .iter()
        .map(|c| (c.files.iter().filter(|p| !ignored(p)).count(), c))
        .collect();
    wide.sort_by_key(|item| std::cmp::Reverse(item.0));
    println!(
        "\nwidest commits in the last {} commits (structural changes):",
        recent.len()
    );
    for (files, commit) in wide.iter().take(limit) {
        println!(
            "  commit:{} {} {:>5} files  {}",
            commit.hash, commit.date, files, commit.subject
        );
    }
    println!(
        "\ncite with --evidence commit:<hash>; inspect with `git -C <repo> show --stat <hash>`"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Loading and rendering book.md / notes

impl Book {
    /// Ignore the book itself as well as the user-configured patterns.
    pub(crate) fn effective_ignore(&self) -> Vec<String> {
        let mut ignore = self.ignore.clone();
        if let Ok(repo) = self.repo_path().canonicalize()
            && let Ok(relative) = self.dir.strip_prefix(repo)
            && !relative.as_os_str().is_empty()
        {
            ignore.push(format!(
                "{}/**",
                relative.to_string_lossy().replace('\\', "/")
            ));
        }
        ignore
    }

    pub fn repo_path(&self) -> PathBuf {
        let path = PathBuf::from(&self.repo);
        if path.is_absolute() {
            path
        } else {
            self.dir.join(path)
        }
    }

    pub(crate) fn label(&self, en: &'static str) -> &'static str {
        if self.lang != "zh" {
            return en;
        }
        match en {
            "Sources" => "来源",
            "Contents" => "目录",
            "Generated from" => "生成自",
            "All pages" => "全书单页",
            "Code structure" => "代码结构",
            "Appendix" => "附录",
            _ => en,
        }
    }
}

pub fn load_book(dir: &Path) -> Result<Book> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("book directory not found: {}", dir.display()))?;
    let path = dir.join(BOOK_FILE);
    let content = fs::read_to_string(&path).with_context(|| {
        format!(
            "{} not found; run `codocia init --repo <path> --book {}`",
            path.display(),
            dir.display()
        )
    })?;
    let (front, body) =
        split_front(&content).with_context(|| format!("{} has no frontmatter", path.display()))?;
    let front = Front::parse(front);
    let number = |key: &str, default: usize| -> Result<usize> {
        match front.scalar(key) {
            Some(value) => value
                .parse()
                .with_context(|| format!("{BOOK_FILE}: `{key}` must be a number")),
            None => Ok(default),
        }
    };
    let mut pages = Vec::new();
    for item in front.list("pages") {
        let (slug, title) = split_page_entry(item);
        validate_slug(&slug, "page slug")?;
        if slug == STRUCTURE_SLUG {
            bail!(
                "{BOOK_FILE}: `{STRUCTURE_SLUG}` is reserved for the generated code structure page"
            );
        }
        if pages.iter().any(|p: &Page| p.slug == slug) {
            bail!("{BOOK_FILE}: duplicate page `{slug}`");
        }
        pages.push(Page { slug, title });
    }
    let out = front.scalar("out").map(unquote);
    if let Some(ref value) = out {
        let path = Path::new(value);
        if path.is_absolute()
            || path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            bail!("{BOOK_FILE}: `out` must be a repository-relative path without `..`");
        }
    }
    // The primary language is read back the way it is written, so `langs`
    // cannot slip past the check by quoting it.
    let lang = front
        .scalar("lang")
        .map(unquote)
        .unwrap_or_else(|| "en".to_string());
    let langs = crate::translate::parse_langs(front.list("langs"), Some(&lang))?;
    Ok(Book {
        out,
        title: front
            .scalar_value("title")
            .map(|value| unquote(&value))
            .unwrap_or_else(|| "Documentation".to_string()),
        repo: front
            .scalar("repo")
            .map(unquote)
            .with_context(|| format!("{BOOK_FILE}: `repo` is required"))?,
        commit: front.scalar("commit").map(unquote).unwrap_or_default(),
        remote: front
            .scalar("remote")
            .map(unquote)
            .filter(|r| !r.is_empty()),
        lang,
        max_pages: number("max_pages", 8)?,
        max_page_words: number("max_page_words", 900)?,
        max_note_words: number("max_note_words", 180)?,
        max_words: number("max_words", 0)?,
        pages,
        ignore: front
            .list("ignore")
            .iter()
            .map(|raw| quoted_item(raw).unwrap_or_else(|| raw.trim().to_string()))
            .collect(),
        langs,
        translations: front
            .list("translations")
            .iter()
            .map(|raw| quoted_item(raw).unwrap_or_else(|| raw.trim().to_string()))
            .collect(),
        intro: body.trim().to_string(),
        link_base: None,
        dir,
    })
}

pub(crate) fn render_book(book: &Book) -> String {
    let mut out = String::from("---\n");
    // 0.2.0 reads these fields without unquoting, so they are written the
    // way 0.2.0 wrote them.
    out.push_str(&plain_line("title", &book.title));
    out.push_str(&plain_line("repo", &book.repo));
    if let Some(path) = &book.out {
        out.push_str(&scalar_line("out", path));
    }
    out.push_str(&plain_line("commit", &book.commit));
    if let Some(remote) = &book.remote {
        out.push_str(&plain_line("remote", remote));
    }
    out.push_str(&plain_line("lang", &book.lang));
    out.push_str(&format!("max_pages: {}\n", book.max_pages));
    out.push_str(&format!("max_page_words: {}\n", book.max_page_words));
    out.push_str(&format!("max_note_words: {}\n", book.max_note_words));
    if book.max_words > 0 {
        out.push_str(&format!("max_words: {}\n", book.max_words));
    }
    out.push_str("pages:\n");
    for page in &book.pages {
        out.push_str(&page_line(&page.slug, &page.title));
    }
    out.push_str("ignore:\n");
    for glob in &book.ignore {
        out.push_str(&item_line(glob));
    }
    if !book.langs.is_empty() {
        out.push_str("langs:\n");
        for lang in &book.langs {
            out.push_str(&item_line(lang));
        }
    }
    if !book.translations.is_empty() {
        out.push_str("translations:\n");
        for item in &book.translations {
            // The entry is written as it stands: rewriting it would drop
            // what the entry says beyond `target: source #pin`, and 0.2.0
            // reads the entry without unquoting it.
            out.push_str(&plain_item(item));
        }
    }
    out.push_str("---\n\n");
    if !book.intro.is_empty() {
        out.push_str(&book.intro);
        out.push('\n');
    }
    out
}

pub fn load_notes(book: &Book) -> Result<Vec<Note>> {
    let root = book.dir.join(NOTES_DIR);
    let mut notes = Vec::new();
    if !root.is_dir() {
        return Ok(notes);
    }
    let mut page_dirs: Vec<_> = fs::read_dir(&root)?.collect::<Result<_, _>>()?;
    page_dirs.sort_by_key(|e| e.file_name());
    for page_dir in page_dirs {
        if !page_dir.file_type()?.is_dir() {
            continue;
        }
        let page = page_dir.file_name().to_string_lossy().to_string();
        let mut files: Vec<_> = fs::read_dir(page_dir.path())?.collect::<Result<_, _>>()?;
        files.sort_by_key(|e| e.file_name());
        for file in files {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let content =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            notes.push(
                parse_note(&page, &id, &content)
                    .with_context(|| format!("parse {}", path.display()))?,
            );
        }
    }
    let order: BTreeMap<&str, usize> = book
        .pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.slug.as_str(), i))
        .collect();
    notes.sort_by(|a, b| {
        order
            .get(a.page.as_str())
            .unwrap_or(&usize::MAX)
            .cmp(order.get(b.page.as_str()).unwrap_or(&usize::MAX))
            .then(a.order.unwrap_or(50).cmp(&b.order.unwrap_or(50)))
            .then(a.id.cmp(&b.id))
    });
    Ok(notes)
}

fn parse_note(page: &str, id: &str, content: &str) -> Result<Note> {
    let (front, body) = split_front(content).context("note has no frontmatter")?;
    let front = Front::parse(front);
    let order = match front.scalar("order") {
        Some(value) => Some(value.parse().context("`order` must be an integer")?),
        None => None,
    };
    let evidence = front
        .list("evidence")
        .iter()
        .map(|raw| parse_evidence(&quoted_item(raw).unwrap_or_else(|| raw.trim().to_string())))
        .collect::<Result<Vec<_>>>()?;
    Ok(Note {
        page: page.to_string(),
        id: id.to_string(),
        title: front
            .scalar_value("title")
            .map(|value| unquote(&value))
            .unwrap_or_default(),
        order,
        evidence,
        body: body.trim().to_string(),
    })
}

fn render_note(note: &Note) -> String {
    let mut out = String::from("---\n");
    out.push_str(&title_line(&note.title));
    if let Some(order) = note.order {
        out.push_str(&format!("order: {order}\n"));
    }
    out.push_str("evidence:\n");
    for item in &note.evidence {
        // 0.2.0 reads the evidence entry without unquoting it.
        out.push_str(&plain_item(&render_evidence(item)));
    }
    out.push_str("---\n\n");
    out.push_str(note.body.trim());
    out.push('\n');
    out
}

/// Refuse writes through symlinks or directories escaping the book.
pub(crate) fn safe_target(root: &Path, path: &Path) -> Result<()> {
    let refuse = || {
        anyhow::anyhow!(
            "{}: refusing to write a note outside the book (symlink or escaping path)",
            path.display()
        )
    };
    let root = root.canonicalize()?;
    let relative = path.strip_prefix(&root).map_err(|_| refuse())?;
    if !relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(refuse());
    }
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(refuse());
    }
    let mut parent = path.parent().ok_or_else(refuse)?;
    loop {
        match fs::symlink_metadata(parent) {
            Ok(_) => {
                if !parent
                    .canonicalize()
                    .map_err(|_| refuse())?
                    .starts_with(&root)
                {
                    return Err(refuse());
                }
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                parent = parent.parent().ok_or_else(refuse)?;
            }
            Err(_) => return Err(refuse()),
        }
    }
    Ok(())
}

fn note_path(book: &Book, page: &str, id: &str) -> PathBuf {
    book.dir.join(NOTES_DIR).join(page).join(format!("{id}.md"))
}

// ---------------------------------------------------------------------------
// Minimal frontmatter: `key: value` scalars and `key:` + `  - item` lists.

pub(crate) struct Front {
    scalars: BTreeMap<String, String>,
    lists: BTreeMap<String, Vec<String>>,
    /// Keys written with an empty value (`key:`), which YAML reads as null.
    empties: BTreeSet<String>,
}

impl Front {
    pub(crate) fn parse(text: &str) -> Self {
        let mut scalars = BTreeMap::new();
        let mut lists: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut empties = BTreeSet::new();
        let mut current: Option<String> = None;
        for line in text.lines() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            if let (Some(item), Some(key)) = (line.trim_start().strip_prefix("- "), &current) {
                lists
                    .entry(key.clone())
                    .or_default()
                    .push(item.trim().to_string());
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim();
                if value.is_empty() {
                    lists.entry(key.clone()).or_default();
                    empties.insert(key.clone());
                    current = Some(key);
                } else {
                    scalars.insert(key.clone(), value.to_string());
                    empties.remove(&key);
                    current = None;
                }
            }
        }
        Self {
            scalars,
            lists,
            empties,
        }
    }

    pub(crate) fn scalar(&self, key: &str) -> Option<&str> {
        self.scalars.get(key).map(String::as_str)
    }

    /// The value of `key`; a key written with an empty value (`key:`) reads
    /// back as an empty string, a missing key as `None`.
    pub(crate) fn scalar_value(&self, key: &str) -> Option<String> {
        match self.scalars.get(key) {
            Some(value) => Some(value.clone()),
            None if self.empties.contains(key) => Some(String::new()),
            None => None,
        }
    }

    pub(crate) fn list(&self, key: &str) -> &[String] {
        self.lists.get(key).map(Vec::as_slice).unwrap_or(&[])
    }
}

pub(crate) fn split_front(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n")?;
    if let Some(body) = rest.strip_prefix("---\n") {
        return Some(("", body));
    }
    let end = rest
        .find("\n---\n")
        .or_else(|| rest.strip_suffix("\n---").map(|s| s.len()))?;
    let body = rest.get(end + "\n---\n".len()..).unwrap_or("");
    Some((&rest[..end], body))
}

/// The value inside a fully quoted scalar, or `None` when it is not
/// quoted. Both quote styles are decoded: a single-quoted scalar doubles
/// every quote inside it, so `''` is one quote, and a double-quoted scalar
/// carries no escapes, so what stands between the quotes is the value.
pub(crate) fn quoted_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        // A doubled quote inside a quoted value is one quote.
        return Some(value[1..value.len() - 1].replace("''", "'"));
    }
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return Some(value[1..value.len() - 1].to_string());
    }
    None
}

/// The value inside a fully quoted scalar, when the whole value is one
/// quoted YAML scalar. A single-quoted scalar doubles every quote inside it,
/// so `'a': 'b'` is two quoted parts and not one quoted entry.
pub(crate) fn quoted_item(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        let inner = &value[1..value.len() - 1];
        if inner.replace("''", "").contains('\'') {
            return None;
        }
        return Some(inner.replace("''", "'"));
    }
    quoted_value(value)
}

pub(crate) fn unquote(value: &str) -> String {
    quoted_value(value).unwrap_or_else(|| value.trim().to_string())
}

// ---------------------------------------------------------------------------
// YAML-safe values

/// Quotes a value so YAML reads it back as the same plain string. Aliases
/// (`*glob`), comments (`#`), mappings (`key: value`) and the other YAML
/// indicators are not plain scalars, and GitHub then refuses to render the
/// frontmatter.
///
/// The quote style is the one codocia 0.2.0 reads back unchanged. 0.2.0
/// strips one pair of matching outer quotes and does not decode `''`, so:
///
/// - a value without a `"` and without a `\` is written in double quotes
///   (`"*it's*"`); 0.2.0 and the current reader both read it back as the
///   value itself,
/// - a value with a `"` or a `\`, and without a `'`, is written in single
///   quotes (`'*a"b*'`, `'Overview: \q'`); 0.2.0 reads it back as the value
///   itself too, and a single-quoted scalar has no escapes, so a `\` stays
///   a `\`,
/// - a value with a `'` as well is written in single quotes with the `'`
///   doubled (`'it''s a "b"'`); only the current reader reads that back
///   exactly, 0.2.0 keeps the doubled quote. That is the one form 0.2.0
///   cannot read back unchanged.
pub(crate) fn yaml_value(value: &str) -> String {
    if !needs_quotes(value) {
        return value.to_string();
    }
    // A double-quoted scalar has YAML escapes, so a `\` in the value would
    // change it (`\n` is a newline) or make it invalid (`\q`); a
    // single-quoted scalar carries no escapes.
    if !value.contains('"') && !value.contains('\\') {
        format!("\"{value}\"")
    } else if !value.contains('\'') {
        format!("'{value}'")
    } else {
        // The value has a `'` and a `"` or a `\`; the `'` is doubled, and
        // only the current reader decodes that.
        format!("'{}'", value.replace('\'', "''"))
    }
}

/// Whether a value has to be quoted to stay a plain YAML scalar.
fn needs_quotes(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if value.starts_with("- ") {
        return true;
    }
    if matches!(
        value.chars().next(),
        Some(first) if "*&!%@`|>?{}[],#'\"".contains(first)
    ) {
        return true;
    }
    // A `: ` inside the value starts a mapping and a ` #` starts a comment,
    // so YAML reads the value without its tail; a value ending with `:` is
    // a mapping key, not a scalar. Quoting keeps the whole value.
    value.contains(": ") || value.contains(" #") || value.ends_with(':')
}

/// One `key: value` line; an empty value is written as a bare `key:`, which
/// reads back as the empty string it was.
pub(crate) fn scalar_line(key: &str, value: &str) -> String {
    if value.is_empty() {
        return format!("{key}:\n");
    }
    format!("{key}: {}\n", yaml_value(value))
}

/// One `title: value` line; an empty title is written as a bare `title:`.
pub(crate) fn title_line(value: &str) -> String {
    if value.is_empty() {
        return "title:\n".to_string();
    }
    format!("title: {}\n", yaml_value(value))
}

/// One `  - value` list item; the whole item is one scalar, so it is quoted
/// as a whole when YAML needs it.
pub(crate) fn item_line(value: &str) -> String {
    format!("  - {}\n", yaml_value(value))
}

/// One `  - slug: title` page entry. The slug is validated, so only the
/// title can need quotes; the entry itself is never quoted as a whole.
pub(crate) fn page_line(slug: &str, title: &str) -> String {
    format!("  - {slug}: {}\n", yaml_value(title))
}

/// Splits a `slug: title` page entry. Books written before the quoting fix
/// quote the whole entry as one scalar; the current format quotes only the
/// title.
pub(crate) fn split_page_entry(raw: &str) -> (String, String) {
    match quoted_item(raw) {
        Some(entry) => {
            let (slug, title) = entry
                .split_once(':')
                .unwrap_or((entry.as_str(), entry.as_str()));
            (slug.trim().to_string(), title.trim().to_string())
        }
        None => {
            let (slug, title) = raw.split_once(':').unwrap_or((raw, raw));
            (slug.trim().to_string(), unquote(title))
        }
    }
}

// ---------------------------------------------------------------------------
// Values codocia 0.2.0 reads without unquoting
//
// The frontmatter codocia 0.2.0 reads as plain text, without unquoting:
//
// - `title`, `repo`, `commit`, `remote` and `lang` of book.md,
// - the entries of `translations:`,
// - the `evidence:` entries of a note,
// - the `source:` pin of a translation.
//
// A quoted value keeps its quotes in the value 0.2.0 sees (`title: '*Demo'`
// reads as `'*Demo'`, and the pin of `source: '#abc123'` reads as
// `'#abc123'`), so these values are written the way 0.2.0 wrote them:
// never quoted.

/// One `key: value` line whose value 0.2.0 reads without unquoting; the
/// value is written as 0.2.0 wrote it. An empty value is written as a bare
/// `key:`, which 0.2.0 reads as an empty value too.
pub(crate) fn plain_line(key: &str, value: &str) -> String {
    if value.is_empty() {
        return format!("{key}:\n");
    }
    format!("{key}: {value}\n")
}

/// One `  - value` list item whose value 0.2.0 reads without unquoting; the
/// item is written as 0.2.0 wrote it.
pub(crate) fn plain_item(value: &str) -> String {
    format!("  - {value}\n")
}

pub(crate) fn validate_slug(value: &str, what: &str) -> Result<()> {
    let ok = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        bail!("{what} `{value}` must use only a-z, 0-9, '-' or '_'");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Evidence

pub fn parse_evidence(raw: &str) -> Result<Evidence> {
    let raw = raw.trim();
    let (target, pin) = match raw.rsplit_once(" #") {
        Some((target, pin)) if !pin.is_empty() && pin.chars().all(|c| c.is_ascii_hexdigit()) => {
            (target.trim(), Some(pin.to_string()))
        }
        _ => (raw, None),
    };
    let number = |value: &str, what: &str| -> Result<u64> {
        value
            .trim_start_matches('#')
            .parse()
            .with_context(|| format!("`{raw}`: {what} must be a number"))
    };
    let target = if let Some(sha) = target.strip_prefix("commit:") {
        if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("`{raw}`: commit must be a hex sha of at least 7 characters");
        }
        EvidenceRef::Commit(sha.to_string())
    } else if let Some(n) = target.strip_prefix("pr:") {
        EvidenceRef::Pr(number(n, "pr")?)
    } else if let Some(n) = target.strip_prefix("issue:") {
        EvidenceRef::Issue(number(n, "issue")?)
    } else {
        let (path, lines) = match target.rsplit_once(':') {
            Some((path, range)) if range.chars().all(|c| c.is_ascii_digit() || c == '-') => {
                let (start, end) = match range.split_once('-') {
                    Some((a, b)) => (a, b),
                    None => (range, range),
                };
                let start: usize = start
                    .parse()
                    .with_context(|| format!("`{raw}`: bad line range"))?;
                let end: usize = end
                    .parse()
                    .with_context(|| format!("`{raw}`: bad line range"))?;
                if start == 0 || end < start {
                    bail!("`{raw}`: line range must be 1-based and ascending");
                }
                (path, Some((start, end)))
            }
            _ => (target, None),
        };
        let path = path.trim_start_matches("./");
        if path.is_empty() || path.starts_with('/') || path.split('/').any(|s| s == "..") {
            bail!("`{raw}`: path must be relative to the repository root");
        }
        EvidenceRef::File {
            path: path.to_string(),
            lines,
        }
    };
    Ok(Evidence { target, pin })
}

fn render_evidence(item: &Evidence) -> String {
    let target = match &item.target {
        EvidenceRef::File { path, lines: None } => path.clone(),
        EvidenceRef::File {
            path,
            lines: Some((a, b)),
        } if a == b => format!("{path}:{a}"),
        EvidenceRef::File {
            path,
            lines: Some((a, b)),
        } => format!("{path}:{a}-{b}"),
        EvidenceRef::Commit(sha) => format!("commit:{sha}"),
        EvidenceRef::Pr(n) => format!("pr:{n}"),
        EvidenceRef::Issue(n) => format!("issue:{n}"),
    };
    match &item.pin {
        Some(pin) => format!("{target} #{pin}"),
        None => target,
    }
}

fn pin_for(repo: &Path, target: &EvidenceRef) -> Result<Option<String>> {
    let EvidenceRef::File { path, lines } = target else {
        return Ok(None);
    };
    let bytes = fs::read(repo.join(path)).with_context(|| format!("read {path}"))?;
    Ok(match lines {
        None => Some(short_hash(&bytes)),
        Some((a, b)) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            if *b > lines.len() {
                bail!("{path}: line range {a}-{b} is outside the file");
            }
            Some(hash_lines(&lines[a - 1..*b]))
        }
    })
}

fn evidence_state(repo: &Path, item: &Evidence) -> EvidenceState {
    match &item.target {
        EvidenceRef::Commit(sha) => {
            if !is_git(repo) {
                return EvidenceState::BadRef("not a git repository".to_string());
            }
            let ok = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !ok {
                return EvidenceState::BadRef("commit not found in repository".to_string());
            }
            // A rewritten commit can still exist locally but 404 on the host.
            let git = |args: &[&str]| {
                Command::new("git")
                    .arg("-C")
                    .arg(repo)
                    .args(args)
                    .output()
                    .ok()
            };
            let on_head = git(&["merge-base", "--is-ancestor", sha, "HEAD"])
                .map(|o| o.status.success())
                .unwrap_or(false);
            // Links point at origin when it exists, so only its branches count.
            let has_origin = git(&["remote", "get-url", "origin"])
                .map(|o| o.status.success())
                .unwrap_or(false);
            let pattern = if has_origin { "origin/*" } else { "*" };
            let on_remote = git(&["branch", "-r", "--contains", sha, "--list", pattern])
                .map(|o| o.status.success() && !o.stdout.trim_ascii().is_empty())
                .unwrap_or(false);
            if on_head || on_remote {
                EvidenceState::Ok
            } else {
                EvidenceState::BadRef(
                    "commit not on HEAD or an origin branch (rewritten?)".to_string(),
                )
            }
        }
        EvidenceRef::Pr(_) | EvidenceRef::Issue(_) => EvidenceState::Ok,
        EvidenceRef::File { path, lines } => {
            let Ok(bytes) = fs::read(repo.join(path)) else {
                return EvidenceState::Missing;
            };
            let Some(pin) = &item.pin else {
                if let Some((_, b)) = lines
                    && *b > String::from_utf8_lossy(&bytes).lines().count()
                {
                    return EvidenceState::BadRange;
                }
                return EvidenceState::Unpinned;
            };
            let Some((a, b)) = lines else {
                return if short_hash(&bytes) == *pin {
                    EvidenceState::Ok
                } else {
                    EvidenceState::Stale
                };
            };
            let text = String::from_utf8_lossy(&bytes);
            let all: Vec<&str> = text.lines().collect();
            if *b <= all.len() && hash_lines(&all[a - 1..*b]) == *pin {
                return EvidenceState::Ok;
            }
            // Same content elsewhere in the file: the code moved, not changed.
            let len = b - a + 1;
            if len <= all.len() {
                let mut best: Option<usize> = None;
                for start in 0..=all.len() - len {
                    if hash_lines(&all[start..start + len]) == *pin {
                        let distance = start.abs_diff(a - 1);
                        if best.is_none_or(|s| distance < s.abs_diff(a - 1)) {
                            best = Some(start);
                        }
                    }
                }
                if let Some(start) = best {
                    return EvidenceState::Moved(start + 1, start + len);
                }
            }
            if *b > all.len() {
                EvidenceState::BadRange
            } else {
                EvidenceState::Stale
            }
        }
    }
}

fn hash_lines(lines: &[&str]) -> String {
    let joined = lines
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    short_hash(joined.as_bytes())
}

pub(crate) fn short_hash(bytes: &[u8]) -> String {
    stable_hash_hex(bytes)[..12].to_string()
}

fn cited_paths(notes: &[Note]) -> BTreeSet<String> {
    notes
        .iter()
        .flat_map(|n| &n.evidence)
        .filter_map(|e| match &e.target {
            EvidenceRef::File { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Check

pub fn check_book(book: &Book, notes: &[Note]) -> Result<Report> {
    let mut report = Report::default();
    let repo = book.repo_path();
    if !repo.is_dir() {
        report
            .errors
            .push(format!("repository not found: {}", repo.display()));
        return Ok(report);
    }
    let slugs: BTreeSet<&str> = book.pages.iter().map(|p| p.slug.as_str()).collect();
    let tracked = tracked_files(&repo).unwrap_or_default();
    let renames = git_renames(&repo, &book.commit);
    if book.pages.len() > book.max_pages {
        report.warnings.push(format!(
            "{} pages exceed the {}-page budget; merge pages",
            book.pages.len(),
            book.max_pages
        ));
    }
    for warning in repetition_warnings(notes) {
        report.warnings.push(warning);
    }
    let book_words: usize = notes.iter().map(|n| word_units(&n.body)).sum();
    if book.max_words > 0 && book_words > book.max_words {
        report.warnings.push(format!(
            "book has {book_words} words (budget {}); cut repetition or merge pages",
            book.max_words
        ));
    }
    for page in &book.pages {
        let page_notes: Vec<&Note> = notes.iter().filter(|n| n.page == page.slug).collect();
        if page_notes.is_empty() {
            report
                .warnings
                .push(format!("page `{}` has no notes yet", page.slug));
        }
        let words: usize = page_notes.iter().map(|n| word_units(&n.body)).sum();
        if words > book.max_page_words {
            report.warnings.push(format!(
                "page `{}` has {words} words (budget {}); cut or split",
                page.slug, book.max_page_words
            ));
        }
    }
    for note in notes {
        let name = format!("{}/{}", note.page, note.id);
        if !slugs.contains(note.page.as_str()) {
            report.errors.push(format!(
                "{name}: page `{}` is not listed in {BOOK_FILE}",
                note.page
            ));
        }
        if note.evidence.is_empty() {
            report.errors.push(format!("{name}: no evidence"));
        }
        if note.body.trim().is_empty() {
            report.errors.push(format!("{name}: empty body"));
        }
        let words = word_units(&note.body);
        if words > book.max_note_words {
            report.warnings.push(format!(
                "{name}: {words} words (budget {}); one note should answer one question",
                book.max_note_words
            ));
        }
        for warning in symbol_warnings(&repo, &tracked, note) {
            report.warnings.push(format!("{name}: {warning}"));
        }
        for link in page_links(&note.body) {
            if link != STRUCTURE_SLUG && !slugs.contains(link.as_str()) {
                report
                    .errors
                    .push(format!("{name}: link [[{link}]] names no page"));
            }
        }
        for item in &note.evidence {
            let shown = render_evidence(item);
            match evidence_state(&repo, item) {
                EvidenceState::Ok => {}
                EvidenceState::Unpinned => report
                    .errors
                    .push(format!("{name}: unpinned evidence {shown}; run `codocia check`")),
                EvidenceState::Missing => match &item.target {
                    EvidenceRef::File { path, .. } if renames.contains_key(path.as_str()) => {
                        report.errors.push(format!(
                            "{name}: {shown} was renamed to {}; run `codocia check`",
                            renames[path.as_str()]
                        ))
                    }
                    _ => report
                        .errors
                        .push(format!("{name}: evidence file no longer exists: {shown}")),
                },
                EvidenceState::BadRange => report
                    .errors
                    .push(format!("{name}: evidence range is outside the file: {shown}")),
                EvidenceState::Stale => report.errors.push(format!(
                    "{name}: code changed under {shown}; review the note, then `codocia accept {name}`"
                )),
                EvidenceState::Moved(a, b) => report.warnings.push(format!(
                    "{name}: {shown} moved to lines {a}-{b}; run `codocia check`"
                )),
                EvidenceState::BadRef(reason) => {
                    report.errors.push(format!("{name}: {shown}: {reason}"))
                }
            }
        }
    }
    crate::translate::check(book, notes, &mut report)?;
    Ok(report)
}

/// Notes on different pages that cite the same code twice or more (ranges
/// overlapping by at least half) usually explain the same mechanism twice.
/// One shared range is an ordinary cross-reference and is not reported.
fn repetition_warnings(notes: &[Note]) -> Vec<String> {
    let ranges: Vec<Vec<(&str, usize, usize)>> = notes
        .iter()
        .map(|n| {
            n.evidence
                .iter()
                .filter_map(|e| match &e.target {
                    EvidenceRef::File {
                        path,
                        lines: Some((a, b)),
                    } => Some((path.as_str(), *a, *b)),
                    _ => None,
                })
                .collect()
        })
        .collect();
    let mut out = Vec::new();
    for i in 0..notes.len() {
        for j in i + 1..notes.len() {
            if notes[i].page == notes[j].page {
                continue;
            }
            let mut shared = Vec::new();
            for &(f1, a1, b1) in &ranges[i] {
                for &(f2, a2, b2) in &ranges[j] {
                    let overlap = b1.min(b2) as isize - a1.max(a2) as isize + 1;
                    let shorter = (b1 - a1 + 1).min(b2 - a2 + 1) as isize;
                    if f1 == f2 && overlap > 0 && overlap * 2 >= shorter {
                        shared.push(format!("{f1}:{}-{}", a1.max(a2), b1.min(b2)));
                    }
                }
            }
            if shared.len() >= 2 {
                out.push(format!(
                    "{}/{} and {}/{} cite the same code ({}); explain it on one page and link from the other",
                    notes[i].page,
                    notes[i].id,
                    notes[j].page,
                    notes[j].id,
                    shared.join(", ")
                ));
            }
        }
    }
    out
}

fn print_report(report: &Report) {
    for error in &report.errors {
        println!("error: {error}");
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    println!(
        "{} error(s), {} warning(s)",
        report.errors.len(),
        report.warnings.len()
    );
}

/// Counts reading length: Latin words plus CJK characters / 2. Fenced code
/// blocks (including diagrams) are not counted.
pub fn word_units(text: &str) -> usize {
    let mut latin = 0;
    let mut cjk = 0;
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for token in line.split_whitespace() {
            if token.chars().any(|c| c.is_ascii_alphanumeric()) {
                latin += 1;
            }
            cjk += token.chars().filter(|c| is_cjk(*c)).count();
        }
    }
    latin + cjk.div_ceil(2)
}

/// Pads to a terminal width where CJK characters take two columns.
fn pad_display(text: &str, width: usize) -> String {
    let used: usize = text.chars().map(|c| if is_cjk(c) { 2 } else { 1 }).sum();
    format!("{text}{}", " ".repeat(width.saturating_sub(used)))
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

fn page_links(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        links.push(after[..end].trim().to_string());
        rest = &after[end + 2..];
    }
    links
}

// ---------------------------------------------------------------------------
// Repository inventory and coverage areas

/// Whether `repo` is a Git checkout. Plain directories are supported
/// without calling git at all: no history, no commit pin.
fn is_git(repo: &Path) -> bool {
    repo.join(".git").exists()
}

/// Files of a plain directory: everything except hidden entries (other than
/// .github), dependency and build folders, and symlinks.
fn walk_files(root: &Path) -> Result<Vec<String>> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                let skip = (name.starts_with('.') && name != ".github")
                    || matches!(
                        name.as_str(),
                        "node_modules" | "target" | "__pycache__" | "dist" | "build"
                    );
                if !skip {
                    visit(root, &entry.path(), out)?;
                }
            } else if kind.is_file() && !name.starts_with(".DS_Store") {
                let rel = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap_or(&entry.path())
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    visit(root, root, &mut out)?;
    Ok(out)
}

fn tracked_files(repo: &Path) -> Result<Vec<String>> {
    if !is_git(repo) {
        return walk_files(repo);
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .context("run git ls-files")?;
    if !output.status.success() {
        bail!("git ls-files failed in {}", repo.display());
    }
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).to_string())
        .collect())
}

pub(crate) struct Inventory {
    pub(crate) files: BTreeMap<String, usize>,
    pub(crate) total_lines: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketKind {
    /// Everything below `dir`.
    Whole,
    /// Only the files directly inside `dir`.
    Files,
    /// Everything below `dir` except the subdirectories listed separately.
    Rest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    /// Directory path ("" for the repository root).
    pub dir: String,
    pub kind: BucketKind,
    /// Subdirectories of `dir` that belong to other buckets.
    pub exclude: Vec<String>,
    pub lines: usize,
    pub files: usize,
}

impl Bucket {
    pub(crate) fn label(&self) -> String {
        match (self.dir.as_str(), self.kind) {
            ("", BucketKind::Files) => "(root files)".to_string(),
            ("", _) => "(other)".to_string(),
            (dir, BucketKind::Whole) => format!("{dir}/"),
            (dir, BucketKind::Files) => format!("{dir}/*"),
            (dir, BucketKind::Rest) => format!("{dir}/ (other)"),
        }
    }

    pub(crate) fn contains(&self, path: &str) -> bool {
        let inside = self.dir.is_empty() || path.starts_with(&format!("{}/", self.dir));
        inside
            && !self
                .exclude
                .iter()
                .any(|e| path.starts_with(&format!("{e}/")))
    }

    fn is_covered(&self, cited: &BTreeSet<String>) -> bool {
        cited.iter().any(|path| self.contains(path))
    }
}

impl Inventory {
    pub(crate) fn scan(repo: &Path, ignore: &[String]) -> Result<Self> {
        let mut files = BTreeMap::new();
        let mut total_lines = 0;
        for path in tracked_files(repo)? {
            if path.is_empty() || ignore.iter().any(|g| glob_matches(g, Path::new(&path))) {
                continue;
            }
            // Codocia's own output is not part of the code to explain.
            if Path::new(&path)
                .ancestors()
                .skip(1)
                .any(|dir| repo.join(dir).join(BUILD_MARKER).is_file())
            {
                continue;
            }
            if let Some(lines) = count_text_lines(&repo.join(&path)) {
                total_lines += lines;
                files.insert(path, lines);
            }
        }
        Ok(Self { files, total_lines })
    }

    /// Splits the repository into at most `max` areas by repeatedly splitting
    /// the largest directory, so big subsystems get their own row. When a
    /// directory has too many children, the small ones share an "(other)" row.
    pub(crate) fn buckets(&self, max: usize) -> Vec<Bucket> {
        let mut dir_lines: BTreeMap<String, usize> = BTreeMap::new();
        let mut children: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut has_files: BTreeSet<String> = BTreeSet::new();
        for (path, lines) in &self.files {
            let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
            has_files.insert(parent.to_string());
            let mut dir = parent.to_string();
            loop {
                *dir_lines.entry(dir.clone()).or_default() += lines;
                if dir.is_empty() {
                    break;
                }
                let up = dir
                    .rsplit_once('/')
                    .map(|(p, _)| p)
                    .unwrap_or("")
                    .to_string();
                children.entry(up.clone()).or_default().insert(dir.clone());
                dir = up;
            }
        }
        let bucket = |dir: &str, kind, exclude: Vec<String>| Bucket {
            dir: dir.to_string(),
            kind,
            exclude,
            lines: dir_lines.get(dir).copied().unwrap_or(0),
            files: 0,
        };
        // Replaces a whole directory with its largest children plus one
        // bucket for the remainder, using at most `slots` buckets.
        let split = |dir: &str, slots: usize| -> Option<Vec<Bucket>> {
            let mut kids: Vec<&String> = children.get(dir)?.iter().collect();
            kids.sort_by_key(|k| std::cmp::Reverse(dir_lines[*k]));
            let remainder = has_files.contains(dir);
            let keep = if kids.len() + usize::from(remainder) <= slots {
                kids.len()
            } else {
                slots.checked_sub(1)?
            };
            if keep == 0 {
                return None;
            }
            let kept: Vec<String> = kids[..keep].iter().map(|k| k.to_string()).collect();
            let mut out: Vec<Bucket> = kept
                .iter()
                .map(|k| bucket(k, BucketKind::Whole, Vec::new()))
                .collect();
            if keep < kids.len() {
                out.push(bucket(dir, BucketKind::Rest, kept));
            } else if remainder {
                out.push(bucket(dir, BucketKind::Files, kept));
            }
            Some(out)
        };
        let mut buckets = split("", max).unwrap_or_default();
        if buckets.is_empty() && !self.files.is_empty() {
            buckets.push(bucket("", BucketKind::Files, Vec::new()));
        }
        let floor = self.total_lines / 25;
        let mut stuck: BTreeSet<String> = BTreeSet::new();
        loop {
            let candidate = buckets
                .iter()
                .enumerate()
                .filter(|(_, b)| {
                    b.kind == BucketKind::Whole
                        && b.lines > floor
                        && children.contains_key(&b.dir)
                        && !stuck.contains(&b.dir)
                })
                .max_by_key(|(_, b)| b.lines)
                .map(|(i, _)| i);
            let Some(index) = candidate else { break };
            let slots = max + 1 - buckets.len();
            match split(&buckets[index].dir.clone(), slots) {
                Some(parts) if parts.len() > 1 => {
                    buckets.splice(index..=index, parts);
                }
                // A directory that cannot be usefully split stays whole.
                _ => {
                    stuck.insert(buckets[index].dir.clone());
                }
            }
        }
        for b in &mut buckets {
            let (lines, files) = self
                .files
                .iter()
                .filter(|(path, _)| b.contains(path))
                .fold((0, 0), |(l, f), (_, n)| (l + n, f + 1));
            b.lines = lines;
            b.files = files;
        }
        buckets.retain(|b| b.files > 0);
        buckets.sort_by(|a, b| b.lines.cmp(&a.lines).then(a.dir.cmp(&b.dir)));
        buckets
    }
}

fn count_text_lines(path: &Path) -> Option<usize> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_TEXT_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path).ok()?.read_to_end(&mut bytes).ok()?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return None;
    }
    let lines = bytes.iter().filter(|b| **b == b'\n').count();
    Some(if bytes.last().is_some_and(|b| *b != b'\n') {
        lines + 1
    } else {
        lines
    })
}

fn percent(part: usize, total: usize) -> usize {
    (part * 100).checked_div(total).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Git helpers

struct GitCommit {
    hash: String,
    date: String,
    subject: String,
    files: Vec<String>,
}

/// First tag of each `major.minor` line, oldest first.
fn release_timeline(repo: &Path) -> Vec<(String, String)> {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "for-each-ref",
            "refs/tags",
            "--sort=creatordate",
            "--format=%(refname:short)%09%(creatordate:short)",
        ])
        .output()
    else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((tag, date)) = line.split_once('\t') else {
            continue;
        };
        let version = tag.trim_start_matches(|c: char| !c.is_ascii_digit());
        let mut parts = version.split('.');
        let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
            continue;
        };
        let minor: String = minor.chars().take_while(char::is_ascii_digit).collect();
        if major.is_empty() || minor.is_empty() || !major.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if seen.insert(format!("{major}.{minor}")) {
            out.push((tag.to_string(), date.to_string()));
        }
    }
    out
}

/// Reads commit headers; with `window`, also the touched paths of the most
/// recent `window` commits (path lists are slow on large histories).
fn git_log(repo: &Path, window: Option<usize>) -> Result<Vec<GitCommit>> {
    if !is_git(repo) {
        return Ok(Vec::new());
    }
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args([
        "log",
        "--no-merges",
        "--date=short",
        "--format=@@%h%x09%ad%x09%s",
    ]);
    if let Some(window) = window {
        command.arg("--name-only").arg(format!("-n{window}"));
    }
    let output = command.output().context("run git log")?;
    if !output.status.success() {
        bail!("git log failed in {}", repo.display());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut commits: Vec<GitCommit> = Vec::new();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            let mut parts = header.splitn(3, '\t');
            commits.push(GitCommit {
                hash: parts.next().unwrap_or("").to_string(),
                date: parts.next().unwrap_or("").to_string(),
                subject: parts.next().unwrap_or("").to_string(),
                files: Vec::new(),
            });
        } else if let Some(commit) = commits.last_mut()
            && !line.is_empty()
        {
            commit.files.push(line.to_string());
        }
    }
    Ok(commits)
}

fn git_parent(repo: &Path, commit: &str) -> Option<String> {
    if !is_git(repo) {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "-q", &format!("{commit}^")])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// True when commits after the pinned one changed anything besides
/// Codocia's own generated docs (directories holding a build manifest), so
/// committing the built docs into the repository is not reported as drift.
fn code_moved(repo: &Path, commit: &str, head: &str, ignore: &[String]) -> bool {
    if head == commit || head.is_empty() || !is_git(repo) {
        return false;
    }
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--name-only", "-z", commit, head])
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .any(|p| {
            let path = String::from_utf8_lossy(p);
            let generated = Path::new(path.as_ref())
                .ancestors()
                .skip(1)
                .any(|dir| repo.join(dir).join(BUILD_MARKER).is_file());
            let out_of_scope = ignore
                .iter()
                .any(|g| glob_matches(g, Path::new(path.as_ref())));
            !generated && !out_of_scope
        })
}

/// Files renamed between the pinned commit and the working tree
/// (old path -> new path), as git's rename detection sees them.
fn git_renames(repo: &Path, commit: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if commit.is_empty() || !is_git(repo) {
        return out;
    }
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "-M", "--name-status", "-z", commit])
        .output()
    else {
        return out;
    };
    let fields: Vec<String> = output
        .stdout
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).to_string())
        .collect();
    let mut i = 0;
    while i < fields.len() {
        let status = &fields[i];
        if status.starts_with('R') && i + 2 < fields.len() {
            out.insert(fields[i + 1].clone(), fields[i + 2].clone());
            i += 3;
        } else if status.starts_with('C') {
            i += 3;
        } else {
            i += 2;
        }
    }
    out
}

fn git_remote_url(repo: &Path) -> Option<String> {
    if !is_git(repo) {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    web_url(String::from_utf8_lossy(&output.stdout).trim())
}

fn web_url(remote: &str) -> Option<String> {
    let url = if let Some(rest) = remote.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        format!("https://{host}/{path}")
    } else if remote.starts_with("https://") || remote.starts_with("http://") {
        remote.to_string()
    } else {
        return None;
    };
    Some(
        url.trim_end_matches(".git")
            .trim_end_matches('/')
            .to_string(),
    )
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

// ---------------------------------------------------------------------------
// Build output

const MANIFEST_HEADER: &str = "generated by codocia build; files:";

/// Makes room for `names` in `out`. Only files listed in the previous
/// build's manifest are removed or overwritten; any other file with a
/// generated name stops the build, and unrelated files are left alone, so
/// a repository's `docs/` can hold hand-written pages next to the book.
fn prepare_output(out: &Path, names: &[String]) -> Result<()> {
    let marker = out.join(BUILD_MARKER);
    let canonical_out = out.canonicalize()?;
    for name in names
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(BUILD_MARKER))
    {
        let relative = Path::new(name);
        let path = out.join(relative);
        let refuse = || {
            anyhow::anyhow!(
                "{}: refusing to write outside the output directory",
                path.display()
            )
        };
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(refuse());
        }
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(refuse());
        }
        let mut ancestor = out.to_path_buf();
        if let Some(parent) = relative.parent() {
            for component in parent.components() {
                ancestor.push(component);
                match fs::symlink_metadata(&ancestor) {
                    Ok(_)
                        if !ancestor
                            .canonicalize()
                            .map_err(|_| refuse())?
                            .starts_with(&canonical_out) =>
                    {
                        return Err(refuse());
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(refuse()),
                }
            }
        }
    }
    let previous: Vec<String> = match fs::read(&marker) {
        Ok(bytes)
            if std::str::from_utf8(&bytes)
                .ok()
                .and_then(|text| text.lines().next())
                == Some(MANIFEST_HEADER) =>
        {
            std::str::from_utf8(&bytes)
                .unwrap()
                .lines()
                .skip(1)
                .map(str::to_string)
                .collect()
        }
        Ok(_) => bail!(
            "{} is not a Codocia manifest; remove it or choose another --out",
            marker.display()
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(err).with_context(|| format!("read {}", marker.display())),
    };
    for name in names {
        if fs::symlink_metadata(out.join(name)).is_ok() && !previous.contains(name) {
            bail!(
                "{} already exists and was not generated by `codocia build`; move it or choose another --out",
                out.join(name).display()
            );
        }
    }
    for name in &previous {
        let relative = Path::new(name);
        let mut components = relative.components().peekable();
        let valid =
            components.peek().is_some() && components.all(|c| matches!(c, Component::Normal(_)));
        let path = out.join(relative);
        let parent_inside = path
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .is_some_and(|p| p.starts_with(&canonical_out));
        if !valid || !parent_inside {
            eprintln!("skipping manifest entry outside the output directory: {name}");
            continue;
        }
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && (metadata.is_file() || metadata.file_type().is_symlink())
        {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            // A skill file has a folder of its own: the empty folder goes
            // with it.
            remove_empty_parents(&path, out);
        }
    }
    Ok(())
}

/// Removes the empty folders above `path`, up to but not including `out`.
fn remove_empty_parents(path: &Path, out: &Path) {
    let mut current = path.parent().map(Path::to_path_buf);
    while let Some(folder) = current {
        if folder.as_path() == out || !folder.starts_with(out) {
            return;
        }
        match fs::read_dir(&folder) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return;
                }
            }
            Err(_) => return,
        }
        if fs::remove_dir(&folder).is_err() {
            return;
        }
        current = folder.parent().map(Path::to_path_buf);
    }
}

// Demote ATX headings without changing fenced code blocks.
fn demote_headings(text: &str) -> String {
    let mut fence: Option<(char, usize)> = None;
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start_matches(' ');
            let indent = line.len() - trimmed.len();
            if indent <= 3 {
                if let Some((ch, len)) = fence {
                    let count = trimmed.chars().take_while(|c| *c == ch).count();
                    if count >= len && trimmed[count..].trim().is_empty() {
                        fence = None;
                    }
                    return line.to_string();
                }
                if let Some(ch @ ('`' | '~')) = trimmed.chars().next() {
                    let count = trimmed.chars().take_while(|c| *c == ch).count();
                    if count >= 3 && (ch != '`' || !trimmed[count..].contains('`')) {
                        fence = Some((ch, count));
                        return line.to_string();
                    }
                }
            }
            if fence.is_none() && indent <= 3 {
                let count = trimmed.bytes().take_while(|b| *b == b'#').count();
                if (1..=6).contains(&count) && trimmed.as_bytes().get(count) == Some(&b' ') {
                    return format!("{}#{trimmed}", &line[..indent]);
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn page_file(book: &Book, slug: &str) -> String {
    if slug == STRUCTURE_SLUG {
        return format!("{STRUCTURE_SLUG}.md");
    }
    let index = book.pages.iter().position(|p| p.slug == slug).unwrap_or(0);
    format!("{:02}-{slug}.md", index + 1)
}

/// The body of a page: every note with its heading, its text and its source
/// line. Page links stay `[[slug]]` so the caller can resolve them.
fn page_notes_body(book: &Book, notes: &[Note], slug: &str) -> String {
    let mut body = String::new();
    for note in notes.iter().filter(|n| n.page == slug) {
        if !note.title.is_empty() {
            body.push_str(&format!("## {}\n\n", note.title));
        }
        body.push_str(note.body.trim());
        body.push_str("\n\n");
        let sources: Vec<String> = note
            .evidence
            .iter()
            .map(|e| evidence_link(book, &e.target))
            .collect();
        if !sources.is_empty() {
            const SHOWN: usize = 5;
            let mut line = sources
                .iter()
                .take(SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(" · ");
            if sources.len() > SHOWN {
                line.push_str(&format!(" · +{}", sources.len() - SHOWN));
            }
            body.push_str(&format!("<sub>{}: {line}</sub>\n\n", book.label("Sources")));
        }
    }
    body
}

/// Returns `(relative file name, content)` for every output file.
/// `structure` is the generated code-structure body, rendered as an appendix.
pub fn render_site(
    book: &Book,
    notes: &[Note],
    structure: Option<&structure::StructureMap>,
) -> Vec<(String, String)> {
    let structure_title = book.label("Code structure");
    let mut titles: BTreeMap<&str, &str> = book
        .pages
        .iter()
        .map(|p| (p.slug.as_str(), p.title.as_str()))
        .collect();
    titles.insert(STRUCTURE_SLUG, structure_title);
    let mut files = Vec::new();
    let mut all = format!(
        "---\ntitle: {} — {}\n---\n\n# {}\n\n",
        book.title,
        book.label("All pages"),
        book.title
    );
    if !book.intro.is_empty() {
        all.push_str(&format!("{}\n\n", book.intro));
    }

    let mut index = format!("---\ntitle: {}\n---\n\n# {}\n\n", book.title, book.title);
    if !book.intro.is_empty() {
        index.push_str(&format!("{}\n\n", book.intro));
    }
    index.push_str(&format!("## {}\n\n", book.label("Contents")));
    for (i, page) in book.pages.iter().enumerate() {
        let sections: Vec<&str> = notes
            .iter()
            .filter(|n| n.page == page.slug && !n.title.is_empty())
            .map(|n| n.title.as_str())
            .collect();
        index.push_str(&format!(
            "{}. [{}]({}){}\n",
            i + 1,
            page.title,
            page_file(book, &page.slug),
            if sections.is_empty() {
                String::new()
            } else {
                format!(" — {}", sections.join(" · "))
            }
        ));
    }
    if structure.is_some() {
        index.push_str(&format!(
            "\n## {}\n\n- [{}]({}) — {}\n",
            book.label("Appendix"),
            structure_title,
            page_file(book, STRUCTURE_SLUG),
            if book.lang == "zh" {
                "从代码生成：功能区、依赖、讲解页"
            } else {
                "generated from the code: areas, dependencies, pages"
            }
        ));
    }
    index.push_str(&format!(
        "\n<sub>{} {} @ `{}`</sub>\n",
        book.label("Generated from"),
        book.remote.clone().unwrap_or_else(|| {
            // Never publish a local absolute path; name the repository only.
            book.repo_path()
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&book.repo))
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        }),
        short(&book.commit)
    ));
    files.push(("index.md".to_string(), index));

    for page in &book.pages {
        let body = page_notes_body(book, notes, &page.slug);
        let page_body = resolve_links(&body, |slug| {
            format!(
                "[{}]({})",
                titles.get(slug).unwrap_or(&slug),
                page_file(book, slug)
            )
        });
        files.push((
            page_file(book, &page.slug),
            format!(
                "---\ntitle: {}\n---\n\n# {}\n\n{}",
                page.title,
                page.title,
                page_body.trim_end()
            ) + "\n",
        ));

        let all_body = resolve_links(&body, |slug| {
            format!("[{}](#{slug})", titles.get(slug).unwrap_or(&slug))
        });
        let demoted = demote_headings(&all_body);
        all.push_str(&format!(
            "<a id=\"{}\"></a>\n\n## {}\n\n{}\n\n",
            page.slug,
            page.title,
            demoted.trim_end()
        ));
    }
    if let Some(map) = structure {
        let page_body = structure::render(book, map, |slug| {
            format!(
                "[{}]({})",
                titles.get(slug).unwrap_or(&slug),
                page_file(book, slug)
            )
        });
        files.push((
            page_file(book, STRUCTURE_SLUG),
            format!("---\ntitle: {structure_title}\n---\n\n# {structure_title}\n\n{page_body}"),
        ));
        let all_body = demote_headings(&structure::render(book, map, |slug| {
            format!("[{}](#{slug})", titles.get(slug).unwrap_or(&slug))
        }));
        all.push_str(&format!(
            "<a id=\"{STRUCTURE_SLUG}\"></a>\n\n## {structure_title}\n\n{}\n\n",
            all_body.trim_end()
        ));
    }
    files.push(("all.md".to_string(), all.trim_end().to_string() + "\n"));
    files
}

// ---------------------------------------------------------------------------
// Skill file: SKILL.md for agent skill stores

/// The directory name of the skill: the repository folder, lowercased, with
/// every character outside `[a-z0-9._-]` replaced by `-` and the leading and
/// trailing `-` and `.` removed.
fn skill_id(book: &Book) -> String {
    let repo = book.repo_path();
    let canonical = repo.canonicalize().unwrap_or(repo);
    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut id = String::new();
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            id.push(ch);
        } else {
            id.push('-');
        }
    }
    let id = id.trim_matches(|c| c == '-' || c == '.').to_string();
    if id.is_empty() {
        "skill".to_string()
    } else {
        id
    }
}

/// The version of the repository: Cargo.toml, then pyproject.toml, then
/// package.json, then `0.1.0`. A version that would need quoting is
/// skipped: the frontmatter holds one plain value.
fn skill_version(book: &Book) -> String {
    let repo = book.repo_path();
    let cargo = repo.join("Cargo.toml");
    let sources = [
        read_toml_version(&cargo, "package"),
        read_toml_version(&cargo, "workspace.package"),
        read_toml_version(&repo.join("pyproject.toml"), "project"),
        read_package_json_version(&repo.join("package.json")),
    ];
    for version in sources.into_iter().flatten() {
        if safe_version(&version) {
            return version;
        }
    }
    "0.1.0".to_string()
}

/// A version that can be written as it is: not empty, and nothing but
/// letters, digits, `.`, `+`, `_` and `-`.
fn safe_version(version: &str) -> bool {
    !version.is_empty()
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '_' | '-'))
}

/// The `version = "x"` of one TOML section, found by scanning lines. Its only
/// job is the `version` string of that one section: anything the reader is not
/// sure about gives `None`, so `skill_version` tries the next source and then
/// `0.1.0`, never a version of another table. A line that holds a multi-line
/// string delimiter (`"""` or `'''`), anywhere in it, makes every line after it
/// unsure, so the reader gives up there; up to that line the file holds no
/// multi-line string, and the header and bracket logic below is exact.
fn read_toml_version(path: &Path, section: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut inside = false;
    // How deep the previous line of a value sits inside an unfinished
    // multi-line array or inline table. A stray closing bracket never takes it
    // below zero.
    let mut depth = 0;
    for line in text.lines() {
        let line = line.trim();
        // A multi-line string hides how many lines a value spans, so a version
        // found after one would be a guess.
        if line.contains("\"\"\"") || line.contains("'''") {
            return None;
        }
        if depth > 0 {
            // A line that continues a multi-line value is neither a header
            // nor a version.
            depth = toml_bracket_depth(line, depth).max(0);
            continue;
        }
        if line.starts_with('[') {
            // A header: an array table closes the current section and never
            // opens a matching one.
            inside = !line.starts_with("[[") && toml_section_name(line) == Some(section);
            continue;
        }
        depth = toml_bracket_depth(line, depth).max(0);
        if inside && let Some(value) = toml_string_value(line, "version") {
            return Some(value);
        }
    }
    None
}

/// The name of a TOML table header `[name]`: the text between the brackets,
/// trimmed, where any character is allowed, quotes included. A comment may
/// follow the header. `None` for any other line, so `[[name]]`, an array
/// table, is not a table header.
fn toml_section_name(line: &str) -> Option<&str> {
    let line = line.trim();
    let rest = line.strip_prefix('[')?;
    if rest.starts_with('[') {
        return None;
    }
    let end = outside_strings(rest).find_map(|(i, ch)| (ch == ']').then_some(i))?;
    let inner = rest[..end].trim();
    // Nothing but a comment may follow the closing bracket.
    let after = rest[end + 1..].trim();
    if !after.is_empty() && !after.starts_with('#') {
        return None;
    }
    (!inner.is_empty()).then_some(inner)
}

/// How deeply a line of a value sits inside an unfinished multi-line array or
/// inline table: every `[` or `{` outside a string opens a bracket and every
/// `]` or `}` outside a string closes one.
fn toml_bracket_depth(line: &str, depth: i32) -> i32 {
    outside_strings(line).fold(depth, |depth, (_, ch)| match ch {
        '[' | '{' => depth + 1,
        ']' | '}' => depth - 1,
        _ => depth,
    })
}

/// The characters of `line` that sit outside a `'...'` or `"..."` string,
/// with their positions. A `#` outside a string ends the line, as it starts
/// a comment.
fn outside_strings(line: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut chars = line.char_indices();
    // The quote character of the string we are inside, if any.
    let mut string: Option<char> = None;
    // Whether the next character of a `"..."` string is escaped.
    let mut escaped = false;
    std::iter::from_fn(move || {
        loop {
            let (i, ch) = chars.next()?;
            match string {
                Some(quote) => {
                    if escaped {
                        escaped = false;
                    } else if quote == '"' && ch == '\\' {
                        escaped = true;
                    } else if ch == quote {
                        string = None;
                    }
                }
                None => {
                    if ch == '#' {
                        return None;
                    }
                    if ch == '"' || ch == '\'' {
                        string = Some(ch);
                    } else {
                        return Some((i, ch));
                    }
                }
            }
        }
    })
}

/// The string value of `key = "value"` on one line.
fn toml_string_value(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

/// The top-level `"version"` of a package.json, read without a JSON parser.
fn read_package_json_version(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    json_top_level_string(&text, "version")
}

/// Reads a string member of the root object of a JSON document.
fn json_top_level_string(text: &str, key: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    let mut depth = 0usize;
    while index < chars.len() {
        match chars[index] {
            '"' => {
                let (name, next) = read_json_string(&chars, index);
                index = next;
                let mut peek = index;
                while peek < chars.len() && chars[peek].is_whitespace() {
                    peek += 1;
                }
                if depth != 1 || peek >= chars.len() || chars[peek] != ':' {
                    continue;
                }
                peek += 1;
                while peek < chars.len() && chars[peek].is_whitespace() {
                    peek += 1;
                }
                if name != key {
                    continue;
                }
                if peek < chars.len() && chars[peek] == '"' {
                    return Some(read_json_string(&chars, peek).0);
                }
                return None;
            }
            '{' | '[' => {
                depth += 1;
                index += 1;
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            _ => index += 1,
        }
    }
    None
}

/// Reads the JSON string that starts at the opening quote.
fn read_json_string(chars: &[char], start: usize) -> (String, usize) {
    let mut out = String::new();
    let mut index = start + 1;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\\' && index + 1 < chars.len() {
            let escaped = chars[index + 1];
            out.push(match escaped {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\u{8}',
                'f' => '\u{c}',
                other => other,
            });
            index += 2;
            continue;
        }
        if ch == '"' {
            return (out, index + 1);
        }
        out.push(ch);
        index += 1;
    }
    (out, index)
}

/// Makes a text safe to write as a frontmatter value: one line, no double
/// quotes, no run of backslashes, no quote at the start and no quote or
/// backslash at the end. The readers of a skill file are line parsers, not
/// YAML: a value that starts or ends with a quote, or that ends with a
/// backslash, would not survive them.
fn skill_scalar(text: &str) -> String {
    let mut flat = String::new();
    let mut space = false;
    for ch in text.chars() {
        let ch = if ch == '\n' || ch == '\r' { ' ' } else { ch };
        if ch.is_whitespace() {
            if !space {
                flat.push(' ');
                space = true;
            }
            continue;
        }
        space = false;
        flat.push(ch);
    }
    // A double quote becomes a single quote first: the readers treat both
    // the same way, and a value that ended with a double quote would end
    // with a single quote after the change, which the readers strip.
    let flat = flat.replace('"', "'");
    // One backslash only: a pair would be unescaped by one of the readers.
    let mut collapsed = String::new();
    let mut backslash = false;
    for ch in flat.chars() {
        if ch == '\\' {
            if !backslash {
                collapsed.push('\\');
            }
            backslash = true;
            continue;
        }
        backslash = false;
        collapsed.push(ch);
    }
    // A pair of outer quotes goes as one.
    let mut value = collapsed.trim().to_string();
    let chars: Vec<char> = value.chars().collect();
    if chars.len() >= 2 && chars[0] == '\'' && chars[chars.len() - 1] == '\'' {
        value = chars[1..chars.len() - 1].iter().collect();
    }
    // What is left may not start with a quote, may not end with a quote and
    // may not end with a backslash.
    loop {
        let before = value.clone();
        value = value
            .trim_start_matches(|c: char| c.is_whitespace() || c == '\'' || c == '"')
            .trim_end_matches(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\')
            .to_string();
        if value == before {
            break;
        }
    }
    value
}

/// The first sentence of the page's first note, for the skill frontmatter.
fn skill_description(book: &Book, notes: &[Note], page: &Page) -> String {
    let mut page_notes: Vec<&Note> = notes.iter().filter(|n| n.page == page.slug).collect();
    page_notes.sort_by_key(|n| (n.order.unwrap_or(50), n.id.clone()));
    let text = match page_notes.first() {
        Some(note) => {
            let body = drop_code_fences(&note.body);
            resolve_links(&body, |inner| skill_link_text(book, inner))
        }
        None => String::new(),
    };
    // Only the first paragraph is read: a sentence of a later paragraph
    // describes that paragraph, not the skill.
    let paragraph = first_paragraph(&text);
    let mut description = first_sentence(&paragraph).unwrap_or(paragraph);
    description = skill_scalar(&description);
    description = cut_at_word(&description, 300);
    // The cut may leave a backslash at the end.
    description = skill_scalar(&description);
    if description.is_empty() {
        description = skill_scalar(&format!("{} — {}", one_line_title(book), page.title));
    }
    description
}

/// Removes fenced code blocks from a text.
fn drop_code_fences(text: &str) -> String {
    let mut out = String::new();
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        if indent <= 3 {
            if let Some((ch, len)) = fence {
                let count = trimmed.chars().take_while(|c| *c == ch).count();
                let rest: String = trimmed.chars().skip(count).collect();
                if count >= len && rest.trim().is_empty() {
                    fence = None;
                }
                continue;
            }
            if let Some(ch @ ('`' | '~')) = trimmed.chars().next() {
                let count = trimmed.chars().take_while(|c| *c == ch).count();
                let rest: String = trimmed.chars().skip(count).collect();
                if count >= 3 && (ch != '`' || !rest.contains('`')) {
                    fence = Some((ch, count));
                    continue;
                }
            }
        }
        if fence.is_some() {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Collapses every run of whitespace into one space.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text up to and including the first `.`, `!` or `?` that ends a
/// sentence.
fn first_sentence(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    for (index, ch) in chars.iter().enumerate() {
        if matches!(ch, '.' | '!' | '?') {
            let next = chars.get(index + 1);
            if next.is_none() || next.is_some_and(|c| c.is_whitespace()) {
                return Some(chars[..=index].iter().collect());
            }
        }
    }
    None
}

/// The first paragraph of a text: everything before the first blank line.
/// Blank lines before the paragraph do not end it.
fn first_paragraph(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        lines.push(line);
    }
    collapse_whitespace(&lines.join("\n"))
}

/// Cuts a text to at most `limit` characters, at a word boundary. A text
/// without any space in it is cut hard.
fn cut_at_word(text: &str, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if end == 0 {
        // No word boundary: cut hard, so the text is not lost.
        return chars[..limit].iter().collect();
    }
    chars[..end]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The plain text of a `[[slug]]` or `[[slug|text]]` link.
fn skill_link_text(book: &Book, inner: &str) -> String {
    match inner.split_once('|') {
        Some((_slug, label)) => label.trim().to_string(),
        None => page_title(book, inner.trim()),
    }
}

/// The title of a page, or the slug when the page is unknown.
fn page_title(book: &Book, slug: &str) -> String {
    if let Some(page) = book.pages.iter().find(|p| p.slug == slug) {
        return page.title.clone();
    }
    if slug == STRUCTURE_SLUG {
        return book.label("Code structure").to_string();
    }
    slug.to_string()
}

/// The book title as one line.
fn one_line_title(book: &Book) -> String {
    book.title.replace(['\n', '\r'], " ").trim().to_string()
}

/// The `name` of the skill: the book title, quoted when it would not parse,
/// and the skill id when the title holds nothing but quotes.
fn skill_name(book: &Book) -> String {
    let name = skill_scalar(&book.title);
    let name = if name.is_empty() {
        skill_id(book)
    } else {
        name
    };
    if name.contains(": ")
        || name.contains(" #")
        || name.starts_with('#')
        || name.starts_with(['[', '{', '-', '*', '&', '!', '|', '>', '%', '@', '`'])
        || name.ends_with(':')
    {
        // A quoted name holds no quote of its own: the readers would strip
        // it and read another name.
        format!("\"{name}\"")
    } else {
        name
    }
}

/// The commit of a permalink in a skill file: a long hex hash is cut to 12
/// characters, anything else stays as it is.
fn skill_commit(commit: &str) -> String {
    if commit.chars().count() > 12 && commit.chars().all(|c| c.is_ascii_hexdigit()) {
        commit.chars().take(12).collect()
    } else {
        commit.to_string()
    }
}

/// Renders `<out>/skill/<id>/SKILL.md`: one page as a skill for agents.
fn render_skill(book: &Book, notes: &[Note], slug: &str) -> Result<(String, String)> {
    let page = book
        .pages
        .iter()
        .find(|p| p.slug == slug)
        .with_context(|| format!("--skill-page: no page \"{slug}\" in {BOOK_FILE}"))?;
    let id = skill_id(book);
    let title = one_line_title(book);
    let description = skill_description(book, notes, page);
    let version = skill_version(book);
    // A skill file is copied into a skill store, so it must not carry
    // relative links: sources become permalinks and page links plain text.
    let mut plain = book.clone();
    plain.link_base = None;
    // A skill store hides a line that holds a full 40-hex commit, as it looks
    // like a leaked token, so the permalinks carry a short commit.
    plain.commit = skill_commit(&plain.commit);
    // The same hiding applies to commit evidence: a note that cites a commit
    // cites its short form in the skill file.
    let mut cited = notes.to_vec();
    for note in &mut cited {
        for evidence in &mut note.evidence {
            if let EvidenceRef::Commit(sha) = &mut evidence.target {
                let short = skill_commit(sha);
                *sha = short;
            }
        }
    }
    let body = page_notes_body(&plain, &cited, slug);
    let body = resolve_links(&body, |inner| skill_link_text(&plain, inner));
    let mut content = format!(
        "---\nname: {}\ndescription: \"{}\"\nversion: {}\ntags: [{id}]\n---\n\n# {title}\n",
        skill_name(book),
        description,
        version
    );
    if !body.trim().is_empty() {
        content.push_str(&format!("\n{}\n", body.trim_end()));
    }
    Ok((format!("skill/{id}/SKILL.md"), content))
}

/// Prints the generated code structure to stdout.
pub fn map(book_dir: &Path, max_areas: usize) -> Result<()> {
    let book = load_book(book_dir)?;
    let notes = load_notes(&book)?;
    let map = structure::build_map(&book, &notes, max_areas)?;
    print!(
        "{}",
        structure::render(&book, &map, |slug| format!("[[{slug}]]"))
    );
    Ok(())
}

fn resolve_links(body: &str, link: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        out.push_str(&rest[..start]);
        out.push_str(&link(after[..end].trim()));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn evidence_link(book: &Book, target: &EvidenceRef) -> String {
    let github = book.remote.as_deref().filter(|r| r.contains("github.com"));
    match target {
        EvidenceRef::File { path, lines } => {
            let name = path.rsplit('/').next().unwrap_or(path);
            let text = match lines {
                Some((a, b)) if a == b => format!("{name}:{a}"),
                Some((a, b)) => format!("{name}:{a}-{b}"),
                None => name.to_string(),
            };
            let anchor = match lines {
                Some((a, b)) if a == b => format!("#L{a}"),
                Some((a, b)) => format!("#L{a}-L{b}"),
                None => String::new(),
            };
            if let Some(base) = &book.link_base {
                return format!("[{text}]({base}{path}{anchor})");
            }
            match &book.remote {
                Some(remote) => {
                    let anchor = match lines {
                        Some((a, b)) if a == b => format!("#L{a}"),
                        Some((a, b)) => format!("#L{a}-L{b}"),
                        None => String::new(),
                    };
                    format!(
                        "[{text}]({remote}/blob/{}/{path}{anchor} \"{path}\")",
                        book.commit
                    )
                }
                None => format!("`{path}{}`", text.strip_prefix(name).unwrap_or("")),
            }
        }
        EvidenceRef::Commit(sha) => match &book.remote {
            Some(remote) => format!("[commit {}]({remote}/commit/{sha})", short(sha)),
            None => format!("commit `{}`", short(sha)),
        },
        EvidenceRef::Pr(n) => match github {
            Some(remote) => format!("[PR #{n}]({remote}/pull/{n})"),
            None => format!("PR #{n}"),
        },
        EvidenceRef::Issue(n) => match github {
            Some(remote) => format!("[issue #{n}]({remote}/issues/{n})"),
            None => format!("issue #{n}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "codocia-book-{name}-{}-{nanos}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?} failed");
    }

    fn fixture() -> (PathBuf, PathBuf) {
        let root = temp_dir("fixture");
        let repo = root.join("repo");
        fs::create_dir_all(repo.join("src/core")).unwrap();
        fs::write(
            repo.join("src/core/engine.rs"),
            "fn a() {}\nfn run() {\n    step();\n}\nfn z() {}\n",
        )
        .unwrap();
        fs::write(repo.join("README.md"), "# Demo\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "initial engine",
            ],
        );
        let book = root.join("book");
        init(&InitOptions {
            repo: repo.clone(),
            book: Some(book.clone()),
            title: Some("Demo".to_string()),
            lang: "en".to_string(),
        })
        .unwrap();
        (repo, book)
    }

    #[cfg(unix)]
    #[test]
    fn implicit_init_refuses_symlinked_book() {
        use std::os::unix::fs::symlink;
        let (repo, _) = fixture();
        let outside = repo.parent().unwrap().join("outside");
        fs::create_dir(&outside).unwrap();
        let implicit = repo.canonicalize().unwrap().join(".codocia");
        symlink(&outside, &implicit).unwrap();
        let error = init(&InitOptions {
            repo: repo.clone(),
            book: None,
            title: None,
            lang: "en".into(),
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "{}: .codocia is a symlink; use --book to choose a book outside the repository",
                implicit.display()
            )
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        // An explicit external book remains supported even if .codocia is a symlink.
        init(&InitOptions {
            repo,
            book: Some(outside.clone()),
            title: None,
            lang: "en".into(),
        })
        .unwrap();
        assert!(outside.join(BOOK_FILE).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn discovery_refuses_symlinked_book_before_check_or_remove() {
        use std::os::unix::fs::symlink;
        let (repo, outside) = fixture();
        let implicit = repo.join(".codocia");
        symlink(&outside, &implicit).unwrap();
        let note = outside.join("notes/overview/victim.md");
        fs::create_dir_all(note.parent().unwrap()).unwrap();
        let content = "---\ntitle: Victim\nevidence:\n  - src/core/engine.rs:2-4\n---\n\nA note.\n";
        fs::write(&note, content).unwrap();
        let expected = format!(
            "{}: .codocia is a symlink; use --book to choose a book outside the repository",
            implicit.display()
        );
        let discover = || find_book_from(&repo.join("src/core"));
        let error = discover().and_then(|book| check(&book, None)).unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(fs::read_to_string(&note).unwrap(), content);
        let error = discover()
            .and_then(|book| note_rm(&book, "overview/victim"))
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(fs::read_to_string(&note).unwrap(), content);
        let explicit = find_book(Some(&outside)).unwrap();
        check(&explicit, None).unwrap();
        assert_ne!(fs::read_to_string(&note).unwrap(), content);
        note_rm(&explicit, "overview/victim").unwrap();
        assert!(!note.exists());
    }

    #[test]
    fn note_rm_rejects_traversal() {
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        let victim = repo.join("victim.md");
        fs::write(&victim, "keep").unwrap();
        assert!(note_rm(&book, "../../victim").is_err());
        assert_eq!(fs::read_to_string(victim).unwrap(), "keep");
        assert!(check(&book, Some("../bad")).is_err());
        assert!(accept(&book, &["../victim".into()], false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn note_writes_and_deletes_reject_symlinks() {
        use std::os::unix::fs::symlink;
        let (_, book) = fixture();
        let outside = temp_dir("note-outside");
        let victim = outside.join("loop.md");
        fs::write(&victim, "keep").unwrap();
        symlink(&outside, book.join("notes/overview")).unwrap();
        let error = add(&book, "loop", &["src/core/engine.rs:2-4"], "Test.")
            .unwrap_err()
            .to_string();
        // Existing files are not replaced without --replace; deletion must still refuse.
        assert!(error.contains("already exists"), "{error}");
        let error = note_rm(&book, "overview/loop").unwrap_err().to_string();
        assert!(
            error.contains("refusing to write a note outside the book"),
            "{error}"
        );
        let error = add(&book, "new", &["src/core/engine.rs:2-4"], "Test.")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("refusing to write a note outside the book"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(victim).unwrap(), "keep");
        assert!(!outside.join("new.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn check_refuses_to_rewrite_symlinked_note() {
        use std::os::unix::fs::symlink;
        let (_, book) = fixture();
        let outside = book.parent().unwrap().join("outside.md");
        let content = "---\ntitle: Engine\nevidence:\n  - src/core/engine.rs:2-4\n---\n\n`run` calls `step`.\n";
        fs::write(&outside, content).unwrap();
        fs::create_dir_all(book.join("notes/overview")).unwrap();
        symlink(&outside, book.join("notes/overview/loop.md")).unwrap();
        let error = check(&book, None).unwrap_err().to_string();
        assert!(
            error.contains("refusing to write a note outside the book (symlink or escaping path)"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(outside).unwrap(), content);
    }

    #[cfg(unix)]
    #[test]
    fn legacy_default_output_rejects_symlink_but_explicit_out_works() {
        use std::os::unix::fs::symlink;
        let (repo, _) = fixture();
        init(&InitOptions {
            repo: repo.clone(),
            book: Some(repo.clone()),
            title: Some("Legacy".into()),
            lang: "en".into(),
        })
        .unwrap();
        let book = find_book_from(&repo).unwrap();
        assert_eq!(load_book(&book).unwrap().repo, ".");
        let outside = repo.parent().unwrap().join("outside");
        fs::create_dir(&outside).unwrap();
        let manifest = format!("{MANIFEST_HEADER}\nvictim.md\n");
        fs::write(outside.join(BUILD_MARKER), &manifest).unwrap();
        fs::write(outside.join("victim.md"), "keep").unwrap();
        symlink("../outside", repo.join("out")).unwrap();
        let before = fs::read(book.join(BOOK_FILE)).unwrap();

        let error = build(&book, None, false, None).unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "{}: output directory resolves outside the book; use --out to choose an external directory",
                book.canonicalize().unwrap().join("out").display()
            )
        );
        assert_eq!(fs::read(book.join(BOOK_FILE)).unwrap(), before);
        assert_eq!(
            fs::read_to_string(outside.join("victim.md")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(outside.join(BUILD_MARKER)).unwrap(),
            manifest
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 2);

        build(&book, Some(outside.clone()), false, None).unwrap();
        assert!(!outside.join("victim.md").exists());
        assert!(outside.join("index.md").is_file());
        assert!(outside.join(BUILD_MARKER).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn build_rejects_symlinked_default_output_without_touching_anything() {
        use std::os::unix::fs::symlink;
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        let outside = temp_dir("default-output-outside");
        let manifest = format!("{MANIFEST_HEADER}\nvictim\n");
        fs::write(outside.join(BUILD_MARKER), &manifest).unwrap();
        fs::write(outside.join("victim"), "keep").unwrap();
        symlink(&outside, repo.join("docs")).unwrap();
        let before = fs::read(book.join(BOOK_FILE)).unwrap();
        let error = build(&book, None, false, None).unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "{}: output directory resolves outside the repository; use --out to choose an external directory",
                repo.canonicalize().unwrap().join("docs").display()
            )
        );
        assert_eq!(fs::read(book.join(BOOK_FILE)).unwrap(), before);
        assert_eq!(fs::read_to_string(outside.join("victim")).unwrap(), "keep");
        assert_eq!(
            fs::read_to_string(outside.join(BUILD_MARKER)).unwrap(),
            manifest
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn build_rejects_symlinked_configured_output_but_explicit_out_works() {
        use std::os::unix::fs::symlink;
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        let path = book.join(BOOK_FILE);
        let text = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            text.replace("repo: ..\n", "repo: ..\nout: alias/generated\n"),
        )
        .unwrap();
        let outside = temp_dir("configured-output-outside");
        symlink(&outside, repo.join("alias")).unwrap();
        let error = build(&book, None, false, None).unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "{}: output directory resolves outside the repository; use --out to choose an external directory",
                repo.canonicalize()
                    .unwrap()
                    .join("alias/generated")
                    .display()
            )
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        build(&book, Some(outside.clone()), false, None).unwrap();
        assert!(outside.join("index.md").is_file());
        assert!(outside.join(BUILD_MARKER).is_file());
        assert!(!outside.join("generated").exists());
    }

    #[cfg(unix)]
    #[test]
    fn build_rejects_internal_and_dangling_output_symlinks() {
        use std::os::unix::fs::symlink;
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        let docs = repo.join("docs");
        let target = repo.join("target-output");
        fs::create_dir(&target).unwrap();
        symlink(&target, &docs).unwrap();
        assert!(
            build(&book, None, false, None)
                .unwrap_err()
                .to_string()
                .contains("output directory resolves outside the repository")
        );
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        fs::remove_file(&docs).unwrap();
        symlink(repo.join("missing"), &docs).unwrap();
        assert!(
            build(&book, None, false, None)
                .unwrap_err()
                .to_string()
                .contains("output directory resolves outside the repository")
        );
        assert!(!repo.join("missing").exists());
    }

    #[cfg(unix)]
    #[test]
    fn build_rejects_escaping_language_directory_before_deletion() {
        use std::os::unix::fs::symlink;
        let (repo, book) = fixture();
        let book_md = book.join(BOOK_FILE);
        let text = fs::read_to_string(&book_md).unwrap();
        fs::write(
            &book_md,
            text.replacen("---\n\n", "langs:\n  - zh\n---\n\n", 1),
        )
        .unwrap();
        let out = repo.join("docs");
        fs::create_dir_all(&out).unwrap();
        let outside = temp_dir("output-outside");
        fs::write(outside.join("all.md"), "keep").unwrap();
        fs::write(out.join("all.md"), "old").unwrap();
        fs::write(
            out.join(BUILD_MARKER),
            format!("{MANIFEST_HEADER}\nall.md\nzh/all.md\n"),
        )
        .unwrap();
        symlink(&outside, out.join("zh")).unwrap();
        let error = build(&book, Some(out.clone()), true, None)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("refusing to write outside the output directory"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(outside.join("all.md")).unwrap(), "keep");
        assert_eq!(fs::read_to_string(out.join("all.md")).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn build_rejects_symlinked_marker_before_deletion() {
        use std::os::unix::fs::symlink;
        let (repo, book) = fixture();
        let out = repo.join("docs");
        fs::create_dir_all(&out).unwrap();
        let outside = repo.join("marker.txt");
        let manifest = format!("{MANIFEST_HEADER}\nall.md\n");
        fs::write(&outside, &manifest).unwrap();
        fs::write(out.join("all.md"), "old").unwrap();
        symlink(&outside, out.join(BUILD_MARKER)).unwrap();
        let error = build(&book, Some(out.clone()), true, None)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("refusing to write outside the output directory"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(outside).unwrap(), manifest);
        assert_eq!(fs::read_to_string(out.join("all.md")).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn accept_refuses_symlinked_translation_and_book() {
        use std::os::unix::fs::symlink;
        let (_, book) = fixture();
        let book_md = book.join(BOOK_FILE);
        let text = fs::read_to_string(&book_md).unwrap();
        fs::write(
            &book_md,
            text.replacen("---\n\n", "langs:\n  - zh\n---\n\n", 1),
        )
        .unwrap();
        let outside = book.parent().unwrap().join("translation.md");
        let translation = "---\ntitle: 中文\n---\n\nTranslation.\n";
        fs::write(&outside, translation).unwrap();
        fs::create_dir_all(book.join("zh")).unwrap();
        symlink(&outside, book.join("zh/book.md")).unwrap();
        let error = accept(&book, &["zh/book".into()], false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("refusing to write a note outside the book"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(&outside).unwrap(), translation);

        fs::remove_file(book.join("zh/book.md")).unwrap();
        fs::write(book.join("zh/book.md"), translation).unwrap();
        let outside_book = book.parent().unwrap().join("external-book.md");
        let content = fs::read(&book_md).unwrap();
        fs::write(&outside_book, &content).unwrap();
        fs::remove_file(&book_md).unwrap();
        symlink(&outside_book, &book_md).unwrap();
        let error = crate::translate::sync(&book, &["zh".into()])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("refusing to write a note outside the book"),
            "{error}"
        );
        assert_eq!(fs::read(outside_book).unwrap(), content);
    }

    #[cfg(unix)]
    #[test]
    fn output_rejects_symlinked_generated_file() {
        use std::os::unix::fs::symlink;
        let (repo, book) = fixture();
        let out = repo.join("docs");
        fs::create_dir_all(&out).unwrap();
        let outside = repo.join("outside.md");
        fs::write(&outside, "keep").unwrap();
        symlink(&outside, out.join("all.md")).unwrap();
        let error = build(&book, Some(out), true, None).unwrap_err().to_string();
        assert!(
            error.contains("refusing to write outside the output directory"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
    }

    #[test]
    fn accept_does_not_count_punctuation_as_overlap() {
        let (repo, book) = fixture();
        let source = repo.join("src/core/engine.rs");
        fs::write(
            &source,
            "fn authorize(u: &User) -> bool {\n    u.is_admin()\n}\n",
        )
        .unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "authorize",
            ],
        );
        let mut loaded = load_book(&book).unwrap();
        loaded.commit = current_commit(&repo).unwrap();
        fs::write(book.join(BOOK_FILE), render_book(&loaded)).unwrap();
        add(&book, "auth", &["src/core/engine.rs:1-3"], "Authorization.").unwrap();
        let note = note_path(&loaded, "overview", "auth");
        let before = fs::read(&note).unwrap();
        fs::write(
            &source,
            "fn delete_backup(p: &Path) {\n    fs::remove_dir_all(p).ok();\n}\n",
        )
        .unwrap();
        assert!(accept(&book, &["overview/auth".into()], false).is_err());
        assert_eq!(fs::read(note).unwrap(), before);
    }

    #[test]
    fn accept_counts_each_new_line_only_once() {
        let (repo, book) = fixture();
        let source = repo.join("src/core/engine.rs");
        fs::write(&source, "fn authorize() {\n    trace();\n    trace();\n    trace();\n    trace();\n    check();\n}\n").unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "authorize",
            ],
        );
        let mut loaded = load_book(&book).unwrap();
        loaded.commit = current_commit(&repo).unwrap();
        fs::write(book.join(BOOK_FILE), render_book(&loaded)).unwrap();
        add(&book, "auth", &["src/core/engine.rs:1-7"], "Authorization.").unwrap();
        let note = note_path(&loaded, "overview", "auth");
        let before = fs::read(&note).unwrap();
        fs::write(&source, "fn archive() {\n    trace();\n    move_files();\n    rotate();\n    compress();\n    upload();\n}\n").unwrap();
        assert!(accept(&book, &["overview/auth".into()], false).is_err());
        assert_eq!(fs::read(note).unwrap(), before);
    }

    #[test]
    fn failed_atomic_write_removes_temporary_file() {
        let dir = temp_dir("failed-write");
        let destination = dir.join("destination");
        fs::create_dir(&destination).unwrap();
        assert!(write_file(destination.clone(), b"data").is_err());
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(entries, vec![destination]);
    }

    #[cfg(unix)]
    #[test]
    fn note_replace_and_check_do_not_modify_hard_links() {
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        add(&book, "loop", &["README.md"], "Original.").unwrap();
        let loaded = load_book(&book).unwrap();
        let path = note_path(&loaded, "overview", "loop");
        let outside = repo.parent().unwrap().join("outside-note.md");
        let original = fs::read(&path).unwrap();
        fs::write(&outside, &original).unwrap();
        fs::remove_file(&path).unwrap();
        fs::hard_link(&outside, &path).unwrap();
        note_add(&NoteAddOptions {
            book: book.clone(),
            page: "overview".into(),
            id: "loop".into(),
            title: None,
            order: None,
            evidence: vec![],
            add_evidence: vec![],
            body: "Replaced.".into(),
            replace: true,
        })
        .unwrap();
        assert_eq!(fs::read(&outside).unwrap(), original);
        assert_ne!(fs::read(&path).unwrap(), original);

        let mut note = load_notes(&loaded).unwrap().remove(0);
        note.evidence[0].pin = None;
        let unpinned = render_note(&note);
        fs::write(&outside, &unpinned).unwrap();
        fs::remove_file(&path).unwrap();
        fs::hard_link(&outside, &path).unwrap();
        check(&book, None).unwrap();
        assert_eq!(fs::read_to_string(&outside).unwrap(), unpinned);
        assert!(load_notes(&loaded).unwrap()[0].evidence[0].pin.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn build_does_not_modify_hard_linked_manifest() {
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        add(&book, "loop", &["README.md"], "Demo.").unwrap();
        let out = repo.join("docs");
        fs::create_dir_all(&out).unwrap();
        let outside = repo.parent().unwrap().join("outside-manifest");
        let manifest = format!("{MANIFEST_HEADER}\n");
        fs::write(&outside, &manifest).unwrap();
        fs::hard_link(&outside, out.join(BUILD_MARKER)).unwrap();
        build(&book, Some(out.clone()), false, None).unwrap();
        assert_eq!(fs::read_to_string(outside).unwrap(), manifest);
        assert_ne!(
            fs::read_to_string(out.join(BUILD_MARKER)).unwrap(),
            manifest
        );
    }

    fn init_in_repo(repo: &Path) -> PathBuf {
        init(&InitOptions {
            repo: repo.to_path_buf(),
            book: None,
            title: Some("Demo".into()),
            lang: "en".into(),
        })
        .unwrap();
        repo.join(".codocia")
    }

    #[test]
    fn default_init_is_portable_and_explicit_nested_and_external_books_work() {
        let (repo, external) = fixture();
        let book = init_in_repo(&repo);
        let text = fs::read_to_string(book.join(BOOK_FILE)).unwrap();
        assert!(text.lines().any(|line| line == "repo: .."), "{text}");
        assert!(!text.contains(&repo.to_string_lossy().to_string()));
        assert_eq!(
            load_book(&external).unwrap().repo,
            repo.canonicalize().unwrap().to_string_lossy()
        );
        let nested = repo.join("nested/book");
        init(&InitOptions {
            repo: repo.clone(),
            book: Some(nested.clone()),
            title: None,
            lang: "en".into(),
        })
        .unwrap();
        assert_eq!(load_book(&nested).unwrap().repo, "../..");
    }

    /// The nearest book above `dir`, the way discovery finds it.
    fn nearest_book_above(dir: &Path) -> Option<PathBuf> {
        for ancestor in dir.ancestors() {
            let candidate = ancestor.join(".codocia");
            if candidate.join(BOOK_FILE).is_file() {
                return Some(candidate);
            }
            if ancestor.join(BOOK_FILE).is_file() && ancestor.join(NOTES_DIR).is_dir() {
                return Some(ancestor.to_path_buf());
            }
        }
        None
    }

    // Spawn the test binary instead of changing the process-wide working directory:
    // other tests run in parallel and may be using relative paths.
    #[test]
    fn discover_book_from_current_directory() {
        if let Ok(expected) = std::env::var("CODOCIA_TEST_DISCOVERY") {
            if expected == "missing" {
                assert_eq!(
                    find_book(None).unwrap_err().to_string(),
                    "no Codocia book found; run `codocia init` in the repository"
                );
            } else {
                assert_eq!(
                    find_book(None).unwrap().canonicalize().unwrap(),
                    PathBuf::from(expected).canonicalize().unwrap()
                );
            }
            assert_eq!(
                find_book(Some(Path::new("explicit"))).unwrap(),
                PathBuf::from("explicit")
            );
            return;
        }
        let (repo, external) = fixture();
        let book = init_in_repo(&repo);
        let unrelated = temp_dir("unrelated");
        let mut cases = vec![
            (repo.join("src/core"), book.to_string_lossy().to_string()),
            (external.clone(), external.to_string_lossy().to_string()),
        ];
        // The temporary directory can sit inside a checkout that has a book
        // of its own; discovery then finds that nearest book, and only a
        // directory without any book above it may report "missing".
        if nearest_book_above(&unrelated).is_none() {
            cases.push((unrelated.clone(), "missing".to_string()));
        }
        for (cwd, expected) in cases {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "book::tests::discover_book_from_current_directory",
                    "--nocapture",
                ])
                .env("CODOCIA_TEST_DISCOVERY", expected)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn configured_output_is_relative_validated_and_overridable() {
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        let path = book.join(BOOK_FILE);
        let original = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            original.replace("repo: ..\n", "repo: ..\nout: docs/public\n"),
        )
        .unwrap();
        build(&book, None, false, None).unwrap();
        assert!(repo.join("docs/public/README.md").is_file());
        let page = fs::read_to_string(repo.join("docs/public/01-overview.md")).unwrap();
        assert!(page.contains("../../src/core/engine.rs#L2-L4"), "{page}");
        build(&book, Some(repo.join("custom-out")), false, None).unwrap();
        assert!(repo.join("custom-out/README.md").is_file());
        for invalid in ["/tmp/x", "../x"] {
            fs::write(
                &path,
                original.replace("repo: ..\n", &format!("repo: ..\nout: {invalid}\n")),
            )
            .unwrap();
            let error = load_book(&book).unwrap_err().to_string();
            assert!(
                error.contains("out") && error.contains("repository-relative"),
                "{error}"
            );
        }
    }

    #[test]
    fn book_without_out_renders_without_out() {
        let (_, book) = fixture();
        let loaded = load_book(&book).unwrap();
        let rendered = render_book(&loaded);
        assert!(!rendered.lines().any(|line| line.starts_with("out:")));
        fs::write(book.join(BOOK_FILE), rendered).unwrap();
        let again = render_book(&load_book(&book).unwrap());
        assert_eq!(again, fs::read_to_string(book.join(BOOK_FILE)).unwrap());
    }

    #[test]
    fn in_repo_index_footer_names_repository_without_remote() {
        let root = temp_dir("demo-repo-name");
        let repo = root.join("demo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("README.md"), "# Demo\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "initial",
            ],
        );
        let book = init_in_repo(&repo);
        assert!(git_remote_url(&repo).is_none());
        build(&book, None, false, None).unwrap();
        let index = fs::read_to_string(repo.join("docs/README.md")).unwrap();
        assert!(index.contains("Generated from demo @ `"), "{index}");
    }

    #[test]
    fn in_repo_book_build_and_coverage_are_portable() {
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        git(&repo, &["add", ".codocia"]);
        check(&book, None).unwrap();
        build(&book, None, false, None).unwrap();
        assert!(repo.join("docs/README.md").is_file());
        let page = fs::read_to_string(repo.join("docs/01-overview.md")).unwrap();
        assert!(page.contains("../src/core/engine.rs#L2-L4"), "{page}");
        for entry in fs::read_dir(repo.join("docs")).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let content = fs::read_to_string(path).unwrap();
                assert!(
                    !content.contains(&repo.canonicalize().unwrap().to_string_lossy().to_string())
                );
            }
        }
        let loaded = load_book(&book).unwrap();
        let inventory = Inventory::scan(&loaded.repo_path(), &loaded.effective_ignore()).unwrap();
        assert!(inventory.files.keys().any(|p| p == "src/core/engine.rs"));
        assert!(inventory.files.keys().all(|p| !p.starts_with(".codocia/")));
        let map = structure::build_map(&loaded, &load_notes(&loaded).unwrap(), 30).unwrap();
        let rendered = structure::render(&loaded, &map, |s| s.to_string());
        assert!(!rendered.contains(".codocia/"), "{rendered}");
        status(&book, 30, None).unwrap();
    }

    #[test]
    fn rebase_skips_book_and_generated_docs_commits() {
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        let original = load_book(&book).unwrap().commit;
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        build(&book, None, false, None).unwrap();
        git(&repo, &["add", ".codocia", "docs"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "book and generated docs",
            ],
        );
        rebase(&book).unwrap();
        assert_eq!(load_book(&book).unwrap().commit, original);
        let note = book.join("notes/overview/loop.md");
        let text = fs::read_to_string(&note).unwrap();
        fs::write(
            &note,
            text.replace("`run` calls `step`.", "The engine runs."),
        )
        .unwrap();
        git(&repo, &["add", ".codocia/notes/overview/loop.md"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "edit note only",
            ],
        );
        rebase(&book).unwrap();
        assert_eq!(load_book(&book).unwrap().commit, original);
        fs::write(repo.join("src/core/engine.rs"), "fn changed() {}\n").unwrap();
        git(&repo, &["add", "src/core/engine.rs"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "code",
            ],
        );
        assert!(code_moved(
            &repo,
            &original,
            &current_commit(&repo).unwrap(),
            &load_book(&book).unwrap().effective_ignore()
        ));
    }

    #[test]
    fn commit_off_head_is_flagged() {
        let (repo, _book) = fixture();
        let commit = |msg: &str| {
            git(
                &repo,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    msg,
                ],
            );
            let out = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        let old = commit("original");
        git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
        let new = commit("rewritten");
        let state =
            |sha: &str| evidence_state(&repo, &parse_evidence(&format!("commit:{sha}")).unwrap());
        assert_eq!(state(&new), EvidenceState::Ok);
        assert!(matches!(state(&old), EvidenceState::BadRef(_)));
    }

    #[test]
    fn translations_track_their_source() {
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        let book_md = book.join(BOOK_FILE);
        let text = fs::read_to_string(&book_md).unwrap();
        fs::write(
            &book_md,
            text.replacen(
                "---\n\n",
                "langs:\n  - zh\ntranslations:\n  - README_CN.md: README.md\n---\n\n",
                1,
            ),
        )
        .unwrap();
        let errors = |b: &Path| {
            let bk = load_book(b).unwrap();
            let notes = load_notes(&bk).unwrap();
            check_book(&bk, &notes).unwrap()
        };
        let r = errors(&book);
        assert!(
            r.errors.iter().any(|e| e.starts_with("zh/book: missing")),
            "{r:?}"
        );
        assert!(
            r.errors
                .iter()
                .any(|e| e.starts_with("zh/overview/loop: missing"))
        );
        assert!(
            r.errors
                .iter()
                .any(|e| e.starts_with("README_CN.md: missing"))
        );

        fs::create_dir_all(book.join("zh/overview")).unwrap();
        fs::write(
            book.join("zh/book.md"),
            "---\ntitle: 演示\npages:\n  - overview: 概览\n---\n\n",
        )
        .unwrap();
        fs::write(
            book.join("zh/overview/loop.md"),
            "---\ntitle: 引擎循环\n---\n\n`run` 调用 `step`。\n",
        )
        .unwrap();
        fs::write(repo.join("README_CN.md"), "# 演示\n").unwrap();
        let r = errors(&book);
        assert_eq!(
            r.errors
                .iter()
                .filter(|e| e.contains("never synced"))
                .count(),
            3,
            "{r:?}"
        );
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("`codocia accept zh/overview/loop`")),
            "{r:?}"
        );
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("`codocia accept README_CN.md`")),
            "{r:?}"
        );
        accept(&book, &["zh".into(), "README_CN.md".into()], false).unwrap();
        let r = errors(&book);
        assert!(r.errors.is_empty() && r.warnings.is_empty(), "{r:?}");

        // The source moves on: both translations drift, parity notices code.
        let note = note_path(&load_book(&book).unwrap(), "overview", "loop");
        let text = fs::read_to_string(&note).unwrap();
        fs::write(
            &note,
            text.replace("`run` calls `step`.", "`run` calls `step` and `stop`."),
        )
        .unwrap();
        fs::write(repo.join("README.md"), "# Demo\n\nMore.\n").unwrap();
        let r = errors(&book);
        assert!(
            r.errors
                .iter()
                .any(|e| e.starts_with("zh/overview/loop: source changed")),
            "{r:?}"
        );
        assert!(
            r.errors
                .iter()
                .any(|e| e.starts_with("README_CN.md: source changed"))
        );
        assert!(
            r.warnings.iter().any(|w| w.contains("missing `stop`")),
            "{r:?}"
        );

        fs::write(
            book.join("zh/overview/loop.md"),
            "---\ntitle: 引擎循环\n---\n\n`run` 调用 `step` 和 `stop`。\n",
        )
        .unwrap();
        accept(
            &book,
            &["zh/overview/loop".into(), "README_CN.md".into()],
            false,
        )
        .unwrap();
        assert!(errors(&book).errors.is_empty());

        build(&book, Some(repo.join("docs")), false, None).unwrap();
        let en = fs::read_to_string(repo.join("docs/01-overview.md")).unwrap();
        let zh = fs::read_to_string(repo.join("docs/zh/01-overview.md")).unwrap();
        assert!(
            en.contains("**English** · [中文](zh/01-overview.md)"),
            "{en}"
        );
        assert!(
            zh.contains("[English](../01-overview.md) · **中文**"),
            "{zh}"
        );
        assert!(zh.contains("# 概览") && zh.contains("## 引擎循环") && zh.contains("来源"));
        assert!(zh.contains("](../../src/core/engine.rs#L2-L4)"), "{zh}");
        assert!(repo.join("docs/zh/README.md").is_file());
    }

    fn add(book: &Path, id: &str, evidence: &[&str], body: &str) -> Result<()> {
        note_add(&NoteAddOptions {
            book: book.to_path_buf(),
            page: "overview".to_string(),
            id: id.to_string(),
            title: Some("Engine loop".to_string()),
            order: None,
            evidence: evidence.iter().map(|s| s.to_string()).collect(),
            add_evidence: Vec::new(),
            body: body.to_string(),
            replace: false,
        })
    }

    #[test]
    fn check_follows_citations_and_reports_count() {
        if let Some(book) = std::env::var_os("CODOCIA_FOLLOW_TEST_BOOK") {
            check(Path::new(&book), None).unwrap();
            return;
        }
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        fs::write(
            repo.join("src/core/engine.rs"),
            "// header\n\nfn a() {}\nfn run() {\n    step();\n}\nfn z() {}\n",
        )
        .unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "book::tests::check_follows_citations_and_reports_count",
                "--nocapture",
            ])
            .env("CODOCIA_FOLLOW_TEST_BOOK", &book)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout} {:?}", output.stderr);
        assert!(stdout.contains("followed 1 moved citation(s)"), "{stdout}");
        assert!(!stdout.contains("moved to lines"), "{stdout}");
        let notes = load_notes(&load_book(&book).unwrap()).unwrap();
        assert!(render_note(&notes[0]).contains("src/core/engine.rs:4-6"));
    }

    #[test]
    fn accept_reviews_changed_and_unrelated_ranges() {
        if let Some(book) = std::env::var_os("CODOCIA_ACCEPT_TEST_BOOK") {
            assert!(accept(Path::new(&book), &["overview/loop".into()], false).is_err());
            return;
        }
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        fs::write(
            repo.join("src/core/engine.rs"),
            "fn a() {}\nfn run() {\n    step_twice();\n}\nfn z() {}\n",
        )
        .unwrap();
        assert!(check(&book, None).is_err());
        let loaded = load_book(&book).unwrap();
        let report = check_book(&loaded, &load_notes(&loaded).unwrap()).unwrap();
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("review the note, then `codocia accept overview/loop`")),
            "{report:?}"
        );
        accept(&book, &["overview/loop".into()], false).unwrap();
        check(&book, None).unwrap();
        fs::write(
            repo.join("src/core/engine.rs"),
            "fn a() {}\ncompletely\ndifferent\ncode\nfn z() {}\n",
        )
        .unwrap();
        let before = fs::read(note_path(&loaded, "overview", "loop")).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "book::tests::accept_reviews_changed_and_unrelated_ranges",
                "--nocapture",
            ])
            .env("CODOCIA_ACCEPT_TEST_BOOK", &book)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout} {:?}", output.stderr);
        assert!(stdout.contains("overview/loop: src/core/engine.rs:2-4 cannot be compared with its pinned version; check the range, then accept --force"), "{stdout}");
        assert!(
            stdout.contains("before:") && stdout.contains("after:"),
            "{stdout}"
        );
        assert_eq!(
            fs::read(note_path(&loaded, "overview", "loop")).unwrap(),
            before
        );
        accept(&book, &["loop".into()], true).unwrap();
        check(&book, None).unwrap();
        assert!(accept(&book, &["unknown".into()], true).is_err());
    }

    #[test]
    fn accept_finds_moved_before_by_pin_and_requires_available_before() {
        if let Some(book) = std::env::var_os("CODOCIA_MOVED_ACCEPT_BOOK") {
            let result = accept(Path::new(&book), &["overview/loop".into()], false);
            if std::env::var_os("CODOCIA_EXPECT_ACCEPT").is_some() {
                result.unwrap();
            } else {
                assert!(result.is_err());
            }
            return;
        }
        let run = |book: &Path, success: bool| {
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "book::tests::accept_finds_moved_before_by_pin_and_requires_available_before",
                "--nocapture",
            ])
            .env("CODOCIA_MOVED_ACCEPT_BOOK", book);
            if success {
                cmd.env("CODOCIA_EXPECT_ACCEPT", "1");
            }
            let output = cmd.output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            assert!(output.status.success(), "{stdout} {:?}", output.stderr);
            stdout
        };
        for changed in [false, true] {
            let (repo, _) = fixture();
            let path = repo.join("src/core/engine.rs");
            fs::write(
                &path,
                "fn main() {\n    let retries = 5;\n    send(retries);\n}\n",
            )
            .unwrap();
            git(&repo, &["add", "src/core/engine.rs"]);
            git(
                &repo,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-qm",
                    "main",
                ],
            );
            // Initialize after the commit so the book's pinned version contains the code.
            let book = init_in_repo(&repo);
            add(&book, "loop", &["src/core/engine.rs:2-3"], "x").unwrap();
            fs::write(
                &path,
                "// a\n// b\nfn main() {\n    let retries = 5;\n    send(retries);\n}\n",
            )
            .unwrap();
            check(&book, None).unwrap();
            assert!(
                render_note(&load_notes(&load_book(&book).unwrap()).unwrap()[0])
                    .contains("src/core/engine.rs:4-5")
            );
            if changed {
                fs::write(
                    &path,
                    "// a\n// b\nfn main() {\n    let retries = 5;\n    send_again(retries);\n}\n",
                )
                .unwrap();
            } else {
                fs::write(
                    &path,
                    "fn other() {\n    unrelated();\n    stuff();\n    more();\n}\n",
                )
                .unwrap();
            }
            let stdout = run(&book, changed);
            assert!(
                stdout.contains("before:\n        let retries = 5;\n        send(retries);"),
                "{stdout}"
            );
            if !changed {
                assert!(stdout.contains("now covers different code"), "{stdout}");
                accept(&book, &["overview/loop".into()], true).unwrap();
            } else {
                assert!(stdout.contains("accepted overview/loop"), "{stdout}");
            }
        }

        let (repo, book) = fixture();
        let path = repo.join("new.rs");
        fs::write(&path, "old\n").unwrap();
        add(&book, "loop", &["new.rs:1-1"], "x").unwrap();
        fs::write(&path, "new\n").unwrap();
        let stdout = run(&book, false);
        assert!(stdout.contains("before: (unavailable)"), "{stdout}");
        assert!(stdout.contains("overview/loop: new.rs:1 cannot be compared with its pinned version; check the range, then accept --force"), "{stdout}");
        accept(&book, &["overview/loop".into()], true).unwrap();
    }

    #[test]
    fn pinned_lines_uses_first_match_without_fallback() {
        let pin = hash_lines(&["wanted", "code"]);
        assert_eq!(
            pinned_lines(
                "wanted\ncode\nother\nwanted\ncode\n",
                Some((3, 4)),
                Some(&pin)
            ),
            Some(vec!["wanted", "code"])
        );
        assert_eq!(pinned_lines("old\ntext\n", Some((1, 2)), Some(&pin)), None);
        assert_eq!(pinned_lines("old\n", Some((2, 3)), Some(&pin)), None);
    }

    #[test]
    fn build_pins_code_but_skips_book_only_commit() {
        let (repo, _) = fixture();
        let book = init_in_repo(&repo);
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        fs::write(repo.join("src/other.rs"), "fn other() {}\n").unwrap();
        git(&repo, &["add", "src/other.rs"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "code",
            ],
        );
        let code_commit = current_commit(&repo).unwrap();
        build(&book, None, false, None).unwrap();
        assert_eq!(load_book(&book).unwrap().commit, code_commit);
        git(&repo, &["add", ".codocia"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "book",
            ],
        );
        build(&book, None, false, None).unwrap();
        assert_eq!(load_book(&book).unwrap().commit, code_commit);
    }

    #[test]
    fn check_pins_new_evidence_and_accept_force_does_not_fix_bad_ranges() {
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        let loaded = load_book(&book).unwrap();
        let path = note_path(&loaded, "overview", "loop");
        let mut note = load_notes(&loaded).unwrap().remove(0);
        note.evidence[0].pin = None;
        fs::write(&path, render_note(&note)).unwrap();
        check(&book, None).unwrap();
        assert!(load_notes(&loaded).unwrap()[0].evidence[0].pin.is_some());
        fs::write(repo.join("src/core/engine.rs"), "short\n").unwrap();
        assert!(accept(&book, &["loop".into()], true).is_err());
        assert!(check(&book, None).is_err());
    }

    #[test]
    fn forced_build_moves_commit_even_if_check_fails() {
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        let old = load_book(&book).unwrap().commit;
        fs::write(
            repo.join("src/core/engine.rs"),
            "fn a() {}\nfn run() {\n    changed();\n}\nfn z() {}\n",
        )
        .unwrap();
        git(&repo, &["add", "src/core/engine.rs"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "change",
            ],
        );
        assert!(build(&book, None, false, None).is_err());
        assert_eq!(load_book(&book).unwrap().commit, old);
        build(&book, None, true, None).unwrap();
        assert_eq!(
            load_book(&book).unwrap().commit,
            current_commit(&repo).unwrap()
        );
    }

    #[test]
    fn check_follows_renamed_files() {
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        git(&repo, &["mv", "src/core/engine.rs", "src/core/runner.rs"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "rename",
            ],
        );
        let loaded = load_book(&book).unwrap();
        let report = check_book(&loaded, &load_notes(&loaded).unwrap()).unwrap();
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("renamed to src/core/runner.rs")),
            "{:?}",
            report.errors
        );
        check(&book, None).unwrap();
        let notes = load_notes(&loaded).unwrap();
        assert!(render_note(&notes[0]).contains("src/core/runner.rs:2-4"));
        let report = check_book(&loaded, &notes).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }

    #[test]
    fn generated_docs_are_not_code() {
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        build(&book, Some(repo.join("docs")), false, None).unwrap();
        let pinned = load_book(&book).unwrap().commit;
        git(&repo, &["add", "docs"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "docs",
            ],
        );
        let inventory = Inventory::scan(&repo, &[]).unwrap();
        assert!(inventory.files.keys().all(|p| !p.starts_with("docs/")));
        rebase(&book).unwrap();
        assert_eq!(load_book(&book).unwrap().commit, pinned);
    }

    #[test]
    fn repeated_explanations_across_pages_are_flagged() {
        let ev = |p: &str, a: usize, b: usize| Evidence {
            target: EvidenceRef::File {
                path: p.to_string(),
                lines: Some((a, b)),
            },
            pin: None,
        };
        let note = |page: &str, id: &str, evidence: Vec<Evidence>| Note {
            page: page.to_string(),
            id: id.to_string(),
            title: String::new(),
            order: None,
            evidence,
            body: "x".to_string(),
        };
        let notes = vec![
            note("a", "one", vec![ev("x.rs", 10, 20), ev("y.rs", 1, 9)]),
            note("b", "two", vec![ev("x.rs", 12, 22), ev("y.rs", 2, 8)]),
            note("c", "three", vec![ev("x.rs", 15, 16)]),
            note("a", "four", vec![ev("x.rs", 10, 20), ev("y.rs", 1, 9)]),
        ];
        let warnings = repetition_warnings(&notes);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().all(|w| w.contains("b/two")));
    }

    #[test]
    fn plain_directories_work_without_git() {
        let root = temp_dir("plain");
        let repo = root.join("src-tree");
        fs::create_dir_all(repo.join("pkg/.cache")).unwrap();
        fs::create_dir_all(repo.join("node_modules/x")).unwrap();
        fs::write(repo.join("pkg/core.py"), "def run():\n    return 1\n").unwrap();
        fs::write(repo.join("pkg/.cache/junk.py"), "x\n").unwrap();
        fs::write(repo.join("node_modules/x/i.js"), "x\n").unwrap();
        let book = root.join("book");
        init(&InitOptions {
            repo: repo.clone(),
            book: Some(book.clone()),
            title: None,
            lang: "en".to_string(),
        })
        .unwrap();
        assert_eq!(walk_files(&repo).unwrap(), vec!["pkg/core.py"]);
        add(&book, "run", &["pkg/core.py:1-2"], "`run` returns 1.").unwrap();
        assert!(add(&book, "c", &["commit:abcdef1"], "x").is_err());
        let loaded = load_book(&book).unwrap();
        let report = check_book(&loaded, &load_notes(&loaded).unwrap()).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        build(&book, None, false, None).unwrap();
    }

    #[test]
    fn quoted_ignore_globs_are_unquoted() {
        let (_repo, book) = fixture();
        let path = book.join(BOOK_FILE);
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("ignore:\n", "ignore:\n  - \"src/**\"\n  - 'README.md'\n");
        fs::write(&path, text).unwrap();
        let loaded = load_book(&book).unwrap();
        assert_eq!(loaded.ignore, vec!["src/**", "README.md"]);
    }

    /// A note with the given title and one pinned piece of evidence.
    fn note_with_title(title: &str) -> Note {
        Note {
            page: "overview".to_string(),
            id: "loop".to_string(),
            title: title.to_string(),
            order: None,
            evidence: vec![Evidence {
                target: EvidenceRef::File {
                    path: "src/core/engine.rs".to_string(),
                    lines: Some((1, 2)),
                },
                pin: Some("abc123".to_string()),
            }],
            body: "The loop runs.".to_string(),
        }
    }

    #[test]
    fn ignore_globs_are_quoted_when_yaml_needs_it() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.ignore = vec!["**/*.lock".to_string(), "docs/img/**".to_string()];
        let rendered = render_book(&loaded);
        assert!(rendered.contains("- \"**/*.lock\""), "{rendered}");
        assert!(rendered.contains("- docs/img/**"), "{rendered}");
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.ignore, vec!["**/*.lock", "docs/img/**"]);
    }

    #[test]
    fn an_empty_note_title_has_no_trailing_space() {
        let note = note_with_title("");
        let rendered = render_note(&note);
        assert!(rendered.contains("title:\n"), "{rendered}");
        assert!(!rendered.contains("title: \n"), "{rendered}");
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(parsed.title, "");
    }

    #[test]
    fn values_with_single_quotes_round_trip() {
        for title in ["it's here", "'quoted'", "a 'b' c", "'''"] {
            let note = note_with_title(title);
            let rendered = render_note(&note);
            let parsed = parse_note("overview", "loop", &rendered).unwrap();
            assert_eq!(parsed.title, title, "{rendered}");
            assert_eq!(
                parsed.evidence[0].pin.as_deref(),
                Some("abc123"),
                "{rendered}"
            );
        }
        let rendered = render_note(&note_with_title("it's here"));
        assert!(rendered.contains("title: it's here\n"), "{rendered}");
    }

    #[test]
    fn evidence_is_written_unquoted() {
        let note = Note {
            page: "overview".to_string(),
            id: "loop".to_string(),
            title: "Loop".to_string(),
            order: None,
            evidence: vec![Evidence {
                target: EvidenceRef::File {
                    path: "src/a.rs".to_string(),
                    lines: Some((1, 2)),
                },
                pin: Some("abc123".to_string()),
            }],
            body: "The loop runs.".to_string(),
        };
        let rendered = render_note(&note);
        assert!(
            rendered.contains("  - src/a.rs:1-2 #abc123\n"),
            "{rendered}"
        );
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(
            parsed.evidence[0].target,
            EvidenceRef::File {
                path: "src/a.rs".to_string(),
                lines: Some((1, 2)),
            }
        );
        assert_eq!(parsed.evidence[0].pin.as_deref(), Some("abc123"));
    }

    #[test]
    fn book_values_with_yaml_characters_round_trip() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.title = "Codocia: a 'notebook'".to_string();
        loaded.pages[0].title = "Overview: the 'whole' repo".to_string();
        loaded.ignore = vec!["**/*.lock".to_string(), "weird 'name'".to_string()];
        let rendered = render_book(&loaded);
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.title, "Codocia: a 'notebook'");
        assert_eq!(again.pages[0].title, "Overview: the 'whole' repo");
        assert_eq!(again.ignore, vec!["**/*.lock", "weird 'name'"]);
    }

    #[test]
    fn page_entries_quote_only_the_title() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.pages[0].title = "Overview: the 'whole' repo".to_string();
        let rendered = render_book(&loaded);
        assert!(
            rendered.contains("- overview: \"Overview: the 'whole' repo\""),
            "{rendered}"
        );
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.pages[0].slug, "overview");
        assert_eq!(again.pages[0].title, "Overview: the 'whole' repo");

        loaded.pages[0].title = "*star".to_string();
        let rendered = render_book(&loaded);
        assert!(rendered.contains("- overview: \"*star\""), "{rendered}");
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.pages[0].slug, "overview");
        assert_eq!(again.pages[0].title, "*star");
    }

    #[test]
    fn translation_entries_are_written_unquoted() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.translations = vec!["README_CN.md: README.md #abc1234".to_string()];
        let rendered = render_book(&loaded);
        assert!(
            rendered.contains("- README_CN.md: README.md #abc1234"),
            "{rendered}"
        );
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.translations, vec!["README_CN.md: README.md #abc1234"]);
    }

    #[test]
    fn translation_entries_keep_their_original_text() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.translations = vec![
            "README_CN.md: README.md #abc1234 and more".to_string(),
            "docs/zh.md:  docs/README.md  #def5678".to_string(),
        ];
        let rendered = render_book(&loaded);
        assert!(
            rendered.contains("- README_CN.md: README.md #abc1234 and more"),
            "{rendered}"
        );
        assert!(
            rendered.contains("- docs/zh.md:  docs/README.md  #def5678"),
            "{rendered}"
        );
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(
            again.translations,
            vec![
                "README_CN.md: README.md #abc1234 and more".to_string(),
                "docs/zh.md:  docs/README.md  #def5678".to_string(),
            ]
        );
    }

    #[test]
    fn langs_cannot_repeat_the_primary_language() {
        for lang_line in ["lang: en\n", "lang: 'en'\n", "lang: \"en\"\n"] {
            let (_repo, book) = fixture();
            let path = book.join(BOOK_FILE);
            let text = fs::read_to_string(&path).unwrap();
            let text = text.replace("lang: en\n", &format!("{lang_line}langs:\n  - en\n"));
            assert!(text.contains("langs:"), "{text}");
            fs::write(&path, text).unwrap();
            let error = load_book(&book).unwrap_err();
            assert!(error.to_string().contains("cannot list `en`"), "{error}");
        }
    }

    #[test]
    fn empty_scalars_are_written_without_a_value() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.title = String::new();
        loaded.commit = String::new();
        loaded.out = Some(String::new());
        let rendered = render_book(&loaded);
        assert!(rendered.contains("title:\n"), "{rendered}");
        assert!(rendered.contains("commit:\n"), "{rendered}");
        assert!(rendered.contains("out:\n"), "{rendered}");
        assert!(!rendered.contains("commit: \n"), "{rendered}");
        assert!(!rendered.contains("out: \n"), "{rendered}");
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.title, "");
        assert_eq!(again.commit, "");
        // An empty `out` reads back as no output at all, the way 0.2.0 reads
        // it: the default output directory is used instead.
        assert_eq!(again.out, None);
    }

    #[test]
    fn books_written_with_quoted_entries_still_load() {
        let (_repo, book) = fixture();
        let original = fs::read_to_string(book.join(BOOK_FILE)).unwrap();
        let quoted = original
            .replace("  - overview: Overview", "  - 'overview: Overview'")
            .replace(
                "ignore:\n",
                "ignore:\n  - '**/*.lock'\ntranslations:\n  - 'README_CN.md: README.md #abc1234'\n",
            );
        assert_ne!(quoted, original);
        fs::write(book.join(BOOK_FILE), &quoted).unwrap();
        let loaded = load_book(&book).unwrap();
        assert_eq!(loaded.pages[0].slug, "overview");
        assert_eq!(loaded.pages[0].title, "Overview");
        assert_eq!(loaded.ignore, vec!["**/*.lock"]);
        assert_eq!(
            loaded.translations,
            vec!["README_CN.md: README.md #abc1234".to_string()]
        );
    }

    #[test]
    fn translated_book_pages_quote_only_the_title() {
        let (_repo, book) = fixture();
        let book_md = book.join(BOOK_FILE);
        let text = fs::read_to_string(&book_md).unwrap();
        fs::write(
            &book_md,
            text.replacen("---\n\n", "langs:\n  - zh\n---\n\n", 1),
        )
        .unwrap();
        fs::create_dir_all(book.join("zh")).unwrap();
        fs::write(
            book.join("zh/book.md"),
            "---\ntitle: 演示\npages:\n  - 'overview: 概览'\n---\n\n译文。\n",
        )
        .unwrap();
        crate::translate::sync(&book, &["zh/book".into()]).unwrap();
        let written = fs::read_to_string(book.join("zh/book.md")).unwrap();
        assert!(written.contains("- overview: 概览"), "{written}");
        assert!(written.contains("title: 演示"), "{written}");
    }

    /// A book with `zh` as a second language and a translated book file.
    fn book_with_translation() -> PathBuf {
        let (_repo, book) = fixture();
        let book_md = book.join(BOOK_FILE);
        let text = fs::read_to_string(&book_md).unwrap();
        fs::write(
            &book_md,
            text.replacen("---\n\n", "langs:\n  - zh\n---\n\n", 1),
        )
        .unwrap();
        fs::create_dir_all(book.join("zh")).unwrap();
        fs::write(
            book.join("zh/book.md"),
            "---\ntitle: 演示\npages:\n  - overview: 概览\n---\n\n译文。\n",
        )
        .unwrap();
        book
    }

    #[test]
    fn a_translation_source_is_written_unquoted() {
        let book = book_with_translation();
        crate::translate::sync(&book, &["zh/book".into()]).unwrap();
        let written = fs::read_to_string(book.join("zh/book.md")).unwrap();
        // 0.2.0 reads the pin without unquoting it.
        assert!(written.contains("source: #"), "{written}");
        assert!(!written.contains("source: '#"), "{written}");
        let pin = crate::translate::source_pin(&book.join("zh/book.md")).unwrap();
        assert!(
            pin.is_some_and(|pin| !pin.is_empty() && pin.chars().all(|c| c.is_ascii_hexdigit())),
            "{written}"
        );
        // The pin is the hash of the book, so the book still checks out.
        let loaded = load_book(&book).unwrap();
        let report = check_book(&loaded, &load_notes(&loaded).unwrap()).unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
    }

    #[test]
    fn a_quoted_translation_source_still_loads() {
        let book = book_with_translation();
        crate::translate::sync(&book, &["zh/book".into()]).unwrap();
        let written = fs::read_to_string(book.join("zh/book.md")).unwrap();
        let pin = written
            .lines()
            .find_map(|line| line.strip_prefix("source: #"))
            .expect("a source pin")
            .to_string();
        // A book written by 1035638 quotes the pin; it reads the same pin.
        let quoted = written.replace(&format!("source: #{pin}"), &format!("source: '#{pin}'"));
        fs::write(book.join("zh/book.md"), &quoted).unwrap();
        assert_eq!(
            crate::translate::source_pin(&book.join("zh/book.md"))
                .unwrap()
                .as_deref(),
            Some(pin.as_str())
        );
        // The pin still matches the book, so syncing changes nothing.
        crate::translate::sync(&book, &["zh/book".into()]).unwrap();
        let again = fs::read_to_string(book.join("zh/book.md")).unwrap();
        assert_eq!(again, quoted);
    }

    #[test]
    fn book_fields_read_raw_by_0_2_0_are_written_unquoted() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.title = "*Star".to_string();
        loaded.repo = "#repo".to_string();
        loaded.commit = "!commit".to_string();
        loaded.remote = Some("@remote".to_string());
        loaded.lang = "&lang".to_string();
        let rendered = render_book(&loaded);
        assert!(rendered.contains("title: *Star\n"), "{rendered}");
        assert!(rendered.contains("repo: #repo\n"), "{rendered}");
        assert!(rendered.contains("commit: !commit\n"), "{rendered}");
        assert!(rendered.contains("remote: @remote\n"), "{rendered}");
        assert!(rendered.contains("lang: &lang\n"), "{rendered}");
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.title, "*Star");
        assert_eq!(again.repo, "#repo");
        assert_eq!(again.commit, "!commit");
        assert_eq!(again.remote.as_deref(), Some("@remote"));
        assert_eq!(again.lang, "&lang");
    }

    #[test]
    fn a_book_with_quoted_fields_still_loads() {
        let (_repo, book) = fixture();
        fs::write(
            book.join(BOOK_FILE),
            "---\ntitle: '*Demo'\nrepo: '..'\ncommit: 'abc123'\nremote: '*git@example.com:a/b.git'\nlang: 'en'\nmax_pages: 4\npages:\n  - overview: Overview\nignore:\n---\n\n",
        )
        .unwrap();
        let loaded = load_book(&book).unwrap();
        assert_eq!(loaded.title, "*Demo");
        assert_eq!(loaded.repo, "..");
        assert_eq!(loaded.commit, "abc123");
        assert_eq!(loaded.remote.as_deref(), Some("*git@example.com:a/b.git"));
        assert_eq!(loaded.lang, "en");
    }

    #[test]
    fn translation_entries_are_never_quoted_as_a_whole() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.translations = vec!["*weird.md: README.md #abc1234".to_string()];
        let rendered = render_book(&loaded);
        assert!(
            rendered.contains("  - *weird.md: README.md #abc1234\n"),
            "{rendered}"
        );
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.translations, vec!["*weird.md: README.md #abc1234"]);
    }

    #[test]
    fn evidence_entries_are_never_quoted_as_a_whole() {
        let note = Note {
            page: "overview".to_string(),
            id: "loop".to_string(),
            title: "Loop".to_string(),
            order: None,
            evidence: vec![Evidence {
                target: EvidenceRef::File {
                    path: "*odd.rs".to_string(),
                    lines: None,
                },
                pin: Some("abc123".to_string()),
            }],
            body: "The loop runs.".to_string(),
        };
        let rendered = render_note(&note);
        assert!(rendered.contains("  - *odd.rs #abc123\n"), "{rendered}");
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(
            parsed.evidence[0].target,
            EvidenceRef::File {
                path: "*odd.rs".to_string(),
                lines: None,
            }
        );
        assert_eq!(parsed.evidence[0].pin.as_deref(), Some("abc123"));
    }

    #[test]
    fn a_note_with_a_quoted_evidence_entry_still_loads() {
        let (_repo, book) = fixture();
        let path = book.join("notes/overview/loop.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "---\ntitle: Loop\nevidence:\n  - 'src/core/engine.rs:2-4 #abc123'\n---\n\nThe loop runs.\n",
        )
        .unwrap();
        let loaded = load_book(&book).unwrap();
        let notes = load_notes(&loaded).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].evidence[0].target,
            EvidenceRef::File {
                path: "src/core/engine.rs".to_string(),
                lines: Some((2, 4)),
            }
        );
        assert_eq!(notes[0].evidence[0].pin.as_deref(), Some("abc123"));
    }

    #[test]
    fn parses_evidence_forms() {
        let e = parse_evidence("src/a.rs:3-9 #abc123").unwrap();
        assert_eq!(
            e.target,
            EvidenceRef::File {
                path: "src/a.rs".to_string(),
                lines: Some((3, 9))
            }
        );
        assert_eq!(e.pin.as_deref(), Some("abc123"));
        assert_eq!(parse_evidence("pr:12").unwrap().target, EvidenceRef::Pr(12));
        assert_eq!(
            parse_evidence("src/a.rs:4").unwrap().target,
            EvidenceRef::File {
                path: "src/a.rs".to_string(),
                lines: Some((4, 4))
            }
        );
        assert!(parse_evidence("../etc/passwd").is_err());
        assert!(parse_evidence("src/a.rs:9-3").is_err());
        assert!(parse_evidence("commit:xyz").is_err());
    }

    #[test]
    fn counts_words_and_cjk_without_code() {
        assert_eq!(word_units("the scheduler runs"), 3);
        assert_eq!(word_units("调度器负责批处理"), 4);
        assert_eq!(word_units("text\n```mermaid\ngraph TD; A-->B\n```\n"), 1);
    }

    #[test]
    fn note_round_trip_and_build() {
        let (_repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4", "README.md"],
            "`run` calls `step` once. See [[overview]].",
        )
        .unwrap();
        let loaded = load_book(&book).unwrap();
        let notes = load_notes(&loaded).unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].evidence.iter().all(|e| e.pin.is_some()));
        let report = check_book(&loaded, &notes).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        build(&book, None, false, None).unwrap();
        let page = fs::read_to_string(book.join("out/01-overview.md")).unwrap();
        assert!(page.contains("## Engine loop"));
        assert!(page.contains("[Overview](01-overview.md)"));
        assert!(page.contains("engine.rs:2-4"));
        assert!(book.join("out/all.md").exists());
        assert!(book.join("out/index.md").exists());
    }

    #[test]
    fn replace_keeps_omitted_fields_and_warns_on_unknown_symbols() {
        let (_repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        note_add(&NoteAddOptions {
            book: book.clone(),
            page: "overview".to_string(),
            id: "loop".to_string(),
            title: None,
            order: Some(5),
            evidence: Vec::new(),
            add_evidence: vec!["README.md".to_string()],
            body: String::new(),
            replace: true,
        })
        .unwrap();
        let loaded = load_book(&book).unwrap();
        let note = load_notes(&loaded).unwrap().remove(0);
        assert_eq!(note.title, "Engine loop");
        assert_eq!(note.order, Some(5));
        assert_eq!(note.body, "`run` calls `step`.");
        assert_eq!(note.evidence.len(), 2);

        let mut wrong = note.clone();
        wrong.body = "`run_engine_loop` lives in `src/core/missing.rs`; `step` exists.".to_string();
        let repo = loaded.repo_path();
        let warnings = symbol_warnings(&repo, &tracked_files(&repo).unwrap(), &wrong);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("run_engine_loop"));
        assert!(warnings[1].contains("missing.rs"));

        wrong.body = "`sgl.system/user/assistant` and `prefill|decode` are not paths.".to_string();
        let warnings = symbol_warnings(&repo, &tracked_files(&repo).unwrap(), &wrong);
        assert!(
            warnings.iter().all(|w| !w.contains("does not exist")),
            "{warnings:?}"
        );
    }

    #[test]
    fn rejects_missing_evidence_and_unknown_page() {
        let (_repo, book) = fixture();
        assert!(add(&book, "x", &["src/missing.rs"], "body").is_err());
        assert!(add(&book, "x", &["src/core/engine.rs:1-99"], "body").is_err());
        assert!(add(&book, "x", &[], "body").is_err());
        let result = note_add(&NoteAddOptions {
            book: book.clone(),
            page: "nope".to_string(),
            id: "x".to_string(),
            title: None,
            order: None,
            evidence: vec!["README.md".to_string()],
            add_evidence: Vec::new(),
            body: "body".to_string(),
            replace: false,
        });
        assert!(result.is_err());
    }

    #[test]
    fn detects_moved_and_stale_evidence() {
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();

        // Insert lines above: same content, new position.
        fs::write(
            repo.join("src/core/engine.rs"),
            "// header\n\nfn a() {}\nfn run() {\n    step();\n}\nfn z() {}\n",
        )
        .unwrap();
        let loaded = load_book(&book).unwrap();
        let report = check_book(&loaded, &load_notes(&loaded).unwrap()).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("moved to lines 4-6"))
        );
        pin(&book, &[]).unwrap();
        let note = &load_notes(&loaded).unwrap()[0];
        assert_eq!(
            note.evidence[0].target,
            EvidenceRef::File {
                path: "src/core/engine.rs".to_string(),
                lines: Some((4, 6))
            }
        );

        // Change the cited code: stale until reviewed and force-pinned.
        fs::write(
            repo.join("src/core/engine.rs"),
            "// header\n\nfn a() {}\nfn run() {\n    step_twice();\n}\nfn z() {}\n",
        )
        .unwrap();
        let report = check_book(&loaded, &load_notes(&loaded).unwrap()).unwrap();
        assert!(report.errors.iter().any(|e| e.contains("code changed")));
        pin(&book, &["overview/loop".to_string()]).unwrap();
        let report = check_book(&loaded, &load_notes(&loaded).unwrap()).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }

    #[test]
    fn buckets_split_largest_directories() {
        let mut files = BTreeMap::new();
        files.insert("a/big/x.rs".to_string(), 500);
        files.insert("a/big/y.rs".to_string(), 400);
        files.insert("a/small/z.rs".to_string(), 50);
        files.insert("a/top.rs".to_string(), 50);
        files.insert("b/w.rs".to_string(), 100);
        files.insert("README.md".to_string(), 10);
        let inventory = Inventory {
            total_lines: files.values().sum(),
            files,
        };
        let buckets = inventory.buckets(10);
        let labels: Vec<String> = buckets.iter().map(Bucket::label).collect();
        assert!(labels.contains(&"a/big/".to_string()), "{labels:?}");
        assert!(labels.contains(&"a/*".to_string()), "{labels:?}");
        assert!(labels.contains(&"(root files)".to_string()), "{labels:?}");
        let cited = BTreeSet::from(["a/big/x.rs".to_string()]);
        let big = buckets.iter().find(|b| b.dir == "a/big").unwrap();
        assert!(big.is_covered(&cited));
        let direct = buckets
            .iter()
            .find(|b| b.dir == "a" && b.kind == BucketKind::Files)
            .unwrap();
        assert!(!direct.is_covered(&cited));
    }

    #[test]
    fn buckets_group_small_children_into_other() {
        let mut files = BTreeMap::new();
        for i in 0..10 {
            files.insert(format!("big/m{i}/x.rs"), 100 + i);
        }
        files.insert("small.rs".to_string(), 5);
        let inventory = Inventory {
            total_lines: files.values().sum(),
            files,
        };
        let buckets = inventory.buckets(5);
        assert!(buckets.len() <= 5, "{buckets:?}");
        let other = buckets.iter().find(|b| b.kind == BucketKind::Rest).unwrap();
        assert_eq!(other.label(), "big/ (other)");
        assert!(other.contains("big/m0/x.rs"));
        assert!(!other.contains("big/m9/x.rs"));
        let total: usize = buckets.iter().map(|b| b.lines).sum();
        assert_eq!(total, inventory.total_lines);
    }

    #[test]
    fn build_keeps_foreign_files_and_refuses_collisions() {
        let (_repo, book) = fixture();
        add(&book, "loop", &["README.md"], "Demo.").unwrap();
        let out = book.join("shared");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("keep.txt"), "x").unwrap();
        build(&book, Some(out.clone()), false, None).unwrap();
        assert!(out.join("keep.txt").exists());
        assert!(out.join("01-overview.md").exists());
        // Rebuilding replaces only its own files.
        build(&book, Some(out.clone()), false, None).unwrap();
        assert!(out.join("keep.txt").exists());
        // A hand-written file with a generated name stops the build.
        let other = book.join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("all.md"), "mine").unwrap();
        assert!(build(&book, Some(other.clone()), false, None).is_err());
        assert_eq!(fs::read_to_string(other.join("all.md")).unwrap(), "mine");
    }

    #[test]
    fn build_refuses_legacy_marker_without_deleting_files() {
        let (_, book) = fixture();
        add(&book, "loop", &["README.md"], "Demo.").unwrap();
        let out = book.join("legacy");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join(BUILD_MARKER), "bogus").unwrap();
        fs::write(out.join("notes.md"), "hand-written").unwrap();
        let err = build(&book, Some(out.clone()), false, None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            format!(
                "{} is not a Codocia manifest; remove it or choose another --out",
                out.canonicalize().unwrap().join(BUILD_MARKER).display()
            )
        );
        assert_eq!(
            fs::read_to_string(out.join("notes.md")).unwrap(),
            "hand-written"
        );
        assert_eq!(fs::read_to_string(out.join(BUILD_MARKER)).unwrap(), "bogus");
    }

    #[test]
    fn build_skips_manifest_entries_outside_output() {
        use std::os::unix::fs::symlink;
        let (_, book) = fixture();
        add(&book, "loop", &["README.md"], "Demo.").unwrap();
        let out = book.join("output");
        let outside = temp_dir("outside");
        fs::create_dir_all(&out).unwrap();
        fs::write(outside.join("absolute.md"), "absolute").unwrap();
        fs::write(outside.join("x.md"), "nested").unwrap();
        symlink(&outside, out.join("sub")).unwrap();
        symlink(outside.join("x.md"), out.join("link.md")).unwrap();
        fs::write(
            out.join(BUILD_MARKER),
            format!(
                "{MANIFEST_HEADER}\n{}\nsub/x.md\nlink.md\n",
                outside.join("absolute.md").display()
            ),
        )
        .unwrap();
        build(&book, Some(out.clone()), false, None).unwrap();
        assert_eq!(
            fs::read_to_string(outside.join("absolute.md")).unwrap(),
            "absolute"
        );
        assert_eq!(fs::read_to_string(outside.join("x.md")).unwrap(), "nested");
        assert!(fs::symlink_metadata(out.join("link.md")).is_err());
    }

    #[test]
    fn all_md_demotes_only_headings_outside_fences() {
        let (_, book) = fixture();
        add(
            &book,
            "loop",
            &["README.md"],
            "### Heading\n\n```rust\n#include <x>\n#[derive(Debug)]\n# A heading in code\n```\n",
        )
        .unwrap();
        let out = book.join("output");
        build(&book, Some(out.clone()), false, None).unwrap();
        let all = fs::read_to_string(out.join("all.md")).unwrap();
        assert!(all.contains("#### Heading"), "{all}");
        assert!(
            all.contains("#include <x>\n#[derive(Debug)]\n# A heading in code"),
            "{all}"
        );
    }

    #[test]
    fn page_check_includes_missing_translations() {
        let (_, book) = fixture();
        add(&book, "loop", &["README.md"], "Demo.").unwrap();
        let book_md = book.join(BOOK_FILE);
        let text = fs::read_to_string(&book_md).unwrap();
        fs::write(
            &book_md,
            text.replacen("---\n\n", "langs:\n  - zh\n---\n\n", 1),
        )
        .unwrap();
        let loaded = load_book(&book).unwrap();
        let report = check_book(&loaded, &load_notes(&loaded).unwrap()).unwrap();
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.starts_with("zh/overview/loop: missing")),
            "{report:?}"
        );
        assert!(check(&book, Some("overview")).is_err());
    }

    #[test]
    fn root_only_repository_has_structure_area() {
        let root = temp_dir("root-only");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("main.rs"), "fn main() {}\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "initial",
            ],
        );
        let book = root.join("book");
        init(&InitOptions {
            repo,
            book: Some(book.clone()),
            title: Some("Root".into()),
            lang: "en".into(),
        })
        .unwrap();
        let loaded = load_book(&book).unwrap();
        let map = structure::build_map(&loaded, &[], 10).unwrap();
        assert_eq!(map.areas.len(), 1);
        assert_eq!(map.areas[0].bucket.label(), "(root files)");
        assert_eq!(map.areas[0].bucket.files, 1);
        let rendered = structure::render(&loaded, &map, |slug| slug.to_string());
        assert!(rendered.contains("(root files)"), "{rendered}");
        assert!(rendered.contains("1 file"), "{rendered}");
        status(&book, 10, None).unwrap();
    }

    #[test]
    fn accept_refuses_unverified_baselines_for_ranges_and_whole_files() {
        if let Some(book) = std::env::var_os("CODOCIA_UNVERIFIED_BASELINE_BOOK") {
            assert!(accept(Path::new(&book), &["overview/loop".into()], false).is_err());
            return;
        }
        for evidence in ["src/core/engine.rs:2-4", "src/core/engine.rs"] {
            let (repo, book) = fixture();
            let path = repo.join("src/core/engine.rs");
            fs::write(&path, "fn a() {}\nnew pinned\ntext\nhere\nfn z() {}\n").unwrap();
            add(&book, "loop", &[evidence], "Demo.").unwrap();
            fs::write(
                &path,
                "fn a() {}\nnew pinned\ntext changed\nhere\nfn z() {}\n",
            )
            .unwrap();
            let loaded = load_book(&book).unwrap();
            let note = note_path(&loaded, "overview", "loop");
            let before = fs::read(&note).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "book::tests::accept_refuses_unverified_baselines_for_ranges_and_whole_files",
                    "--nocapture",
                ])
                .env("CODOCIA_UNVERIFIED_BASELINE_BOOK", &book)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{stdout} {:?}", output.stderr);
            assert!(stdout.contains("before: (unavailable)"), "{stdout}");
            assert!(stdout.contains("cannot be compared with its pinned version; check the range, then accept --force"), "{stdout}");
            assert_eq!(fs::read(&note).unwrap(), before);
            accept(&book, &["overview/loop".into()], true).unwrap();
        }
    }

    #[test]
    fn build_inside_repo_uses_relative_links_and_readme() {
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "`run` calls `step`.",
        )
        .unwrap();
        let out = repo.join("docs");
        build(&book, Some(out.clone()), false, None).unwrap();
        assert!(out.join("README.md").exists());
        assert!(!out.join("index.md").exists());
        let page = fs::read_to_string(out.join("01-overview.md")).unwrap();
        assert!(page.contains("](../src/core/engine.rs#L2-L4)"), "{page}");
        let index = fs::read_to_string(out.join("README.md")).unwrap();
        assert!(
            !index.contains(&repo.to_string_lossy().to_string()),
            "{index}"
        );
    }

    #[test]
    fn web_url_normalizes_remotes() {
        assert_eq!(
            web_url("git@github.com:a/b.git").as_deref(),
            Some("https://github.com/a/b")
        );
        assert_eq!(
            web_url("https://github.com/a/b.git").as_deref(),
            Some("https://github.com/a/b")
        );
        assert_eq!(web_url("/local/path"), None);
    }

    // ------------------------------------------------------------------
    // Titles codocia 0.2.0 unquotes

    /// The value codocia 0.2.0 reads: it strips one pair of matching outer
    /// quotes and does not decode `''`.
    fn unquote_0_2_0(value: &str) -> String {
        let value = value.trim();
        for quote in ['"', '\''] {
            if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
                return value[1..value.len() - 1].to_string();
            }
        }
        value.to_string()
    }

    /// The value of a `key: value` line, as codocia 0.2.0 reads it.
    fn scalar_0_2_0(line: &str) -> String {
        let (_, value) = line.split_once(':').expect("a `key: value` line");
        unquote_0_2_0(value)
    }

    /// The value of a `key: value` line, as codocia 0.2.0 reads it without
    /// unquoting.
    fn raw_scalar_0_2_0(line: &str) -> String {
        let (_, value) = line.split_once(':').expect("a `key: value` line");
        value.trim().to_string()
    }

    /// The title of a `- slug: title` line, as codocia 0.2.0 reads it.
    fn page_title_0_2_0(line: &str) -> String {
        let item = line.trim_start().strip_prefix("- ").expect("a list item");
        let (_, title) = item.split_once(':').expect("a `slug: title` entry");
        unquote_0_2_0(title)
    }

    /// The line of a rendered file that holds the title of a page.
    fn page_line<'a>(text: &'a str, slug: &str) -> &'a str {
        text.lines()
            .find(|line| line.trim_start().starts_with(&format!("- {slug}:")))
            .unwrap_or_else(|| panic!("a page entry for `{slug}` in {text}"))
    }

    /// The line of a rendered file that holds a title.
    fn title_line(text: &str) -> &str {
        text.lines()
            .find(|line| line.starts_with("title:"))
            .unwrap_or_else(|| panic!("a title in {text}"))
    }

    #[test]
    fn a_note_title_with_a_colon_is_quoted() {
        let note = note_with_title("Overview: details");
        let rendered = render_note(&note);
        assert!(
            rendered.contains("title: \"Overview: details\"\n"),
            "{rendered}"
        );
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(parsed.title, "Overview: details");
        assert_eq!(scalar_0_2_0(title_line(&rendered)), "Overview: details");
    }

    #[test]
    fn a_note_title_ending_with_a_colon_is_quoted() {
        let note = note_with_title("Overview:");
        let rendered = render_note(&note);
        assert!(rendered.contains("title: \"Overview:\"\n"), "{rendered}");
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(parsed.title, "Overview:");
        assert_eq!(scalar_0_2_0(title_line(&rendered)), "Overview:");
    }

    #[test]
    fn a_page_title_with_a_hash_is_quoted() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.pages[0].title = "a # b".to_string();
        let rendered = render_book(&loaded);
        assert!(rendered.contains("- overview: \"a # b\"\n"), "{rendered}");
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.pages[0].slug, "overview");
        assert_eq!(again.pages[0].title, "a # b");
        assert_eq!(page_title_0_2_0(page_line(&rendered, "overview")), "a # b");
    }

    #[test]
    fn an_ignore_glob_with_an_apostrophe_is_written_in_double_quotes() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.ignore = vec!["*it's*".to_string()];
        let rendered = render_book(&loaded);
        assert!(rendered.contains("- \"*it's*\"\n"), "{rendered}");
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.ignore, vec!["*it's*"]);
        let line = rendered
            .lines()
            .find(|line| line.trim_start().starts_with("- \"*it's*\""))
            .expect("the ignore glob");
        let item = line.trim_start().strip_prefix("- ").unwrap();
        assert_eq!(unquote_0_2_0(item), "*it's*");
    }

    #[test]
    fn a_value_with_a_double_quote_is_written_in_single_quotes() {
        let note = note_with_title("*a\"b");
        let rendered = render_note(&note);
        assert!(rendered.contains("title: '*a\"b'\n"), "{rendered}");
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(parsed.title, "*a\"b");
        assert_eq!(scalar_0_2_0(title_line(&rendered)), "*a\"b");
    }

    #[test]
    fn a_title_with_a_backslash_is_written_in_single_quotes() {
        let note = note_with_title("Overview: \\q");
        let rendered = render_note(&note);
        // A double-quoted scalar has YAML escapes, so `\\q` would be
        // invalid there; a single-quoted one has none.
        assert!(rendered.contains("title: 'Overview: \\q'\n"), "{rendered}");
        // The current reader reads the backslash back.
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(parsed.title, "Overview: \\q");
        // 0.2.0 reads it back too.
        assert_eq!(scalar_0_2_0(title_line(&rendered)), "Overview: \\q");
    }

    #[test]
    fn a_title_with_an_escape_sequence_keeps_the_backslash() {
        let note = note_with_title("Overview: \\n");
        let rendered = render_note(&note);
        assert!(rendered.contains("title: 'Overview: \\n'\n"), "{rendered}");
        // `\\n` stays a backslash and an `n`, it is not a newline.
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(parsed.title, "Overview: \\n");
        assert_eq!(scalar_0_2_0(title_line(&rendered)), "Overview: \\n");
    }

    #[test]
    fn a_value_with_a_quote_and_a_backslash_doubles_the_single_quote() {
        let note = note_with_title("*it's \\q");
        let rendered = render_note(&note);
        assert!(rendered.contains("title: '*it''s \\q'\n"), "{rendered}");
        // The current reader decodes the doubled quote and keeps the
        // backslash.
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(parsed.title, "*it's \\q");
        // 0.2.0 does not decode the doubled quote: it keeps it.
        assert_eq!(scalar_0_2_0(title_line(&rendered)), "*it''s \\q");
    }

    #[test]
    fn a_value_with_both_quotes_doubles_the_single_quote() {
        let note = note_with_title("*it's a \"b\"*");
        let rendered = render_note(&note);
        assert!(
            rendered.contains("title: '*it''s a \"b\"*'\n"),
            "{rendered}"
        );
        // The current reader decodes the doubled quote.
        let parsed = parse_note("overview", "loop", &rendered).unwrap();
        assert_eq!(parsed.title, "*it's a \"b\"*");
        // 0.2.0 does not decode it: it keeps the doubled quote.
        assert_eq!(scalar_0_2_0(title_line(&rendered)), "*it''s a \"b\"*");
    }

    #[test]
    fn a_book_title_with_a_colon_is_written_unquoted() {
        let (_repo, book) = fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.title = "Codocia: x".to_string();
        let rendered = render_book(&loaded);
        // 0.2.0 reads the book title without unquoting it, so it is written
        // as it is.
        assert!(rendered.contains("title: Codocia: x\n"), "{rendered}");
        fs::write(book.join(BOOK_FILE), &rendered).unwrap();
        let again = load_book(&book).unwrap();
        assert_eq!(again.title, "Codocia: x");
        assert_eq!(raw_scalar_0_2_0(title_line(&rendered)), "Codocia: x");
    }

    #[test]
    fn a_translated_book_quotes_its_titles() {
        let (_repo, book) = fixture();
        let book_md = book.join(BOOK_FILE);
        let text = fs::read_to_string(&book_md).unwrap();
        fs::write(
            &book_md,
            text.replacen("---\n\n", "langs:\n  - zh\n---\n\n", 1),
        )
        .unwrap();
        fs::create_dir_all(book.join("zh")).unwrap();
        fs::write(
            book.join("zh/book.md"),
            "---\ntitle: 演示: 概览\npages:\n  - overview: 概览: 详情\n---\n\n译文。\n",
        )
        .unwrap();
        crate::translate::sync(&book, &["zh/book".into()]).unwrap();
        let written = fs::read_to_string(book.join("zh/book.md")).unwrap();
        assert!(written.contains("title: \"演示: 概览\""), "{written}");
        assert!(written.contains("- overview: \"概览: 详情\""), "{written}");
        // The current reader reads both titles back.
        let (front, _) = split_front(&written).unwrap();
        let front = Front::parse(front);
        assert_eq!(
            front.scalar_value("title").map(|value| unquote(&value)),
            Some("演示: 概览".to_string())
        );
        assert_eq!(
            front.list("pages"),
            ["overview: \"概览: 详情\"".to_string()].as_slice()
        );
        // 0.2.0 reads both titles back too.
        assert_eq!(scalar_0_2_0(title_line(&written)), "演示: 概览");
        assert_eq!(
            page_title_0_2_0(page_line(&written, "overview")),
            "概览: 详情"
        );
    }

    #[test]
    fn a_translated_note_title_with_a_colon_is_quoted() {
        let (_repo, book) = fixture();
        add(&book, "loop", &["src/core/engine.rs:1-2"], "The loop runs.").unwrap();
        let book_md = book.join(BOOK_FILE);
        let text = fs::read_to_string(&book_md).unwrap();
        fs::write(
            &book_md,
            text.replacen("---\n\n", "langs:\n  - zh\n---\n\n", 1),
        )
        .unwrap();
        fs::create_dir_all(book.join("zh/overview")).unwrap();
        fs::write(
            book.join("zh/overview/loop.md"),
            "---\ntitle: 循环: 细节\n---\n\n循环运行。\n",
        )
        .unwrap();
        crate::translate::sync(&book, &["zh/overview/loop".into()]).unwrap();
        let written = fs::read_to_string(book.join("zh/overview/loop.md")).unwrap();
        assert!(written.contains("title: \"循环: 细节\""), "{written}");
        let (front, _) = split_front(&written).unwrap();
        let front = Front::parse(front);
        assert_eq!(
            front.scalar_value("title").map(|value| unquote(&value)),
            Some("循环: 细节".to_string())
        );
        assert_eq!(scalar_0_2_0(title_line(&written)), "循环: 细节");
    }

    #[test]
    fn quoted_values_round_trip_through_the_reader() {
        for value in [
            "Overview: details",
            "a # b",
            "Trailing:",
            "*it's*",
            "*a\"b",
            "*it's a \"b\"*",
            "'quoted'",
            "\"quoted\"",
            "Overview: \\q",
            "Overview: \\n",
            "*it's \\q",
        ] {
            assert!(needs_quotes(value), "{value}");
            let written = yaml_value(value);
            // The reader decodes both quote styles.
            assert_eq!(unquote(&written), value, "{written}");
            assert_eq!(quoted_item(&written).as_deref(), Some(value), "{written}");
        }
    }

    // -----------------------------------------------------------------------
    // SKILL.md

    /// A book with one note on the `overview` page.
    fn skill_fixture() -> (PathBuf, PathBuf) {
        let (repo, book) = fixture();
        add(
            &book,
            "loop",
            &["src/core/engine.rs:2-4"],
            "The `run` function calls `step`.",
        )
        .unwrap();
        (repo, book)
    }

    /// Adds a note with the given body to the `overview` page.
    fn add_body(book: &Path, id: &str, body: &str) {
        note_add(&NoteAddOptions {
            book: book.to_path_buf(),
            page: "overview".to_string(),
            id: id.to_string(),
            title: Some("Engine loop".to_string()),
            order: None,
            evidence: vec!["src/core/engine.rs:2-4".to_string()],
            add_evidence: Vec::new(),
            body: body.to_string(),
            replace: true,
        })
        .unwrap();
    }

    /// Builds a skill file and returns its lines.
    fn skill_lines(book: &Path, out: &Path) -> Vec<String> {
        build(book, Some(out.to_path_buf()), false, Some("overview")).unwrap();
        let text = fs::read_to_string(out.join("skill/repo/SKILL.md")).unwrap();
        text.lines().map(str::to_string).collect()
    }

    /// The link targets of a Markdown text.
    fn link_targets(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find("](") {
            let after = &rest[start + 2..];
            let Some(end) = after.find(')') else { break };
            out.push(after[..end].to_string());
            rest = &after[end + 1..];
        }
        out
    }

    #[test]
    fn build_with_skill_page_writes_a_skill_file() {
        let (_repo, book) = skill_fixture();
        let out = temp_dir("skill-out");
        build(&book, Some(out.clone()), false, Some("overview")).unwrap();
        let path = out.join("skill/repo/SKILL.md");
        assert!(path.is_file(), "{}", path.display());
        let text = fs::read_to_string(&path).unwrap();
        let expected = "\
---
name: Demo
description: \"The `run` function calls `step`.\"
version: 0.1.0
tags: [repo]
---

# Demo

## Engine loop

The `run` function calls `step`.

<sub>Sources: `src/core/engine.rs:2-4`</sub>
";
        assert_eq!(text, expected);
        // The page itself is unchanged by the flag.
        let page = fs::read_to_string(out.join("01-overview.md")).unwrap();
        assert!(page.contains("## Engine loop"), "{page}");
        // The file is recorded in the build marker like every other output.
        let marker = fs::read_to_string(out.join(BUILD_MARKER)).unwrap();
        assert!(marker.contains("skill/repo/SKILL.md"), "{marker}");
    }

    #[test]
    fn skill_frontmatter_is_skrun_safe() {
        let (_repo, book) = skill_fixture();
        let out = temp_dir("skill-front");
        let lines = skill_lines(&book, &out);
        assert_eq!(lines[0], "---");
        let end = lines.iter().skip(1).position(|l| l == "---").unwrap() + 1;
        assert!(end > 1);
        for line in &lines[1..end] {
            let (key, value) = line.split_once(": ").expect(line);
            assert!(!key.is_empty() && !key.contains(' '), "{line}");
            assert!(!value.contains('\n'), "{line}");
            assert!(!value.trim().is_empty(), "{line}");
        }
        let keys: Vec<&str> = lines[1..end]
            .iter()
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(keys, vec!["name", "description", "version", "tags"]);
        assert_eq!(lines[end - 1], "tags: [repo]");
    }

    /// A minimal book for the pure helpers.
    fn plain_book(repo: &Path, title: &str) -> Book {
        Book {
            dir: repo.to_path_buf(),
            title: title.to_string(),
            repo: repo.to_string_lossy().to_string(),
            out: None,
            commit: String::new(),
            remote: None,
            lang: "en".to_string(),
            max_pages: 8,
            max_page_words: 900,
            max_note_words: 180,
            max_words: 0,
            pages: vec![Page {
                slug: "overview".to_string(),
                title: "Overview".to_string(),
            }],
            ignore: Vec::new(),
            langs: Vec::new(),
            translations: Vec::new(),
            intro: String::new(),
            link_base: None,
        }
    }

    #[test]
    fn skill_id_normalizes_the_repository_name() {
        let root = temp_dir("skill-id");
        let repo = root.join("My Repo (v2)!");
        fs::create_dir_all(&repo).unwrap();
        let book = plain_book(&repo, "Demo");
        assert_eq!(skill_id(&book), "my-repo--v2");
        // Nothing left after the trimming means the plain name.
        let empty = root.join("-.-");
        fs::create_dir_all(&empty).unwrap();
        let book = plain_book(&empty, "Demo");
        assert_eq!(skill_id(&book), "skill");
    }

    #[test]
    fn skill_name_quotes_a_title_that_would_not_parse() {
        let root = temp_dir("skill-name");
        let book = plain_book(&root, "Demo: the book");
        assert_eq!(skill_name(&book), "\"Demo: the book\"");
        let book = plain_book(&root, "Demo #1");
        assert_eq!(skill_name(&book), "\"Demo #1\"");
        // A title that is only quotes holds no quote of its own.
        let book = plain_book(&root, "'Demo'");
        assert_eq!(skill_name(&book), "Demo");
        let book = plain_book(&root, "\"Demo\"");
        assert_eq!(skill_name(&book), "Demo");
        let book = plain_book(&root, "Demo");
        assert_eq!(skill_name(&book), "Demo");
        let book = plain_book(&root, "Demo\nBook");
        assert_eq!(skill_name(&book), "Demo Book");
        // A double quote becomes a single quote, and a quote at the end of
        // the name goes: the readers strip it. The old expectation,
        // `The 'x'`, held the bug: the readers read that name differently.
        let book = plain_book(&root, "The \"x\"");
        assert_eq!(skill_name(&book), "The 'x");
        // A title that starts like a comment or like a list is quoted.
        let book = plain_book(&root, "#hash title");
        assert_eq!(skill_name(&book), "\"#hash title\"");
        let book = plain_book(&root, "[x] y");
        assert_eq!(skill_name(&book), "\"[x] y\"");
        // A pair of backslashes would be unescaped by one reader.
        let book = plain_book(&root, "a\\\\b");
        assert_eq!(skill_name(&book), "a\\b");
        // A quoted name holds no quote of its own: the readers would strip
        // it and read another name.
        let book = plain_book(&root, "#Demo'");
        assert_eq!(skill_name(&book), "\"#Demo\"");
        let book = plain_book(&root, "#Demo\"");
        assert_eq!(skill_name(&book), "\"#Demo\"");
        // A title that holds nothing but quotes falls back to the skill id.
        let book = plain_book(&root, "\"\"");
        assert_eq!(skill_name(&book), skill_id(&book));
        let book = plain_book(&root, "\"");
        assert_eq!(skill_name(&book), skill_id(&book));
    }

    #[test]
    fn skill_scalar_never_starts_or_ends_with_a_quote() {
        // A value that starts or ends with a quote is read differently by
        // the two readers: one strips the quote, the other keeps it.
        for input in [
            "\"a\"",
            "'a'",
            "a\"",
            "\"a",
            "x 'y'",
            "x \"y\"",
            "'\"a\"'",
            "a\\\\",
            "a\\",
            "\\\\a\\\\b",
            "  \"  spaced  \"  ",
            "it's",
        ] {
            let value = skill_scalar(input);
            assert!(!value.starts_with(['\'', '"']), "{input:?} -> {value:?}");
            assert!(!value.ends_with(['\'', '"']), "{input:?} -> {value:?}");
            assert!(!value.contains('"'), "{input:?} -> {value:?}");
            assert!(!value.contains("\\\\"), "{input:?} -> {value:?}");
            assert!(!value.ends_with('\\'), "{input:?} -> {value:?}");
        }
        // A quote inside a value stays.
        assert_eq!(skill_scalar("it's"), "it's");
    }

    #[test]
    fn skill_version_reads_cargo_package() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-cargo");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 1.2.3".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_reads_cargo_workspace_package() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"2.0.1\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-workspace");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 2.0.1".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_reads_pyproject() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("pyproject.toml"),
            "[project]\nname = \"demo\"\nversion = \"3.4.5\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-pyproject");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 3.4.5".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_reads_package_json() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("package.json"),
            "{\n  \"name\": \"demo\",\n  \"version\": \"4.5.6\",\n  \"dependencies\": {\n    \"version\": \"9.9.9\"\n  }\n}\n",
        )
        .unwrap();
        let out = temp_dir("skill-npm");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 4.5.6".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_skips_a_version_that_needs_quoting() {
        let (repo, book) = skill_fixture();
        // The escape in the JSON file is a newline: the version would end
        // the line of the frontmatter.
        fs::write(
            repo.join("package.json"),
            "{\n  \"name\": \"demo\",\n  \"version\": \"1.0\\nrequires: [x]\",\n}\n",
        )
        .unwrap();
        let out = temp_dir("skill-npm-unsafe");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 0.1.0".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_skips_an_empty_version() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-empty-version");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 0.1.0".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_keeps_a_prerelease_version() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.2.3-beta.1+build\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-prerelease");
        let lines = skill_lines(&book, &out);
        assert!(
            lines.contains(&"version: 1.2.3-beta.1+build".to_string()),
            "{lines:?}"
        );
    }

    #[test]
    fn skill_version_falls_back_to_the_default() {
        let (repo, book) = skill_fixture();
        // A workspace version is not a version of its own.
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion.workspace = true\n",
        )
        .unwrap();
        let out = temp_dir("skill-fallback");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 0.1.0".to_string()), "{lines:?}");
    }

    #[test]
    fn toml_headers_allow_a_comment_spaces_and_quotes() {
        assert_eq!(toml_section_name("[package] # main"), Some("package"));
        assert_eq!(toml_section_name("[ package ]"), Some("package"));
        // A quoted key is part of the name, brackets inside it included.
        assert_eq!(
            toml_section_name("[target.'cfg(unix)'.dependencies.libc]"),
            Some("target.'cfg(unix)'.dependencies.libc")
        );
        // An array table is not a table header, and nothing but a comment
        // may follow the closing bracket.
        assert_eq!(toml_section_name("[[bin]]"), None);
        assert_eq!(toml_section_name("[package] and more"), None);
    }

    #[test]
    fn toml_brackets_inside_a_string_or_a_comment_do_not_count() {
        // A bracket inside a string value or inside a comment is part of it.
        assert_eq!(toml_bracket_depth("description = \"a ] b [\"", 0), 0);
        assert_eq!(toml_bracket_depth("keywords = [ # [x]", 0), 1);
        // A line of an array value keeps the value open.
        assert_eq!(toml_bracket_depth("[\"a\", \"b\"],", 1), 1);
        assert_eq!(toml_bracket_depth("]", 1), 0);
        // A multi-line inline table keeps a value open like an array does.
        assert_eq!(toml_bracket_depth("meta = {", 0), 1);
        assert_eq!(toml_bracket_depth("  version = \"9.9\",", 1), 1);
        assert_eq!(toml_bracket_depth("}", 1), 0);
    }

    #[test]
    fn skill_version_skips_a_version_below_a_header_with_a_quoted_key() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion.workspace = true\n\n[target.'cfg(unix)'.dependencies.libc]\nversion = \"0.2\"\n\n[workspace.package]\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-quoted-key");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 1.0.0".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_skips_a_version_of_an_array_table() {
        let (repo, book) = skill_fixture();
        // An array table never opens a section that can hold the version.
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"demo\"\n\n[[bin]]\nname = \"demo\"\nversion = \"9.9\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-array-table");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 0.1.0".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_reads_a_version_after_a_value_with_brackets() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"demo\"\ndescription = \"a ] b [\"\nversion = \"3.0.0\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-brackets-in-value");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 3.0.0".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_reads_a_version_after_a_multi_line_array() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nkeywords = [\n[\"a\", \"b\"],\n]\nversion = \"1.4.2\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-toml-array");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 1.4.2".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_reads_a_version_before_a_multi_line_string() {
        let (repo, book) = skill_fixture();
        // The reader gives up at the first multi-line string, so a version
        // before it is still read.
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nversion = \"1.0.0\"\ndescription = \"\"\"\nA [x\n\"\"\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-toml-before-string");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 1.0.0".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_gives_up_at_a_one_line_multi_line_string() {
        let (repo, book) = skill_fixture();
        // A multi-line string that opens and closes on one line was misread as
        // an open one, and a version of another table was used instead.
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\ndescription = \"\"\"It's a demo\"\"\"\n[dependencies.foo]\ndescription = \"\"\"It's foo\"\"\"\nversion = \"6.6.6\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-toml-one-line-string");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 0.1.0".to_string()), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("6.6.6")), "{lines:?}");
    }

    #[test]
    fn skill_version_gives_up_at_a_multi_line_string_in_pyproject() {
        let (repo, book) = skill_fixture();
        // The same shape in pyproject.toml: no version of another table.
        fs::write(
            repo.join("pyproject.toml"),
            "[project]\ndescription = \"\"\"It's a demo\"\"\"\n[tool.a]\ndescription = \"\"\"It's foo\"\"\"\nversion = \"6.6.6\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-pyproject-one-line-string");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 0.1.0".to_string()), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("6.6.6")), "{lines:?}");
    }

    #[test]
    fn skill_version_gives_up_at_a_header_inside_a_multi_line_string() {
        let (repo, book) = skill_fixture();
        // A header inside a literal multi-line string is not a header, so the
        // version after it is a guess.
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\ndescription = '''\n[not a header]\n'''\nversion = \"2.0.0\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-toml-header");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 0.1.0".to_string()), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("2.0.0")), "{lines:?}");
    }

    #[test]
    fn skill_version_reads_a_version_after_a_multi_line_inline_table() {
        let (repo, book) = skill_fixture();
        // A multi-line inline table is a continuation, and its own version is
        // not the version of the section.
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nmeta = {\n  version = \"9.9\",\n}\nversion = \"3.0.0\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-toml-inline-table");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 3.0.0".to_string()), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("9.9")), "{lines:?}");
    }

    #[test]
    fn skill_version_skips_a_stray_bracket_and_an_array_element() {
        let (repo, book) = skill_fixture();
        // A stray closing bracket may not take the scanner below the value,
        // where the elements of the array would look like headers.
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nbroken = ]\nkeywords = [\n[\"a\"],\n]\nversion = \"3.0.0\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-toml-stray");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 3.0.0".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_version_reads_a_commented_header() {
        let (repo, book) = skill_fixture();
        fs::write(
            repo.join("Cargo.toml"),
            "[ package ] # main\nname = \"demo\"\nversion = \"1.4.2\"\n",
        )
        .unwrap();
        let out = temp_dir("skill-toml-comment");
        let lines = skill_lines(&book, &out);
        assert!(lines.contains(&"version: 1.4.2".to_string()), "{lines:?}");
    }

    #[test]
    fn skill_description_takes_the_first_sentence() {
        let (_repo, book) = skill_fixture();
        add_body(
            &book,
            "loop",
            "The `run` function calls `step`. Then it waits for the next event.",
        );
        let out = temp_dir("skill-sentence");
        let lines = skill_lines(&book, &out);
        // The note with the lowest order comes first, and only its first
        // sentence describes the skill.
        let description = lines
            .iter()
            .find(|l| l.starts_with("description: "))
            .unwrap();
        assert_eq!(
            description,
            "description: \"The `run` function calls `step`.\""
        );
        // The body keeps the whole note.
        let text = fs::read_to_string(out.join("skill/repo/SKILL.md")).unwrap();
        assert!(text.contains("Then it waits for the next event."), "{text}");
    }

    #[test]
    fn skill_description_replaces_page_links_and_quotes() {
        let (_repo, book) = skill_fixture();
        let loaded = load_book(&book).unwrap();
        let notes = load_notes(&loaded).unwrap();
        let page = loaded.pages[0].clone();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "The `run` function calls `step`.");
        // A page link becomes the page title, a quote becomes a single quote.
        let mut notes = notes.clone();
        notes[0].body = "See [[overview]] and say \"hello\" now.".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "See Overview and say 'hello' now.");
        // A link with its own text keeps that text.
        notes[0].body = "See [[overview|the page]] now.".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "See the page now.");
        // A long first sentence is cut at a word boundary.
        notes[0].body = format!("{}.", "word ".repeat(100));
        let text = skill_description(&loaded, &notes, &page);
        assert!(text.chars().count() <= 300, "{}", text.chars().count());
        assert!(text.ends_with("word"), "{text}");
        // A note without any sentence keeps its first paragraph.
        notes[0].body =
            "First paragraph without a terminator\n\nSecond paragraph without one".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "First paragraph without a terminator");
        // A sentence of a later paragraph does not describe the skill: the
        // first paragraph without a sentence end does.
        notes[0].body = "Intro without a full stop\n\nSecond paragraph. More.".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "Intro without a full stop");
        // An empty note falls back to the book and page title.
        notes[0].body = String::new();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "Demo — Overview");
    }

    #[test]
    fn skill_description_is_safe_to_quote() {
        let (_repo, book) = skill_fixture();
        let loaded = load_book(&book).unwrap();
        let notes = load_notes(&loaded).unwrap();
        let page = loaded.pages[0].clone();
        let mut notes = notes.clone();
        // A leading quote would end the quoted value for one of the readers.
        notes[0].body = "\"run\" starts here.".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "run' starts here.");
        // A pair of backslashes would be unescaped by one of the readers.
        notes[0].body = "C:\\\\path\\\\x".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "C:\\path\\x");
        // A sentence that ends with a backslash loses it.
        notes[0].body = "The path ends with a backslash\\".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "The path ends with a backslash");
        // A cut never leaves a trailing quote: the readers would strip it.
        notes[0].body = format!("{}' tail words and more", "a".repeat(299));
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "a".repeat(299));
        assert!(!text.ends_with('\''), "{text}");
        // The same for a sentence that ends with a quote.
        notes[0].body = "He said \"hi\"".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert!(!text.ends_with('\''), "{text}");
        // A description without a terminator keeps its whole paragraph, and
        // the quote at its end goes: the readers strip it.
        notes[0].body = "It prints \"done\"".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "It prints 'done");
    }

    #[test]
    fn skill_description_skips_a_leading_code_block() {
        let (_repo, book) = skill_fixture();
        let loaded = load_book(&book).unwrap();
        let notes = load_notes(&loaded).unwrap();
        let page = loaded.pages[0].clone();
        let mut notes = notes.clone();
        // The code block goes, and the paragraph after it describes the
        // skill.
        notes[0].body =
            "```rust\nlet x = 1;\n```\n\nThe engine waits for the next event".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "The engine waits for the next event");
        // With a terminator, the first sentence of that paragraph is used.
        notes[0].body = "```rust\nlet x = 1;\n```\n\nThe engine waits. Then it stops.".to_string();
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text, "The engine waits.");
    }

    #[test]
    fn first_paragraph_skips_leading_blank_lines() {
        assert_eq!(first_paragraph("\n\n  \nHello\n\nWorld"), "Hello");
    }

    #[test]
    fn cut_at_word_cuts_a_text_without_spaces() {
        let text = "字".repeat(400);
        let cut = cut_at_word(&text, 300);
        assert_eq!(cut.chars().count(), 300);
        // The description of a note without any space keeps its first 300
        // characters.
        let (_repo, book) = skill_fixture();
        let loaded = load_book(&book).unwrap();
        let notes = load_notes(&loaded).unwrap();
        let page = loaded.pages[0].clone();
        let mut notes = notes.clone();
        notes[0].body = "字".repeat(400);
        let text = skill_description(&loaded, &notes, &page);
        assert_eq!(text.chars().count(), 300);
    }

    #[test]
    fn skill_links_are_absolute() {
        let (_repo, book) = skill_fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.remote = Some("https://github.com/acme/demo".to_string());
        fs::write(book.join(BOOK_FILE), render_book(&loaded)).unwrap();
        let out = temp_dir("skill-remote");
        build(&book, Some(out.clone()), false, Some("overview")).unwrap();
        let text = fs::read_to_string(out.join("skill/repo/SKILL.md")).unwrap();
        let links = link_targets(&text);
        assert!(!links.is_empty(), "{text}");
        for link in &links {
            assert!(
                link.starts_with("https://github.com/acme/demo/"),
                "{link} in {text}"
            );
        }
        assert!(!text.contains("](../"), "{text}");
        // The page itself uses the same permalinks.
        let page = fs::read_to_string(out.join("01-overview.md")).unwrap();
        assert!(
            page.contains("https://github.com/acme/demo/blob/"),
            "{page}"
        );
    }

    #[test]
    fn skill_permalinks_carry_a_short_commit() {
        let (repo, book) = skill_fixture();
        let mut loaded = load_book(&book).unwrap();
        loaded.remote = Some("https://github.com/acme/demo".to_string());
        fs::write(book.join(BOOK_FILE), render_book(&loaded)).unwrap();
        // A note that cites the pinned commit of the repository.
        let commit = String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let commit = commit.trim().to_string();
        assert_eq!(commit.len(), 40, "{commit}");
        add(
            &book,
            "history",
            &[&format!("commit:{commit}")],
            "The loop landed in one commit.",
        )
        .unwrap();
        let out = temp_dir("skill-short-commit");
        build(&book, Some(out.clone()), false, Some("overview")).unwrap();
        let text = fs::read_to_string(out.join("skill/repo/SKILL.md")).unwrap();
        let links = link_targets(&text);
        assert!(!links.is_empty(), "{text}");
        let short = &commit[..12];
        for link in &links {
            assert!(
                link.contains(&format!("/blob/{short}/"))
                    || link.contains(&format!("/commit/{short}")),
                "{link} in {text}"
            );
        }
        // No line of the skill file holds a 40-hex run: a skill store hides
        // such lines as possible tokens, so the commit evidence is short too.
        for line in text.lines() {
            let mut run = 0;
            for ch in line.chars().chain([' ']) {
                run = if ch.is_ascii_hexdigit() { run + 1 } else { 0 };
                assert!(run < 40, "{line} in {text}");
            }
        }
        // The page itself keeps the full commit, in a permalink and in the
        // commit evidence of the note.
        let page = fs::read_to_string(out.join("01-overview.md")).unwrap();
        assert!(
            page.contains(&format!("https://github.com/acme/demo/blob/{commit}/")),
            "{page}"
        );
        assert!(
            page.contains(&format!("https://github.com/acme/demo/commit/{commit}")),
            "{page}"
        );
    }

    #[test]
    fn skill_without_remote_has_no_relative_links() {
        let (_repo, book) = skill_fixture();
        let out = temp_dir("skill-plain");
        build(&book, Some(out.clone()), false, Some("overview")).unwrap();
        let text = fs::read_to_string(out.join("skill/repo/SKILL.md")).unwrap();
        assert!(!text.contains("](../"), "{text}");
        assert!(text.contains("`src/core/engine.rs:2-4`"), "{text}");
    }

    #[test]
    fn skill_page_must_exist() {
        let (_repo, book) = skill_fixture();
        // The output folder does not exist yet: a wrong page writes nothing.
        let out = temp_dir("skill-missing").join("out");
        let error = build(&book, Some(out.clone()), false, Some("nope"))
            .unwrap_err()
            .to_string();
        assert_eq!(error, "--skill-page: no page \"nope\" in book.md");
        assert!(!out.exists(), "{}", out.display());
    }

    #[test]
    fn build_without_the_skill_page_writes_no_skill_file() {
        let (_repo, book) = skill_fixture();
        let out = temp_dir("skill-none");
        build(&book, Some(out.clone()), false, None).unwrap();
        assert!(!out.join("skill").exists(), "{}", out.display());
        let marker = fs::read_to_string(out.join(BUILD_MARKER)).unwrap();
        assert!(!marker.contains("skill/"), "{marker}");
        let mut names: Vec<String> = fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                ".codocia-build".to_string(),
                "01-overview.md".to_string(),
                "all.md".to_string(),
                "index.md".to_string(),
                "structure.md".to_string(),
            ]
        );
        // The flag only adds the skill file: the pages are the same.
        let with = temp_dir("skill-none-with");
        build(&book, Some(with.clone()), false, Some("overview")).unwrap();
        assert_eq!(
            fs::read(out.join("01-overview.md")).unwrap(),
            fs::read(with.join("01-overview.md")).unwrap()
        );
        assert_eq!(
            fs::read(out.join("all.md")).unwrap(),
            fs::read(with.join("all.md")).unwrap()
        );
    }

    #[test]
    fn a_stale_skill_file_is_removed() {
        let (_repo, book) = skill_fixture();
        let out = temp_dir("skill-stale");
        build(&book, Some(out.clone()), false, Some("overview")).unwrap();
        assert!(out.join("skill/repo/SKILL.md").is_file());
        build(&book, Some(out.clone()), false, None).unwrap();
        assert!(!out.join("skill/repo/SKILL.md").exists());
        // The empty folder of the skill goes with the file.
        assert!(
            !out.join("skill").exists(),
            "{}",
            out.join("skill").display()
        );
        let marker = fs::read_to_string(out.join(BUILD_MARKER)).unwrap();
        assert!(!marker.contains("skill/"), "{marker}");
    }
}
