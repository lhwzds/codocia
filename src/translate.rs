//! Translations kept in step with their source.
//!
//! Every translation records the hash of the source text it was made from.
//! `check` fails when the source has changed since (the translation drifted)
//! and warns when code, external links or headings differ between the two.
//! Codocia never translates; an agent edits the translation, then runs
//! `codocia accept <target>` to record that it matches the source again.
//!
//! Two kinds of pairs:
//! - book translations, for each language in `langs:`: `<book>/<lang>/book.md`
//!   (title, page titles, intro) and `<book>/<lang>/<page>/<id>.md` per note;
//! - repository files listed in `translations:` as `target: source #hash`,
//!   for example `README_CN.md: README.md #…`.

use crate::book::{
    self, Book, Front, Note, Report, code_spans, render_book, render_site, short_hash, split_front,
    unquote, validate_slug,
};
use crate::structure::StructureMap;
use crate::write_file;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const RESERVED: &[&str] = &["notes", "out"];

pub(crate) fn parse_langs(items: &[String], primary: Option<&str>) -> Result<Vec<String>> {
    let primary = primary.unwrap_or("en");
    let mut langs = Vec::new();
    for item in items {
        let lang = unquote(item);
        validate_slug(&lang, "language")?;
        if lang == primary || RESERVED.contains(&lang.as_str()) || langs.contains(&lang) {
            bail!("book.md: `langs` cannot list `{lang}`");
        }
        langs.push(lang);
    }
    Ok(langs)
}

fn lang_name(lang: &str) -> &str {
    match lang {
        "en" => "English",
        "zh" => "中文",
        "ja" => "日本語",
        "ko" => "한국어",
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Translation files

struct Translated {
    title: String,
    source: Option<String>,
    /// Page titles, only in `<lang>/book.md`.
    pages: Vec<(String, String)>,
    body: String,
}

fn read_translated(path: &Path) -> Result<Option<Translated>> {
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(None);
    };
    let (front, body) =
        split_front(&content).with_context(|| format!("{} has no frontmatter", path.display()))?;
    let front = Front::parse(front);
    let pages = front
        .list("pages")
        .iter()
        .filter_map(|item| item.split_once(':'))
        .map(|(slug, title)| (slug.trim().to_string(), unquote(title)))
        .collect();
    Ok(Some(Translated {
        title: unquote(front.scalar("title").unwrap_or("")),
        source: front
            .scalar("source")
            .map(|s| s.trim_start_matches('#').to_string()),
        pages,
        body: body.trim().to_string(),
    }))
}

fn write_translated(root: &Path, path: &Path, t: &Translated) -> Result<()> {
    let mut out = format!("---\ntitle: {}\n", t.title);
    if let Some(source) = &t.source {
        out.push_str(&format!("source: #{source}\n"));
    }
    if !t.pages.is_empty() {
        out.push_str("pages:\n");
        for (slug, title) in &t.pages {
            out.push_str(&format!("  - {slug}: {title}\n"));
        }
    }
    out.push_str("---\n\n");
    out.push_str(&t.body);
    out.push('\n');
    book::safe_target(root, path)?;
    write_file(path.to_path_buf(), out.as_bytes())
}

fn book_file(book: &Book, lang: &str) -> PathBuf {
    book.dir.join(lang).join("book.md")
}

fn note_file(book: &Book, lang: &str, note: &Note) -> PathBuf {
    book.dir
        .join(lang)
        .join(&note.page)
        .join(format!("{}.md", note.id))
}

/// What a translation of the book file must match: title, page titles, intro.
fn book_source(book: &Book) -> String {
    let mut text = format!("{}\n", book.title);
    for page in &book.pages {
        text.push_str(&format!("{}: {}\n", page.slug, page.title));
    }
    text.push_str(&book.intro);
    text
}

fn note_source(note: &Note) -> String {
    format!("{}\n{}", note.title, note.body)
}

fn hash(text: &str) -> String {
    short_hash(text.trim().as_bytes())
}

/// A repository file pair from `translations:`.
struct FilePair {
    target: String,
    source: String,
    pin: Option<String>,
}

fn parse_pair(raw: &str) -> Result<FilePair> {
    let (target, rest) = raw
        .split_once(':')
        .with_context(|| format!("book.md: translation `{raw}` must be `target: source`"))?;
    let mut parts = rest.split_whitespace();
    let source = parts
        .next()
        .with_context(|| format!("book.md: translation `{raw}` has no source"))?;
    Ok(FilePair {
        target: unquote(target),
        source: unquote(source),
        pin: parts.next().map(|p| p.trim_start_matches('#').to_string()),
    })
}

/// Repository files in `translations:`, sources and targets alike.
pub(crate) fn paths(book: &Book) -> Vec<String> {
    book.translations
        .iter()
        .filter_map(|raw| parse_pair(raw).ok())
        .flat_map(|pair| [pair.target, pair.source])
        .collect()
}

fn render_pair(pair: &FilePair) -> String {
    match &pair.pin {
        Some(pin) => format!("{}: {} #{pin}", pair.target, pair.source),
        None => format!("{}: {}", pair.target, pair.source),
    }
}

// ---------------------------------------------------------------------------
// check

pub(crate) fn check(book: &Book, notes: &[Note], report: &mut Report) -> Result<()> {
    for lang in &book.langs {
        let name = format!("{lang}/book");
        match read_translated(&book_file(book, lang))? {
            None => report.errors.push(format!(
                "{name}: missing {lang}/book.md (translated title, page titles and intro)"
            )),
            Some(t) => {
                drift(report, &name, &t.source, &hash(&book_source(book)));
                for page in &book.pages {
                    if !t.pages.iter().any(|(slug, _)| slug == &page.slug) {
                        report
                            .errors
                            .push(format!("{name}: no title for page `{}`", page.slug));
                    }
                }
                parity(report, &name, &book.intro, &t.body);
            }
        }
        for note in notes {
            let name = format!("{lang}/{}/{}", note.page, note.id);
            match read_translated(&note_file(book, lang, note))? {
                None => report.errors.push(format!(
                    "{name}: missing translation of {}/{}",
                    note.page, note.id
                )),
                Some(t) => {
                    drift(report, &name, &t.source, &hash(&note_source(note)));
                    parity(report, &name, &note.body, &t.body);
                }
            }
        }
        for orphan in orphans(book, lang, notes)? {
            report.errors.push(format!(
                "{lang}/{orphan}: no source note {orphan}; delete the translation"
            ));
        }
    }
    let repo = book.repo_path();
    for raw in &book.translations {
        let pair = parse_pair(raw)?;
        let Ok(source) = fs::read_to_string(repo.join(&pair.source)) else {
            report
                .errors
                .push(format!("{}: source {} not found", pair.target, pair.source));
            continue;
        };
        let Ok(target) = fs::read_to_string(repo.join(&pair.target)) else {
            report.errors.push(format!(
                "{}: missing translation of {}",
                pair.target, pair.source
            ));
            continue;
        };
        drift(report, &pair.target, &pair.pin, &hash(&source));
        parity(report, &pair.target, &source, &target);
    }
    Ok(())
}

fn drift(report: &mut Report, name: &str, recorded: &Option<String>, current: &str) {
    match recorded {
        Some(pin) if pin == current => {}
        Some(_) => report.errors.push(format!(
            "{name}: source changed since it was translated; update the translation, then `codocia accept {name}`"
        )),
        None => report.errors.push(format!(
            "{name}: never synced; check it matches the source, then `codocia accept {name}`"
        )),
    }
}

/// Translation files whose source note no longer exists.
fn orphans(book: &Book, lang: &str, notes: &[Note]) -> Result<Vec<String>> {
    let root = book.dir.join(lang);
    let mut found = Vec::new();
    let Ok(pages) = fs::read_dir(&root) else {
        return Ok(found);
    };
    for page in pages {
        let page = page?;
        if !page.file_type()?.is_dir() {
            continue;
        }
        let slug = page.file_name().to_string_lossy().to_string();
        for file in fs::read_dir(page.path())? {
            let path = file?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if !notes.iter().any(|n| n.page == slug && n.id == id) {
                found.push(format!("{slug}/{id}"));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Mechanical differences a translation should not have: code, external
/// links and the heading outline stay the same in every language.
fn parity(report: &mut Report, name: &str, source: &str, target: &str) {
    let a = Shape::of(source);
    let b = Shape::of(target);
    if a.headings != b.headings {
        report.warnings.push(format!(
            "{name}: {} heading(s), source has {}",
            b.headings, a.headings
        ));
    }
    if a.blocks != b.blocks {
        report.warnings.push(format!(
            "{name}: code blocks differ from the source (comments are ignored)"
        ));
    }
    for (what, x, y) in [
        ("inline code", &a.spans, &b.spans),
        ("link", &a.urls, &b.urls),
    ] {
        let missing = difference(x, y);
        let extra = difference(y, x);
        if !missing.is_empty() || !extra.is_empty() {
            let mut parts = Vec::new();
            if !missing.is_empty() {
                parts.push(format!("missing {}", shown(&missing)));
            }
            if !extra.is_empty() {
                parts.push(format!("extra {}", shown(&extra)));
            }
            report
                .warnings
                .push(format!("{name}: {what} differs: {}", parts.join("; ")));
        }
    }
}

struct Shape {
    headings: usize,
    blocks: Vec<Vec<String>>,
    spans: BTreeMap<String, usize>,
    urls: BTreeMap<String, usize>,
}

impl Shape {
    fn of(text: &str) -> Self {
        let mut shape = Shape {
            headings: 0,
            blocks: Vec::new(),
            spans: BTreeMap::new(),
            urls: BTreeMap::new(),
        };
        let mut block: Option<Vec<String>> = None;
        let mut paragraph = String::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                shape.add_spans(&paragraph);
                paragraph.clear();
                match block.take() {
                    Some(lines) => shape.blocks.push(lines),
                    None => block = Some(Vec::new()),
                }
                continue;
            }
            if let Some(lines) = &mut block {
                let code = strip_comment(trimmed);
                if !code.is_empty() && code.is_ascii() {
                    lines.push(code.to_string());
                }
                continue;
            }
            if trimmed.is_empty() {
                shape.add_spans(&paragraph);
                paragraph.clear();
            } else {
                paragraph.push_str(trimmed);
                paragraph.push(' ');
            }
            if line.starts_with('#') {
                shape.headings += 1;
            }
            for url in external_urls(line) {
                *shape.urls.entry(url).or_default() += 1;
            }
        }
        shape.add_spans(&paragraph);
        shape
    }

    /// Inline code of one paragraph; a span may wrap across lines. Spans
    /// carrying prose (non-ASCII) are allowed to be translated.
    fn add_spans(&mut self, paragraph: &str) {
        for span in code_spans(paragraph) {
            let span = span.trim();
            if span.is_ascii() && !span.is_empty() {
                *self.spans.entry(span.to_string()).or_default() += 1;
            }
        }
    }
}

/// Code without its comment, which a translation may translate.
fn strip_comment(line: &str) -> &str {
    if ["#", "//", "--", ";"].iter().any(|c| line.starts_with(c)) {
        return "";
    }
    let cut = [" #", " //", " --"]
        .iter()
        .filter_map(|c| line.find(c))
        .min()
        .unwrap_or(line.len());
    line[..cut].trim_end()
}

fn external_urls(line: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("http") {
        let tail = &rest[start..];
        if !(tail.starts_with("http://") || tail.starts_with("https://")) {
            rest = &tail[4..];
            continue;
        }
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '"' | '\'' | '`' | ']'))
            .unwrap_or(tail.len());
        urls.push(tail[..end].trim_end_matches(['.', ',', ';']).to_string());
        rest = &tail[end..];
    }
    urls
}

fn difference(a: &BTreeMap<String, usize>, b: &BTreeMap<String, usize>) -> Vec<String> {
    a.iter()
        .filter(|(key, n)| b.get(*key).copied().unwrap_or(0) < **n)
        .map(|(key, _)| key.clone())
        .collect()
}

fn shown(items: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut text = items
        .iter()
        .take(SHOWN)
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > SHOWN {
        text.push_str(&format!(" +{}", items.len() - SHOWN));
    }
    text
}

// ---------------------------------------------------------------------------
// sync

/// Records that each target matches its current source. A target is a
/// language (`zh`: its whole book translation), `zh/book`, `zh/<page>/<id>`,
/// or a repository file listed in `translations:`.
pub(crate) fn is_target(book: &Book, notes: &[Note], target: &str) -> Result<bool> {
    let target = target.trim_end_matches(".md");
    let (lang, rest) = target.split_once('/').unwrap_or((target, ""));
    if book.langs.iter().any(|l| l == lang) {
        return Ok(rest.is_empty()
            || rest == "book"
            || notes.iter().any(|n| rest == format!("{}/{}", n.page, n.id)));
    }
    for raw in &book.translations {
        let pair = parse_pair(raw)?;
        if pair.target.trim_end_matches(".md") == target {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn sync(book_dir: &Path, targets: &[String]) -> Result<()> {
    if targets.is_empty() {
        bail!(
            "name what to sync: a language, `<lang>/book`, `<lang>/<page>/<id>` or a file from `translations:`"
        );
    }
    let mut book = book::load_book(book_dir)?;
    let notes = book::load_notes(&book)?;
    let mut synced = 0;
    for target in targets {
        let target = target.trim_end_matches(".md");
        let (lang, rest) = target.split_once('/').unwrap_or((target, ""));
        if book.langs.iter().any(|l| l == lang) {
            if rest.is_empty() || rest == "book" {
                synced += sync_file(
                    &book.dir,
                    &book_file(&book, lang),
                    &hash(&book_source(&book)),
                )?;
            }
            for note in &notes {
                if rest.is_empty() || rest == format!("{}/{}", note.page, note.id) {
                    synced += sync_file(
                        &book.dir,
                        &note_file(&book, lang, note),
                        &hash(&note_source(note)),
                    )?;
                }
            }
            continue;
        }
        let repo = book.repo_path();
        let mut found = false;
        for raw in book.translations.iter_mut() {
            let mut pair = parse_pair(raw)?;
            if pair.target != target && pair.target.trim_end_matches(".md") != target {
                continue;
            }
            let source = fs::read_to_string(repo.join(&pair.source))
                .with_context(|| format!("read {}", pair.source))?;
            pair.pin = Some(hash(&source));
            *raw = render_pair(&pair);
            found = true;
            synced += 1;
        }
        if !found {
            bail!("`{target}` is neither a language in `langs:` nor a file in `translations:`");
        }
    }
    book::safe_target(&book.dir, &book.dir.join("book.md"))?;
    write_file(book.dir.join("book.md"), render_book(&book).as_bytes())?;
    println!("synced {synced} translation(s)");
    Ok(())
}

fn sync_file(root: &Path, path: &Path, current: &str) -> Result<usize> {
    let Some(mut t) = read_translated(path)? else {
        bail!(
            "{} does not exist; write the translation first",
            path.display()
        );
    };
    if t.source.as_deref() == Some(current) {
        return Ok(0);
    }
    t.source = Some(current.to_string());
    write_translated(root, path, &t)?;
    Ok(1)
}

// ---------------------------------------------------------------------------
// build

/// Renders each translation into `<lang>/` beside the primary files and puts
/// a language switch under every page title.
pub(crate) fn add_languages(
    book: &Book,
    notes: &[Note],
    map: &StructureMap,
    files: &mut Vec<(String, String)>,
    in_repo: bool,
) -> Result<()> {
    if book.langs.is_empty() {
        return Ok(());
    }
    let mut all_langs = vec![book.lang.clone()];
    all_langs.extend(book.langs.iter().cloned());
    for (name, content) in files.iter_mut() {
        *content = with_switch(content, &book.lang, &all_langs, &book.lang, name);
    }
    for lang in &book.langs {
        let mut tbook = book.clone();
        tbook.lang = lang.clone();
        tbook.link_base = book.link_base.as_ref().map(|b| format!("../{b}"));
        if let Some(t) = read_translated(&book_file(book, lang))? {
            if !t.title.is_empty() {
                tbook.title = t.title;
            }
            tbook.intro = t.body;
            for page in &mut tbook.pages {
                if let Some((_, title)) = t.pages.iter().find(|(slug, _)| slug == &page.slug) {
                    page.title = title.clone();
                }
            }
        }
        let mut tnotes = notes.to_vec();
        for note in &mut tnotes {
            if let Some(t) = read_translated(&note_file(book, lang, note))? {
                note.title = t.title;
                note.body = t.body;
            }
        }
        for (name, content) in render_site(&tbook, &tnotes, Some(map)) {
            let name = if in_repo && name == "index.md" {
                "README.md".to_string()
            } else {
                name
            };
            let content = with_switch(&content, lang, &all_langs, &book.lang, &name);
            files.push((format!("{lang}/{name}"), content));
        }
    }
    Ok(())
}

/// Inserts `English · [中文](zh/x.md)` after the first H1 of a page.
fn with_switch(
    content: &str,
    current: &str,
    langs: &[String],
    primary: &str,
    name: &str,
) -> String {
    let links: Vec<String> = langs
        .iter()
        .map(|lang| {
            if lang == current {
                format!("**{}**", lang_name(lang))
            } else {
                let path = match (current == primary, lang == primary) {
                    (true, _) => format!("{lang}/{name}"),
                    (false, true) => format!("../{name}"),
                    (false, false) => format!("../{lang}/{name}"),
                };
                format!("[{}]({path})", lang_name(lang))
            }
        })
        .collect();
    let line = format!("<sub>{}</sub>\n\n", links.join(" · "));
    match content.find("\n# ") {
        Some(start) => {
            let end = content[start + 1..]
                .find('\n')
                .map(|i| start + 1 + i + 1)
                .unwrap_or(content.len());
            format!(
                "{}\n{line}{}",
                &content[..end],
                content[end..].trim_start_matches('\n')
            )
        }
        None => format!("{line}{content}"),
    }
}
