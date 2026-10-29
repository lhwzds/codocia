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
use std::path::{Path, PathBuf};
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
    pub commit: String,
    pub remote: Option<String>,
    pub lang: String,
    pub max_pages: usize,
    pub max_page_words: usize,
    pub max_note_words: usize,
    pub pages: Vec<Page>,
    pub ignore: Vec<String>,
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
    pub book: PathBuf,
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

pub fn init(options: &InitOptions) -> Result<()> {
    let repo = options
        .repo
        .canonicalize()
        .with_context(|| format!("repository not found: {}", options.repo.display()))?;
    let commit = current_commit(&repo)?;
    fs::create_dir_all(&options.book)
        .with_context(|| format!("create {}", options.book.display()))?;
    let book_dir = options.book.canonicalize()?;
    if book_dir.starts_with(&repo) && book_dir != repo {
        // Allowed, but the notes are then part of the repository's worktree.
        eprintln!("note: the book lives inside the repository; add it to .gitignore if unwanted");
    }
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
    let book = Book {
        dir: book_dir.clone(),
        title,
        repo: repo.to_string_lossy().to_string(),
        commit,
        remote: git_remote_url(&repo),
        lang: options.lang.clone(),
        max_pages: 8,
        max_page_words: 900,
        max_note_words: 180,
        pages: vec![Page {
            slug: "overview".to_string(),
            title: overview.to_string(),
        }],
        ignore: Vec::new(),
        intro: String::new(),
        link_base: None,
    };
    write_file(book_file, render_book(&book).as_bytes())?;
    fs::create_dir_all(book_dir.join(NOTES_DIR))?;
    println!(
        "created book at {} for {} @ {}",
        book_dir.display(),
        book.repo,
        short(&book.commit)
    );
    println!("next: edit {BOOK_FILE} (pages, ignore), then run `codocia status`");
    Ok(())
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
        if !code_moved(&repo, &book.commit, &head) {
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
    println!("  total: {} notes, {} words", notes.len(), total_words);

    let inventory = Inventory::scan(&repo, &book.ignore)?;
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

fn code_spans(body: &str) -> Vec<String> {
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
    let path = note_path(&book, page, id);
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
            write_file(
                note_path(&book, &note.page, &note.id),
                render_note(&note).as_bytes(),
            )?;
            println!("updated {}/{}", note.page, note.id);
            changed += 1;
        }
    }
    let head = current_commit(&repo)?;
    if code_moved(&repo, &book.commit, &head) && force.is_empty() {
        println!(
            "repo HEAD {} differs from the pinned commit {}",
            short(&head),
            short(&book.commit)
        );
    }
    println!("updated {changed} note(s)");
    Ok(())
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
    let head = current_commit(&book.repo_path())?;
    book.commit = head;
    write_file(book.dir.join(BOOK_FILE), render_book(&book).as_bytes())?;
    println!("book now pinned to {}", short(&book.commit));
    Ok(())
}

/// Validates the book; with `page`, reports only that page's problems.
pub fn check(book_dir: &Path, page: Option<&str>) -> Result<()> {
    let book = load_book(book_dir)?;
    let notes = load_notes(&book)?;
    let mut report = check_book(&book, &notes)?;
    if let Some(page) = page {
        let own = |m: &String| {
            m.starts_with(&format!("{page}/")) || m.contains(&format!("page `{page}`"))
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

pub fn build(book_dir: &Path, out: Option<PathBuf>, force: bool) -> Result<()> {
    let book = load_book(book_dir)?;
    let notes = load_notes(&book)?;
    let report = check_book(&book, &notes)?;
    if !report.errors.is_empty() {
        print_report(&report);
        if !force {
            bail!("check failed; fix the errors or pass --force");
        }
    }
    let mut book = book;
    let out = out.unwrap_or_else(|| book.dir.join("out"));
    fs::create_dir_all(&out).with_context(|| format!("create {}", out.display()))?;
    let out = out.canonicalize()?;
    // Inside the repository: link sources relatively and name the index
    // README.md so the folder renders on GitHub and in any checkout.
    let repo = book.repo_path().canonicalize()?;
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
    let ignored = |path: &str| book.ignore.iter().any(|g| glob_matches(g, Path::new(path)));
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
    pub fn repo_path(&self) -> PathBuf {
        let path = PathBuf::from(&self.repo);
        if path.is_absolute() {
            path
        } else {
            self.dir.join(path)
        }
    }

    fn label(&self, en: &'static str) -> &'static str {
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
        let (slug, title) = item
            .split_once(':')
            .map(|(s, t)| (s.trim(), t.trim()))
            .unwrap_or((item.trim(), item.trim()));
        validate_slug(slug, "page slug")?;
        if slug == STRUCTURE_SLUG {
            bail!(
                "{BOOK_FILE}: `{STRUCTURE_SLUG}` is reserved for the generated code structure page"
            );
        }
        if pages.iter().any(|p: &Page| p.slug == slug) {
            bail!("{BOOK_FILE}: duplicate page `{slug}`");
        }
        pages.push(Page {
            slug: slug.to_string(),
            title: unquote(title),
        });
    }
    Ok(Book {
        title: front.scalar("title").unwrap_or("Documentation").to_string(),
        repo: front
            .scalar("repo")
            .with_context(|| format!("{BOOK_FILE}: `repo` is required"))?
            .to_string(),
        commit: front.scalar("commit").unwrap_or("").to_string(),
        remote: front
            .scalar("remote")
            .map(str::to_string)
            .filter(|r| !r.is_empty()),
        lang: front.scalar("lang").unwrap_or("en").to_string(),
        max_pages: number("max_pages", 8)?,
        max_page_words: number("max_page_words", 900)?,
        max_note_words: number("max_note_words", 180)?,
        pages,
        ignore: front.list("ignore").iter().map(|g| unquote(g)).collect(),
        intro: body.trim().to_string(),
        link_base: None,
        dir,
    })
}

fn render_book(book: &Book) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {}\n", book.title));
    out.push_str(&format!("repo: {}\n", book.repo));
    out.push_str(&format!("commit: {}\n", book.commit));
    if let Some(remote) = &book.remote {
        out.push_str(&format!("remote: {remote}\n"));
    }
    out.push_str(&format!("lang: {}\n", book.lang));
    out.push_str(&format!("max_pages: {}\n", book.max_pages));
    out.push_str(&format!("max_page_words: {}\n", book.max_page_words));
    out.push_str(&format!("max_note_words: {}\n", book.max_note_words));
    out.push_str("pages:\n");
    for page in &book.pages {
        out.push_str(&format!("  - {}: {}\n", page.slug, page.title));
    }
    out.push_str("ignore:\n");
    for glob in &book.ignore {
        out.push_str(&format!("  - {glob}\n"));
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
        .map(|raw| parse_evidence(raw))
        .collect::<Result<Vec<_>>>()?;
    Ok(Note {
        page: page.to_string(),
        id: id.to_string(),
        title: unquote(front.scalar("title").unwrap_or("")),
        order,
        evidence,
        body: body.trim().to_string(),
    })
}

fn render_note(note: &Note) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {}\n", note.title));
    if let Some(order) = note.order {
        out.push_str(&format!("order: {order}\n"));
    }
    out.push_str("evidence:\n");
    for item in &note.evidence {
        out.push_str(&format!("  - {}\n", render_evidence(item)));
    }
    out.push_str("---\n\n");
    out.push_str(note.body.trim());
    out.push('\n');
    out
}

fn note_path(book: &Book, page: &str, id: &str) -> PathBuf {
    book.dir.join(NOTES_DIR).join(page).join(format!("{id}.md"))
}

// ---------------------------------------------------------------------------
// Minimal frontmatter: `key: value` scalars and `key:` + `  - item` lists.

struct Front {
    scalars: BTreeMap<String, String>,
    lists: BTreeMap<String, Vec<String>>,
}

impl Front {
    fn parse(text: &str) -> Self {
        let mut scalars = BTreeMap::new();
        let mut lists: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
                    current = Some(key);
                } else {
                    scalars.insert(key, value.to_string());
                    current = None;
                }
            }
        }
        Self { scalars, lists }
    }

    fn scalar(&self, key: &str) -> Option<&str> {
        self.scalars.get(key).map(String::as_str)
    }

    fn list(&self, key: &str) -> &[String] {
        self.lists.get(key).map(Vec::as_slice).unwrap_or(&[])
    }
}

fn split_front(content: &str) -> Option<(&str, &str)> {
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

fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn validate_slug(value: &str, what: &str) -> Result<()> {
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
            let ok = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if ok {
                EvidenceState::Ok
            } else {
                EvidenceState::BadRef("commit not found in repository".to_string())
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

fn short_hash(bytes: &[u8]) -> String {
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
                    .push(format!("{name}: unpinned evidence {shown}; run `codocia pin`")),
                EvidenceState::Missing => match &item.target {
                    EvidenceRef::File { path, .. } if renames.contains_key(path.as_str()) => {
                        report.errors.push(format!(
                            "{name}: {shown} was renamed to {}; run `codocia pin`",
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
                    "{name}: code changed under {shown}; review the note, then `codocia pin --force {name}`"
                )),
                EvidenceState::Moved(a, b) => report.warnings.push(format!(
                    "{name}: {shown} moved to lines {a}-{b}; run `codocia pin`"
                )),
                EvidenceState::BadRef(reason) => {
                    report.errors.push(format!("{name}: {shown}: {reason}"))
                }
            }
        }
    }
    Ok(report)
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

fn tracked_files(repo: &Path) -> Result<Vec<String>> {
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

/// True when commits after the pinned one changed anything besides
/// Codocia's own generated docs (directories holding a build manifest), so
/// committing the built docs into the repository is not reported as drift.
fn code_moved(repo: &Path, commit: &str, head: &str) -> bool {
    if head == commit || head.is_empty() {
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
            !Path::new(path.as_ref())
                .ancestors()
                .skip(1)
                .any(|dir| repo.join(dir).join(BUILD_MARKER).is_file())
        })
}

/// Files renamed between the pinned commit and the working tree
/// (old path -> new path), as git's rename detection sees them.
fn git_renames(repo: &Path, commit: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if commit.is_empty() {
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
    let previous: Vec<String> = match fs::read_to_string(&marker) {
        Ok(text) if text.starts_with(MANIFEST_HEADER) => {
            text.lines().skip(1).map(str::to_string).collect()
        }
        Ok(_) => {
            // Legacy marker: the whole directory was generated.
            fs::remove_dir_all(out).with_context(|| format!("remove {}", out.display()))?;
            fs::create_dir_all(out)?;
            return Ok(());
        }
        Err(_) => Vec::new(),
    };
    for name in names {
        if out.join(name).exists() && !previous.contains(name) {
            bail!(
                "{} already exists and was not generated by `codocia build`; move it or choose another --out",
                out.join(name).display()
            );
        }
    }
    for name in &previous {
        let path = out.join(name);
        if !name.contains("..") && path.is_file() {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    Ok(())
}

fn page_file(book: &Book, slug: &str) -> String {
    if slug == STRUCTURE_SLUG {
        return format!("{STRUCTURE_SLUG}.md");
    }
    let index = book.pages.iter().position(|p| p.slug == slug).unwrap_or(0);
    format!("{:02}-{slug}.md", index + 1)
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
                "由 Codocia 从代码生成：功能区、依赖、讲解页"
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
            Path::new(&book.repo)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        }),
        short(&book.commit)
    ));
    files.push(("index.md".to_string(), index));

    for page in &book.pages {
        let mut body = String::new();
        for note in notes.iter().filter(|n| n.page == page.slug) {
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
        let demoted = all_body
            .lines()
            .map(|l| {
                if l.starts_with('#') {
                    format!("#{l}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
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
        let all_body = structure::render(book, map, |slug| {
            format!("[{}](#{slug})", titles.get(slug).unwrap_or(&slug))
        })
        .lines()
        .map(|l| {
            if l.starts_with('#') {
                format!("#{l}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
        all.push_str(&format!(
            "<a id=\"{STRUCTURE_SLUG}\"></a>\n\n## {structure_title}\n\n{}\n\n",
            all_body.trim_end()
        ));
    }
    files.push(("all.md".to_string(), all.trim_end().to_string() + "\n"));
    files
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
            book: book.clone(),
            title: Some("Demo".to_string()),
            lang: "en".to_string(),
        })
        .unwrap();
        (repo, book)
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
    fn pin_follows_renamed_files() {
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
        pin(&book, &[]).unwrap();
        let report = check_book(&loaded, &load_notes(&loaded).unwrap()).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
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

        build(&book, None, false).unwrap();
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
        build(&book, Some(out.clone()), false).unwrap();
        assert!(out.join("keep.txt").exists());
        assert!(out.join("01-overview.md").exists());
        // Rebuilding replaces only its own files.
        build(&book, Some(out.clone()), false).unwrap();
        assert!(out.join("keep.txt").exists());
        // A hand-written file with a generated name stops the build.
        let other = book.join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("all.md"), "mine").unwrap();
        assert!(build(&book, Some(other.clone()), false).is_err());
        assert_eq!(fs::read_to_string(other.join("all.md")).unwrap(), "mine");
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
        build(&book, Some(out.clone()), false).unwrap();
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
}
