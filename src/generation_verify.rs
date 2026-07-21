//! Deterministic verification for staged, Codocia-managed Markdown.
//!
//! The verifier reads from an isolated staging tree whose layout mirrors the
//! target workspace (for example, `<staging>/docs/index.md`). It never mutates
//! the staging tree or the source workspace.

use crate::generation_model::{
    BrokenLink, DocPlan, DocumentationDepth, GENERATION_SCHEMA_VERSION, Inventory, PageKind,
    PageSpec, PageVerification, VerificationCheck, VerificationIssue, VerificationReport,
    VerificationSeverity, VerificationStatus, gfm_slug, validate_docs_page_path,
};
use crate::integrity::sha256_fingerprint;
use anyhow::Result;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Depth is a deterministic publication contract, not only a prompt hint.
/// Index pages have a smaller floor because they primarily provide navigation;
/// every other planned page must carry the requested explanatory density.
pub(crate) fn minimum_body_text_chars(depth: DocumentationDepth, kind: PageKind) -> usize {
    match (depth, kind == PageKind::Index) {
        (DocumentationDepth::Compact, true) => 32,
        (DocumentationDepth::Compact, false) => 32,
        (DocumentationDepth::Standard, true) => 300,
        (DocumentationDepth::Standard, false) => 800,
        (DocumentationDepth::Deep, true) => 600,
        (DocumentationDepth::Deep, false) => 1_200,
    }
}

const PLACEHOLDER_MARKERS: &[&str] = &[
    "todo",
    "tbd",
    "fixme",
    "placeholder",
    "coming soon",
    "lorem ipsum",
    "待补充",
    "待完善",
    "占位",
];

/// Verify the complete staged Markdown set described by `plan`.
///
/// `staged_docs_root` is the staged equivalent of `plan.docs_root` itself.
/// Source content is re-hashed from `workspace` so a change during generation
/// blocks the publish step.
pub fn verify_staged_docs(
    workspace: impl AsRef<Path>,
    staged_docs_root: impl AsRef<Path>,
    inventory: &Inventory,
    plan: &DocPlan,
) -> Result<VerificationReport> {
    let workspace = workspace.as_ref();
    let staged_docs_root = staged_docs_root.as_ref();
    let mut issues = Vec::new();
    let mut broken_links = Vec::new();
    let mut repairs = plan
        .pages
        .iter()
        .map(|page| (page.id.clone(), Vec::<String>::new()))
        .collect::<BTreeMap<_, _>>();

    let mut model_errors = Vec::new();
    if let Err(errors) = plan.validate_against_inventory(inventory) {
        model_errors.extend(errors);
    }
    model_errors.sort();
    model_errors.dedup();
    for message in &model_errors {
        push_issue(
            &mut issues,
            "invalid_generation_model",
            None,
            message.clone(),
            false,
        );
    }

    let (workspace_matches_inventory, changed_source_paths) =
        workspace_matches_inventory(workspace, inventory);
    let fingerprint_matches = !plan.source_fingerprint.trim().is_empty()
        && plan.source_fingerprint == inventory.source_fingerprint
        && workspace_matches_inventory;
    if !fingerprint_matches {
        push_issue(
            &mut issues,
            "source_fingerprint_mismatch",
            None,
            format!(
                "source fingerprint mismatch: plan=`{}`, inventory=`{}`, changed workspace paths={:?}",
                plan.source_fingerprint, inventory.source_fingerprint, changed_source_paths
            ),
            false,
        );
    }

    let mut all_pages_exist = true;
    let mut all_frontmatter_valid = true;
    let mut all_content_valid = true;
    let mut all_links_valid = true;
    let planned_staged_pages = plan
        .pages
        .iter()
        .filter_map(|page| page.path.strip_prefix(&plan.docs_root).ok())
        .map(Path::to_path_buf)
        .collect::<BTreeSet<_>>();
    let staged_markdown_pages = collect_staged_markdown_paths(staged_docs_root)?;
    let unexpected_staged_pages = staged_markdown_pages
        .difference(&planned_staged_pages)
        .cloned()
        .collect::<Vec<_>>();
    for path in &unexpected_staged_pages {
        push_issue(
            &mut issues,
            "unexpected_staged_page",
            None,
            format!(
                "staging contains Markdown page `{}` that is absent from the documentation plan",
                plan.docs_root.join(path).display()
            ),
            false,
        );
    }

    for page in &plan.pages {
        if let Err(error) = validate_docs_page_path(&page.path, &plan.docs_root) {
            all_pages_exist = false;
            add_page_repair(
                &mut repairs,
                page,
                format!("Fix invalid planned page path: {error}"),
            );
            push_issue(
                &mut issues,
                "invalid_page_path",
                Some(&page.id),
                error,
                false,
            );
            continue;
        }

        let relative_page = page
            .path
            .strip_prefix(&plan.docs_root)
            .expect("validated page path is inside docs_root");
        let staged_path = staged_docs_root.join(relative_page);
        if !staged_path.is_file() {
            all_pages_exist = false;
            add_page_repair(
                &mut repairs,
                page,
                format!("Create planned page `{}`", page.path.display()),
            );
            push_issue(
                &mut issues,
                "missing_planned_page",
                Some(&page.id),
                format!("planned page `{}` does not exist", page.path.display()),
                true,
            );
            continue;
        }

        let markdown = match fs::read_to_string(&staged_path) {
            Ok(markdown) => markdown,
            Err(error) => {
                all_pages_exist = false;
                add_page_repair(
                    &mut repairs,
                    page,
                    format!("Make `{}` readable as UTF-8", page.path.display()),
                );
                push_issue(
                    &mut issues,
                    "unreadable_planned_page",
                    Some(&page.id),
                    format!("failed to read `{}`: {error}", page.path.display()),
                    true,
                );
                continue;
            }
        };

        let parsed = match parse_markdown(&markdown) {
            Ok(parsed) => parsed,
            Err(error) => {
                all_frontmatter_valid = false;
                all_content_valid = false;
                add_page_repair(&mut repairs, page, error.clone());
                push_issue(
                    &mut issues,
                    "invalid_frontmatter",
                    Some(&page.id),
                    error,
                    true,
                );
                continue;
            }
        };

        if parsed.codocia_managed != Some(true) {
            all_frontmatter_valid = false;
            let message = "frontmatter must contain `codocia_managed: true`".to_string();
            add_page_repair(&mut repairs, page, message.clone());
            push_issue(
                &mut issues,
                "invalid_managed_flag",
                Some(&page.id),
                message,
                true,
            );
        }

        let expected_language = plan.page_language(page).ok();
        let language_matches = match (parsed.language.as_deref(), expected_language) {
            (Some(actual), Some(expected)) => actual == expected,
            (None, _) => plan.secondary_language.is_none(),
            _ => false,
        };
        if !language_matches {
            all_frontmatter_valid = false;
            let message = format!(
                "frontmatter language {:?} does not match planned language {:?}",
                parsed.language, expected_language
            );
            add_page_repair(&mut repairs, page, message.clone());
            push_issue(
                &mut issues,
                "language_mismatch",
                Some(&page.id),
                message,
                true,
            );
        }

        if let Some(reason) = unsafe_renderable_markdown(&parsed.body) {
            all_content_valid = false;
            let message = format!("page contains executable or unsafe site content: {reason}");
            add_page_repair(&mut repairs, page, message.clone());
            push_issue(
                &mut issues,
                "unsafe_site_content",
                Some(&page.id),
                message,
                true,
            );
        }
        if let Some(reason) = managed_h1_violation(&parsed.body, &page.title) {
            all_content_valid = false;
            let message = format!("body H1 does not match the managed page contract: {reason}");
            add_page_repair(&mut repairs, page, message.clone());
            push_issue(
                &mut issues,
                "page_h1_mismatch",
                Some(&page.id),
                message,
                true,
            );
        }

        // The normalizer omits `covers` for pages whose planned set is empty.
        // Treat omission and `covers: []` as the same empty declaration.
        let actual_covers = parsed.covers.as_deref().unwrap_or_default();
        let covers_match = equal_unique_strings(actual_covers, &page.covers);
        if !covers_match {
            all_frontmatter_valid = false;
            let message = format!(
                "frontmatter covers {:?} do not match planned covers {:?}",
                actual_covers, page.covers
            );
            add_page_repair(&mut repairs, page, message.clone());
            push_issue(
                &mut issues,
                "covers_mismatch",
                Some(&page.id),
                message,
                true,
            );
        }

        let meaningful_chars = meaningful_body_chars(&parsed.body);
        let minimum_body_chars = minimum_body_text_chars(plan.depth, page.kind);
        if meaningful_chars < minimum_body_chars {
            all_content_valid = false;
            let message = format!(
                "body contains {meaningful_chars} meaningful character(s); minimum for {:?} depth is {minimum_body_chars}",
                plan.depth
            );
            add_page_repair(&mut repairs, page, message.clone());
            push_issue(&mut issues, "body_too_short", Some(&page.id), message, true);
        }

        let placeholder = find_placeholder(&parsed.body);
        if let Some(marker) = placeholder {
            all_content_valid = false;
            let message = format!("body contains placeholder marker `{marker}`");
            add_page_repair(&mut repairs, page, message.clone());
            push_issue(
                &mut issues,
                "placeholder_content",
                Some(&page.id),
                message,
                true,
            );
        }

        let heading_slugs = markdown_heading_slugs(&parsed.body);
        for required in &page.required_sections {
            let required_slug = gfm_slug(required);
            if required_slug.is_empty() || !heading_slugs.contains(&required_slug) {
                all_content_valid = false;
                let message =
                    format!("required section `{required}` is missing as a Markdown heading");
                add_page_repair(&mut repairs, page, message.clone());
                push_issue(
                    &mut issues,
                    "missing_required_section",
                    Some(&page.id),
                    message,
                    true,
                );
            }
        }

        if page.kind.requires_source_coverage() {
            let missing_paths = missing_body_source_paths(page, &parsed.body);
            if !missing_paths.is_empty() {
                all_content_valid = false;
                let message = format!(
                    "source-covering page body is missing {} exact inline-code citation(s) required by `source_files`: {}",
                    missing_paths.len(),
                    missing_paths
                        .iter()
                        .map(|path| format!("`{}`", path.display()))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                add_page_repair(
                    &mut repairs,
                    page,
                    format!(
                        "Cite every missing `source_files` path exactly as Markdown inline code in the page body (not only in frontmatter or a fenced code block): {}",
                        missing_paths
                            .iter()
                            .map(|path| format!("`{}`", path.display()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                push_issue(
                    &mut issues,
                    "page_body_missing_source_path",
                    Some(&page.id),
                    message,
                    true,
                );
            }
        }

        for target in markdown_link_targets(&parsed.body) {
            let Some(local_target) = local_link_target(&target) else {
                continue;
            };
            let Some(resolved) = resolve_local_link(&page.path, &plan.docs_root, &local_target)
            else {
                all_links_valid = false;
                let message = "relative link escapes the staged documentation root".to_string();
                broken_links.push(BrokenLink {
                    page_id: page.id.clone(),
                    target: target.clone(),
                    message: message.clone(),
                });
                add_page_repair(
                    &mut repairs,
                    page,
                    format!("Repair link `{target}`: {message}"),
                );
                continue;
            };
            if !resolved.starts_with(&plan.docs_root) {
                all_links_valid = false;
                let message = "relative link escapes the published documentation root".to_string();
                broken_links.push(BrokenLink {
                    page_id: page.id.clone(),
                    target: target.clone(),
                    message: message.clone(),
                });
                add_page_repair(
                    &mut repairs,
                    page,
                    format!("Repair link `{target}`: {message}"),
                );
                continue;
            }
            let Some((resolved, resolved_file)) = resolve_link_file(
                workspace,
                staged_docs_root,
                &plan.docs_root,
                &resolved,
                local_target.directory_hint,
            ) else {
                all_links_valid = false;
                let message = format!("resolved file `{}` does not exist", resolved.display());
                broken_links.push(BrokenLink {
                    page_id: page.id.clone(),
                    target: target.clone(),
                    message: message.clone(),
                });
                add_page_repair(
                    &mut repairs,
                    page,
                    format!("Repair link `{target}`: {message}"),
                );
                continue;
            };

            if is_markdown_file(&resolved_file)
                && let Some(target_page) = plan
                    .pages
                    .iter()
                    .find(|candidate| candidate.path == resolved)
                && plan.page_language(page).ok() != plan.page_language(target_page).ok()
            {
                all_links_valid = false;
                let message = format!(
                    "link resolves to page `{}` in a different documentation language",
                    target_page.path.display()
                );
                broken_links.push(BrokenLink {
                    page_id: page.id.clone(),
                    target: target.clone(),
                    message: message.clone(),
                });
                add_page_repair(
                    &mut repairs,
                    page,
                    format!("Repair link `{target}`: {message}"),
                );
                continue;
            }

            if let Some(fragment) = local_target.fragment.as_deref()
                && is_markdown_file(&resolved_file)
            {
                let heading_exists = fs::read_to_string(&resolved_file)
                    .map(|markdown| markdown_heading_slugs(&markdown).contains(fragment))
                    .unwrap_or(false);
                if !heading_exists {
                    all_links_valid = false;
                    let message = format!(
                        "heading fragment `#{fragment}` does not exist in `{}`",
                        resolved.display()
                    );
                    broken_links.push(BrokenLink {
                        page_id: page.id.clone(),
                        target: target.clone(),
                        message: message.clone(),
                    });
                    add_page_repair(
                        &mut repairs,
                        page,
                        format!("Repair link `{target}`: {message}"),
                    );
                }
            }
        }
    }

    let included = inventory.included_paths();
    let mut uncovered_files = Vec::new();
    for path in included {
        let covered = plan.pages.iter().any(|page| {
            page.source_files.contains(&path)
                && page
                    .covers
                    .iter()
                    .any(|pattern| glob_matches(pattern, &path))
        });
        if !covered {
            uncovered_files.push(path.clone());
            push_issue(
                &mut issues,
                "uncovered_inventory_file",
                None,
                format!(
                    "included inventory file `{}` is not assigned to a page whose covers pattern matches it",
                    path.display()
                ),
                true,
            );
        }
    }
    uncovered_files.sort();
    broken_links
        .sort_by(|left, right| (&left.page_id, &left.target).cmp(&(&right.page_id, &right.target)));

    let coverage_complete = uncovered_files.is_empty();
    let model_valid = model_errors.is_empty();
    let checks = vec![
        check(
            "generation_model",
            model_valid,
            if model_valid {
                "plan and inventory satisfy their data contracts".to_string()
            } else {
                format!("{} model contract error(s)", model_errors.len())
            },
        ),
        check(
            "source_fingerprint",
            fingerprint_matches,
            if fingerprint_matches {
                "plan, inventory, and current source fingerprints match".to_string()
            } else {
                "source fingerprint changed or is inconsistent".to_string()
            },
        ),
        check(
            "planned_pages",
            all_pages_exist,
            if all_pages_exist {
                format!("all {} planned page(s) exist", plan.pages.len())
            } else {
                "one or more planned pages are missing or unreadable".to_string()
            },
        ),
        check(
            "staged_page_set",
            unexpected_staged_pages.is_empty(),
            if unexpected_staged_pages.is_empty() {
                "staging contains no Markdown pages outside the plan".to_string()
            } else {
                format!(
                    "{} unexpected Markdown page(s) exist in staging",
                    unexpected_staged_pages.len()
                )
            },
        ),
        check(
            "frontmatter",
            all_frontmatter_valid,
            if all_frontmatter_valid {
                "all pages are Codocia-managed and declare the planned covers".to_string()
            } else {
                "one or more pages have invalid managed metadata or covers".to_string()
            },
        ),
        check(
            "content",
            all_content_valid,
            if all_content_valid {
                "all pages contain non-placeholder bodies".to_string()
            } else {
                "one or more pages are empty, too short, or contain placeholders".to_string()
            },
        ),
        check(
            "relative_links",
            all_links_valid,
            if all_links_valid {
                "all relative Markdown links resolve to staged files".to_string()
            } else {
                format!(
                    "{} relative Markdown link(s) are broken",
                    broken_links.len()
                )
            },
        ),
        check(
            "inventory_coverage",
            coverage_complete,
            if coverage_complete {
                "all included inventory files are covered".to_string()
            } else {
                format!("{} included file(s) are uncovered", uncovered_files.len())
            },
        ),
    ];

    let page_results = plan
        .pages
        .iter()
        .map(|page| {
            let required_repairs = repairs.remove(&page.id).unwrap_or_default();
            let status = if required_repairs.is_empty() {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Failed
            };
            let summary = if status == VerificationStatus::Passed {
                "page passed deterministic verification".to_string()
            } else {
                format!("page requires {} repair(s)", required_repairs.len())
            };
            (
                page.id.clone(),
                PageVerification {
                    status,
                    summary,
                    required_repairs,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    let failed = checks
        .iter()
        .any(|check| check.status == VerificationStatus::Failed)
        || !issues.is_empty()
        || !broken_links.is_empty()
        || !uncovered_files.is_empty();
    let status = if failed {
        VerificationStatus::Failed
    } else {
        VerificationStatus::Passed
    };
    let summary = if status == VerificationStatus::Passed {
        format!(
            "verification passed for {} planned page(s)",
            plan.pages.len()
        )
    } else {
        format!(
            "verification failed with {} issue(s), {} broken link(s), and {} uncovered file(s)",
            issues.len(),
            broken_links.len(),
            uncovered_files.len()
        )
    };

    Ok(VerificationReport {
        schema_version: GENERATION_SCHEMA_VERSION,
        // The report describes this plan. A mismatch with the freshly computed
        // fingerprint is represented as a failed check and issue above.
        source_fingerprint: plan.source_fingerprint.clone(),
        status,
        summary,
        checks,
        page_results,
        uncovered_files,
        broken_links,
        issues,
    })
}

fn collect_staged_markdown_paths(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    if root.exists() {
        collect_staged_markdown_paths_inner(root, root, &mut paths)?;
    }
    Ok(paths)
}

fn collect_staged_markdown_paths_inner(
    root: &Path,
    directory: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(anyhow::anyhow!(
                "staging contains a symbolic link: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_staged_markdown_paths_inner(root, &path, paths)?;
        } else if metadata.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("mdx")
                })
        {
            paths.insert(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ParsedMarkdown {
    codocia_managed: Option<bool>,
    language: Option<String>,
    covers: Option<Vec<String>>,
    body: String,
}

fn workspace_matches_inventory(workspace: &Path, inventory: &Inventory) -> (bool, Vec<PathBuf>) {
    let mut changed = Vec::new();
    for file in &inventory.files {
        let Some(expected_hash) = file.content_hash.as_deref() else {
            continue;
        };
        let matches = fs::read(workspace.join(&file.path))
            .map(|content| sha256_fingerprint(&content) == expected_hash)
            .unwrap_or(false);
        if !matches {
            changed.push(file.path.clone());
        }
    }
    changed.sort();
    (changed.is_empty(), changed)
}

fn parse_markdown(markdown: &str) -> Result<ParsedMarkdown, String> {
    let normalized = markdown.replace("\r\n", "\n");
    let mut lines = normalized.lines();
    if lines.next() != Some("---") {
        return Err("Markdown must begin with YAML frontmatter".to_string());
    }

    let mut frontmatter = Vec::new();
    let mut closed = false;
    for line in &mut lines {
        if line == "---" {
            closed = true;
            break;
        }
        frontmatter.push(line);
    }
    if !closed {
        return Err("Markdown frontmatter is not terminated by `---`".to_string());
    }

    let mut codocia_managed = None;
    let mut language = None;
    let mut covers = None;
    let mut in_covers = false;
    for line in frontmatter {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("codocia_managed:") {
            codocia_managed = match value.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => return Err("`codocia_managed` must be `true` or `false`".to_string()),
            };
            in_covers = false;
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("language:") {
            if language.is_some() {
                return Err("frontmatter declares `language` more than once".to_string());
            }
            let value = unquote(value.trim()).trim();
            if value.is_empty() {
                return Err("frontmatter `language` must not be empty".to_string());
            }
            language = Some(value.to_string());
            in_covers = false;
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("covers:") {
            if covers.is_some() {
                return Err("frontmatter declares `covers` more than once".to_string());
            }
            covers = Some(parse_inline_covers(value.trim())?);
            in_covers = value.trim().is_empty();
            continue;
        }
        if in_covers {
            if let Some(value) = trimmed.strip_prefix("- ") {
                covers
                    .as_mut()
                    .expect("covers is initialized before list entries")
                    .push(unquote(value.trim()).to_string());
                continue;
            }
            if is_top_level_key(line) {
                in_covers = false;
            } else if !trimmed.is_empty() {
                return Err(format!("invalid covers list entry `{trimmed}`"));
            }
        }
    }

    Ok(ParsedMarkdown {
        codocia_managed,
        language,
        covers,
        body: lines.collect::<Vec<_>>().join("\n"),
    })
}

fn parse_inline_covers(value: &str) -> Result<Vec<String>, String> {
    if value.is_empty() || value == "[]" {
        return Ok(Vec::new());
    }
    let Some(inner) = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return Err("inline `covers` must be a YAML list such as `[]` or `[\"src/**\"]`".into());
    };
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(inner
        .split(',')
        .map(|item| unquote(item.trim()).to_string())
        .collect())
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn is_top_level_key(line: &str) -> bool {
    !line.starts_with(char::is_whitespace) && line.contains(':')
}

fn equal_unique_strings(actual: &[String], expected: &[String]) -> bool {
    let actual_set = actual.iter().collect::<BTreeSet<_>>();
    let expected_set = expected.iter().collect::<BTreeSet<_>>();
    actual.len() == actual_set.len()
        && expected.len() == expected_set.len()
        && actual_set == expected_set
}

fn meaningful_body_chars(body: &str) -> usize {
    body.chars()
        .filter(|character| {
            character.is_alphanumeric() || (!character.is_ascii() && !character.is_whitespace())
        })
        .count()
}

pub(crate) fn missing_body_source_paths(page: &PageSpec, body: &str) -> Vec<PathBuf> {
    let inline_paths = markdown_inline_code_values(body);
    page.source_files
        .iter()
        .filter(|path| !inline_paths.contains(path.to_string_lossy().as_ref()))
        .cloned()
        .collect()
}

fn markdown_inline_code_values(body: &str) -> BTreeSet<String> {
    let searchable = without_code_blocks(body);
    let bytes = searchable.as_bytes();
    let mut values = BTreeSet::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'`' {
            index += 1;
            continue;
        }
        let delimiter_start = index;
        while index < bytes.len() && bytes[index] == b'`' {
            index += 1;
        }
        let delimiter_len = index - delimiter_start;
        let content_start = index;
        let mut cursor = index;
        let mut closed = None;
        while cursor < bytes.len() {
            if bytes[cursor] != b'`' {
                cursor += 1;
                continue;
            }
            let close_start = cursor;
            while cursor < bytes.len() && bytes[cursor] == b'`' {
                cursor += 1;
            }
            if cursor - close_start == delimiter_len {
                closed = Some((close_start, cursor));
                break;
            }
        }
        if let Some((content_end, next)) = closed {
            let value = searchable[content_start..content_end].trim();
            if !value.is_empty() && !value.contains('\r') && !value.contains('\n') {
                values.insert(value.to_string());
            }
            index = next;
        } else {
            index = content_start;
        }
    }
    values
}

fn find_placeholder(body: &str) -> Option<&'static str> {
    let lowercase = body.to_lowercase();
    PLACEHOLDER_MARKERS
        .iter()
        .copied()
        .find(|marker| lowercase.contains(marker))
}

fn markdown_link_targets(body: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(body, options) {
        match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                targets.push(dest_url.into_string());
            }
            Event::Html(fragment) | Event::InlineHtml(fragment) => {
                targets.extend(html_link_targets(&fragment));
            }
            _ => {}
        }
    }
    targets
}

pub(crate) fn unsafe_renderable_markdown(body: &str) -> Option<String> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(body, options) {
        match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. })
                if unsafe_renderable_url(&dest_url) =>
            {
                return Some(format!("unsafe URL `{dest_url}`"));
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                return Some("raw HTML is not allowed".to_string());
            }
            _ => {}
        }
    }
    None
}

fn unsafe_renderable_url(value: &str) -> bool {
    let normalized = value
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && !character.is_ascii_control())
        .collect::<String>()
        .to_ascii_lowercase();
    normalized.starts_with("javascript:")
        || normalized.starts_with("vbscript:")
        || normalized.starts_with("data:")
}

pub(crate) fn managed_h1_violation(body: &str, expected_title: &str) -> Option<String> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let h1_count = Parser::new_ext(body, options)
        .filter(|event| {
            matches!(
                event,
                Event::Start(Tag::Heading {
                    level: HeadingLevel::H1,
                    ..
                })
            )
        })
        .count();
    if h1_count != 1 {
        return Some(format!(
            "expected exactly one semantic H1, found {h1_count}"
        ));
    }

    let Some(first_nonblank) = body.lines().find(|line| !line.trim().is_empty()) else {
        return Some("body is empty".to_string());
    };
    let indent = first_nonblank
        .bytes()
        .take_while(|byte| *byte == b' ')
        .count();
    let trimmed = first_nonblank.trim_start();
    let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    let actual = (indent <= 3 && hashes == 1)
        .then(|| atx_heading_text(first_nonblank))
        .flatten();
    if actual != Some(expected_title) {
        return Some(format!(
            "the first nonblank line must be the exact ATX heading `# {expected_title}`"
        ));
    }
    None
}

fn without_code_blocks(body: &str) -> String {
    let mut output = String::new();
    let mut fence: Option<&str> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            output.push('\n');
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some("```");
            output.push('\n');
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some("~~~");
            output.push('\n');
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    output
}

fn html_link_targets(fragment: &str) -> Vec<String> {
    let bytes = fragment.as_bytes();
    let mut targets = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let name_len = if ascii_name_at(bytes, index, b"href") {
            4
        } else if ascii_name_at(bytes, index, b"src") {
            3
        } else {
            index += 1;
            continue;
        };
        if index > 0 && is_html_name_byte(bytes[index - 1]) {
            index += name_len;
            continue;
        }
        let mut cursor = index + name_len;
        if cursor < bytes.len() && is_html_name_byte(bytes[cursor]) {
            index += name_len;
            continue;
        }
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            index += name_len;
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let Some(&first) = bytes.get(cursor) else {
            break;
        };
        let (start, end) = if matches!(first, b'\'' | b'"') {
            let quote = first;
            let start = cursor + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != quote {
                end += 1;
            }
            (start, end)
        } else {
            let start = cursor;
            let mut end = start;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && !matches!(bytes[end], b'>' | b'\'' | b'"')
            {
                end += 1;
            }
            (start, end)
        };
        if start < end
            && let Some(value) = fragment.get(start..end)
        {
            targets.push(value.to_string());
        }
        index = end.saturating_add(1);
    }
    targets
}

fn ascii_name_at(haystack: &[u8], index: usize, needle: &[u8]) -> bool {
    haystack
        .get(index..index.saturating_add(needle.len()))
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
}

fn is_html_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.')
}

#[derive(Debug)]
struct LocalLinkTarget {
    path: Option<PathBuf>,
    fragment: Option<String>,
    site_root_relative: bool,
    directory_hint: bool,
}

fn local_link_target(target: &str) -> Option<LocalLinkTarget> {
    let target = target.trim();
    if target.is_empty() || target.starts_with("//") || has_uri_scheme(target) {
        return None;
    }

    let (path_and_query, raw_fragment) = target
        .split_once('#')
        .map_or((target, None), |(path, fragment)| (path, Some(fragment)));
    let raw_path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path);
    let site_root_relative = raw_path.starts_with('/');
    let directory_hint = raw_path.ends_with('/');
    let raw_path = raw_path.strip_prefix('/').unwrap_or(raw_path);
    let path = if raw_path.is_empty() {
        None
    } else {
        Some(PathBuf::from(percent_decode(raw_path)?))
    };
    let fragment = match raw_fragment.filter(|fragment| !fragment.is_empty()) {
        Some(fragment) => Some(percent_decode(fragment)?),
        None => None,
    };

    if path.is_none() && fragment.is_none() && !site_root_relative {
        return None;
    }

    Some(LocalLinkTarget {
        path,
        fragment,
        site_root_relative,
        directory_hint,
    })
}

fn has_uri_scheme(target: &str) -> bool {
    let prefix = target.split(['/', '#', '?']).next().unwrap_or_default();
    prefix.find(':').is_some_and(|colon| {
        colon > 0
            && prefix[..colon]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn resolve_relative_link(page_path: &Path, target: &Path) -> Option<PathBuf> {
    let mut resolved = page_path.parent()?.to_path_buf();
    for component in target.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            Component::Normal(segment) => resolved.push(segment),
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    Some(resolved)
}

fn resolve_local_link(
    page_path: &Path,
    docs_root: &Path,
    target: &LocalLinkTarget,
) -> Option<PathBuf> {
    if target.site_root_relative {
        let mut resolved = docs_root.to_path_buf();
        let path = target.path.as_deref().unwrap_or_else(|| Path::new(""));
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    if resolved == docs_root {
                        return None;
                    }
                    resolved.pop();
                }
                Component::Normal(segment) => resolved.push(segment),
                Component::Prefix(_) | Component::RootDir => return None,
            }
        }
        return Some(resolved);
    }

    target.path.as_deref().map_or_else(
        || Some(page_path.to_path_buf()),
        |path| resolve_relative_link(page_path, path),
    )
}

fn resolve_link_file(
    workspace: &Path,
    staged_docs_root: &Path,
    docs_root: &Path,
    resolved: &Path,
    directory_hint: bool,
) -> Option<(PathBuf, PathBuf)> {
    let Ok(relative) = resolved.strip_prefix(docs_root) else {
        let physical = workspace.join(resolved);
        return physical
            .is_file()
            .then(|| (resolved.to_path_buf(), physical));
    };

    let base = staged_docs_root.join(relative);
    let mut candidates = Vec::new();
    if relative.as_os_str().is_empty() || directory_hint {
        candidates.push(base.join("index.md"));
        candidates.push(base.join("index.mdx"));
    } else {
        candidates.push(base.clone());
        if resolved.extension().is_none() {
            candidates.push(base.with_extension("md"));
            candidates.push(base.with_extension("mdx"));
            candidates.push(base.join("index.md"));
            candidates.push(base.join("index.mdx"));
        }
    }

    candidates.into_iter().find_map(|physical| {
        if !physical.is_file() {
            return None;
        }
        let actual_relative = physical.strip_prefix(staged_docs_root).ok()?;
        Some((docs_root.join(actual_relative), physical))
    })
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "md" | "mdx"))
}

pub(crate) fn markdown_heading_slugs(markdown: &str) -> BTreeSet<String> {
    let body = parse_markdown(markdown)
        .map(|parsed| parsed.body)
        .unwrap_or_else(|_| markdown.to_string());
    let searchable = without_code_blocks(&body);
    let lines = searchable.lines().collect::<Vec<_>>();
    let mut headings = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if let Some(heading) = atx_heading_text(lines[index]) {
            headings.push(heading.to_string());
            index += 1;
            continue;
        }
        if index + 1 < lines.len()
            && !lines[index].trim().is_empty()
            && is_setext_underline(lines[index + 1])
        {
            headings.push(lines[index].trim().to_string());
            index += 2;
            continue;
        }
        index += 1;
    }

    let mut slugs = BTreeSet::new();
    let mut next_suffix = BTreeMap::<String, usize>::new();
    for heading in headings {
        let base = gfm_slug(&heading);
        if base.is_empty() {
            continue;
        }
        let mut slug = base.clone();
        while slugs.contains(&slug) {
            let suffix = next_suffix.entry(base.clone()).or_insert(0);
            *suffix += 1;
            slug = format!("{base}-{suffix}");
        }
        next_suffix.entry(base).or_insert(0);
        slugs.insert(slug);
    }
    slugs
}

fn atx_heading_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }

    let content = rest.trim();
    let without_closing = content.trim_end_matches('#');
    if without_closing.len() != content.len()
        && without_closing
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
    {
        Some(without_closing.trim_end())
    } else {
        Some(content)
    }
}

fn is_setext_underline(line: &str) -> bool {
    let marker = line.trim();
    !marker.is_empty()
        && (marker.bytes().all(|byte| byte == b'=') || marker.bytes().all(|byte| byte == b'-'))
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
    !path.is_empty()
        && segment_matches(pattern[0], path[0])
        && glob_match_segments(&pattern[1..], &path[1..])
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
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
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

fn check(name: &str, passed: bool, message: String) -> VerificationCheck {
    VerificationCheck {
        name: name.to_string(),
        status: if passed {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        },
        message,
    }
}

fn push_issue(
    issues: &mut Vec<VerificationIssue>,
    code: &str,
    page_id: Option<&str>,
    message: String,
    repairable: bool,
) {
    issues.push(VerificationIssue {
        code: code.to_string(),
        severity: VerificationSeverity::Error,
        page_id: page_id.map(str::to_string),
        message,
        repairable,
    });
}

fn add_page_repair(repairs: &mut BTreeMap<String, Vec<String>>, page: &PageSpec, repair: String) {
    repairs.entry(page.id.clone()).or_default().push(repair);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation_model::{
        DocumentationDepth, FileDisposition, FileEntry, FileKind, PageKind,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "codocia-generation-verify-{label}-{}-{}",
                std::process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn page(
        id: &str,
        path: &str,
        kind: PageKind,
        covers: &[&str],
        source_files: &[&str],
    ) -> PageSpec {
        let title = id
            .split('-')
            .enumerate()
            .map(|(index, part)| {
                if index == 0 {
                    let mut characters = part.chars();
                    characters
                        .next()
                        .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
                        .unwrap_or_default()
                } else {
                    part.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        PageSpec {
            id: id.to_string(),
            path: PathBuf::from(path),
            language: "en".to_string(),
            title,
            kind,
            purpose: format!("Explain {id}"),
            covers: covers.iter().map(|value| value.to_string()).collect(),
            source_files: source_files.iter().map(PathBuf::from).collect(),
            topics: vec![id.to_string()],
            algorithms: Vec::new(),
            algorithm_sections: Vec::new(),
            required_sections: vec![id.to_string()],
            depends_on: Vec::new(),
            managed: true,
        }
    }

    fn plan(mut pages: Vec<PageSpec>, fingerprint: &str) -> DocPlan {
        if !pages.iter().any(|page| page.kind == PageKind::FileAtlas) {
            let source_files = pages
                .iter()
                .flat_map(|page| page.source_files.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            pages.push(page(
                "file-atlas",
                "docs/file-atlas.md",
                PageKind::FileAtlas,
                &["**"],
                &source_files
                    .iter()
                    .map(|path| path.to_str().unwrap())
                    .collect::<Vec<_>>(),
            ));
        }
        DocPlan {
            schema_version: GENERATION_SCHEMA_VERSION,
            project_name: "example".to_string(),
            source_fingerprint: sha256_fingerprint(fingerprint.as_bytes()),
            docs_root: PathBuf::from("docs"),
            language: "en".to_string(),
            secondary_language: None,
            audience: "developers".to_string(),
            depth: DocumentationDepth::Compact,
            pages,
        }
    }

    fn inventory(root: &TestDir, paths: &[&str], fingerprint: &str) -> Inventory {
        let atlas_entries = paths
            .iter()
            .map(|path| format!("- `{path}`: source-grounded implementation evidence."))
            .collect::<Vec<_>>()
            .join("\n");
        root.write(
            "docs/file-atlas.md",
            &format!(
                "---\ncodocia_managed: true\ncovers:\n  - \"**\"\n---\n\n# File atlas\n\nThis page maps repository files to the generated conceptual documentation and its verified implementation boundaries.\n\n{atlas_entries}\n"
            ),
        );
        Inventory {
            schema_version: GENERATION_SCHEMA_VERSION,
            project_name: "example".to_string(),
            source_revision: None,
            source_fingerprint: sha256_fingerprint(fingerprint.as_bytes()),
            languages: BTreeSet::from(["Rust".to_string()]),
            files: paths
                .iter()
                .map(|path| {
                    let content = format!("source fixture for {path}\n");
                    root.write(path, &content);
                    FileEntry {
                        path: PathBuf::from(path),
                        kind: FileKind::Source,
                        language: Some("Rust".to_string()),
                        size_bytes: content.len() as u64,
                        content_hash: Some(sha256_fingerprint(content.as_bytes())),
                        disposition: FileDisposition::Include,
                    }
                })
                .collect(),
        }
    }

    fn valid_plan(fingerprint: &str) -> DocPlan {
        plan(
            vec![
                page("index", "docs/index.md", PageKind::Index, &[], &[]),
                page(
                    "architecture",
                    "docs/architecture.md",
                    PageKind::Architecture,
                    &["src/**"],
                    &["src/lib.rs", "src/main.rs"],
                ),
            ],
            fingerprint,
        )
    }

    #[test]
    fn complete_staging_tree_passes_all_checks() {
        let root = TestDir::new("pass");
        root.write(
            "docs/index.md",
            "---\ncodocia_managed: true\ncovers: []\n---\n\n# Index\n\nRead the [architecture](architecture.md) to understand the complete runtime design and its boundaries.\n",
        );
        root.write(
            "docs/architecture.md",
            "---\ncodocia_managed: true\ncovers:\n  - src/**\n---\n\n# Architecture\n\nThe runtime owns request processing, state transitions, and explicit failure handling across both `src/lib.rs` and `src/main.rs`.\n",
        );
        let plan = valid_plan("source-1");
        let inventory = inventory(&root, &["src/lib.rs", "src/main.rs"], "source-1");

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();

        assert_eq!(report.status, VerificationStatus::Passed);
        assert!(report.issues.is_empty());
        assert!(report.broken_links.is_empty());
        assert!(report.uncovered_files.is_empty());
        assert_eq!(report.validate_against(&plan), Ok(()));
    }

    #[test]
    fn rejects_markdown_pages_outside_the_plan() {
        let root = TestDir::new("unexpected-staged-page");
        root.write(
            "docs/index.md",
            "---\ncodocia_managed: true\ncovers: []\n---\n\n# Index\n\nRead the architecture page for the complete runtime design and implementation boundaries.\n",
        );
        root.write(
            "docs/architecture.md",
            "---\ncodocia_managed: true\ncovers:\n  - src/**\n---\n\n# Architecture\n\nThe runtime owns request processing, state transitions, and explicit failure handling across both `src/lib.rs` and `src/main.rs`.\n",
        );
        root.write(
            "docs/stale.md",
            "---\ncodocia_managed: true\ncovers: []\n---\n\n# Stale\n\nThis page is not part of the current documentation plan.\n",
        );
        let plan = valid_plan("source-1");
        let inventory = inventory(&root, &["src/lib.rs", "src/main.rs"], "source-1");

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();

        assert_eq!(report.status, VerificationStatus::Failed);
        assert!(report.issues.iter().any(|issue| {
            issue.code == "unexpected_staged_page"
                && issue.message.contains("docs/stale.md")
                && !issue.repairable
        }));
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.name == "staged_page_set")
                .unwrap()
                .status,
            VerificationStatus::Failed
        );
    }

    #[test]
    fn file_atlas_body_must_name_every_planned_source_path() {
        let root = TestDir::new("atlas-body-coverage");
        root.write(
            "docs/index.md",
            "---\ncodocia_managed: true\ncovers: []\n---\n\n# Index\n\nRead the architecture page for the complete runtime design and implementation boundaries.\n",
        );
        root.write(
            "docs/architecture.md",
            "---\ncodocia_managed: true\ncovers:\n  - src/**\n---\n\n# Architecture\n\nThe runtime owns request processing, state transitions, and explicit failure handling across both `src/lib.rs` and `src/main.rs`.\n",
        );
        let plan = valid_plan("source-1");
        let inventory = inventory(&root, &["src/lib.rs", "src/main.rs"], "source-1");
        root.write(
            "docs/file-atlas.md",
            "---\ncodocia_managed: true\ncovers:\n  - \"**\"\n---\n\n# File atlas\n\nThe atlas discusses `src/lib.rs` in detail and mentions only the longer unrelated path `src/main.rs.bak`, so deterministic verification must still reject the missing exact path.\n",
        );

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();

        assert_eq!(report.status, VerificationStatus::Failed);
        assert!(report.issues.iter().any(|issue| {
            issue.code == "page_body_missing_source_path"
                && issue.page_id.as_deref() == Some("file-atlas")
                && issue.message.contains("src/main.rs")
        }));
    }

    #[test]
    fn every_source_covering_page_body_must_cite_each_source_file() {
        let root = TestDir::new("substantive-body-coverage");
        root.write(
            "docs/index.md",
            "---\ncodocia_managed: true\ncovers: []\n---\n\n# Index\n\nRead the architecture page for the complete runtime design and implementation boundaries.\n",
        );
        root.write(
            "docs/architecture.md",
            "---\ncodocia_managed: true\ncovers:\n  - src/**\n---\n\n# Architecture\n\nThe runtime explains `src/lib.rs`, while frontmatter metadata and the longer unrelated inline value `src/main.rs.bak` cannot bind the missing evidence path.\n",
        );
        let plan = valid_plan("source-1");
        let inventory = inventory(&root, &["src/lib.rs", "src/main.rs"], "source-1");

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();

        assert_eq!(report.status, VerificationStatus::Failed);
        let issue = report
            .issues
            .iter()
            .find(|issue| {
                issue.code == "page_body_missing_source_path"
                    && issue.page_id.as_deref() == Some("architecture")
            })
            .expect("architecture should fail body evidence binding");
        assert!(issue.repairable);
        assert!(issue.message.contains("`source_files`"));
        assert!(issue.message.contains("`src/main.rs`"));
        assert!(
            report.page_results["architecture"]
                .required_repairs
                .iter()
                .any(|repair| {
                    repair.contains("Markdown inline code")
                        && repair.contains("not only in frontmatter or a fenced code block")
                        && repair.contains("`src/main.rs`")
                })
        );
    }

    #[test]
    fn reports_missing_page_bad_frontmatter_placeholder_and_broken_link() {
        let root = TestDir::new("page-errors");
        root.write(
            "docs/index.md",
            "---\ncodocia_managed: false\ncovers:\n  - wrong/**\n---\n\n# TODO\n\n[x](missing.md)\n",
        );
        let plan = valid_plan("source-1");
        let inventory = inventory(&root, &["src/lib.rs", "src/main.rs"], "source-1");

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();
        let codes = report
            .issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(report.status, VerificationStatus::Failed);
        assert!(codes.contains("missing_planned_page"));
        assert!(codes.contains("invalid_managed_flag"));
        assert!(codes.contains("covers_mismatch"));
        assert!(codes.contains("placeholder_content"));
        assert!(codes.contains("body_too_short"));
        assert_eq!(report.broken_links.len(), 1);
        assert_eq!(
            report.page_results["architecture"].status,
            VerificationStatus::Failed
        );
    }

    #[test]
    fn reports_source_drift_and_inventory_files_without_matching_coverage() {
        let root = TestDir::new("coverage");
        root.write(
            "docs/index.md",
            "---\ncodocia_managed: true\ncovers: []\n---\n\n# Index\n\nThis overview explains the repository, its runtime responsibilities, and the supported contributor workflow.\n",
        );
        root.write(
            "docs/architecture.md",
            "---\ncodocia_managed: true\ncovers:\n  - src/lib.rs\n---\n\n# Architecture\n\nThis page explains the `src/lib.rs` entry point, its stable contract, state transitions, and validation boundaries.\n",
        );
        let plan = plan(
            vec![
                page("index", "docs/index.md", PageKind::Index, &[], &[]),
                page(
                    "architecture",
                    "docs/architecture.md",
                    PageKind::Architecture,
                    &["src/lib.rs"],
                    &["src/lib.rs"],
                ),
            ],
            "source-1",
        );
        let inventory = inventory(&root, &["src/lib.rs", "src/main.rs"], "source-1");
        root.write("src/lib.rs", "changed source content\n");

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();

        assert_eq!(report.status, VerificationStatus::Failed);
        assert_eq!(report.uncovered_files, vec![PathBuf::from("src/main.rs")]);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "source_fingerprint_mismatch")
        );
    }

    #[test]
    fn resolves_nested_parent_links_and_ignores_links_inside_fences() {
        let root = TestDir::new("nested-links");
        root.write(
            "docs/index.md",
            "---\ncodocia_managed: true\ncovers: []\n---\n\n# Index\n\nThis repository overview provides enough detail for maintainers and links all generated subsystem documentation.\n",
        );
        root.write(
            "docs/guides/runtime.md",
            "---\ncodocia_managed: true\ncovers:\n  - src/**\n---\n\n# Runtime\n\n`src/lib.rs` implements the runtime. Return to the [overview](../index.md) for repository boundaries and contributor guidance.\n\n```md\n[example only](missing.md)\n```\n",
        );
        let plan = plan(
            vec![
                page("index", "docs/index.md", PageKind::Index, &[], &[]),
                page(
                    "runtime",
                    "docs/guides/runtime.md",
                    PageKind::Architecture,
                    &["src/**"],
                    &["src/lib.rs"],
                ),
            ],
            "source-1",
        );
        let inventory = inventory(&root, &["src/lib.rs"], "source-1");

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();

        assert_eq!(report.status, VerificationStatus::Passed);
        assert!(report.broken_links.is_empty());
    }

    #[test]
    fn validates_site_root_explicit_and_heading_fragment_links() {
        let root = TestDir::new("root-and-fragments");
        root.write(
            "docs/index.md",
            "---\ncodocia_managed: true\ncovers: []\n---\n\n# Index\n\n## Overview\n\nFollow the [site route](/foo#request-flow-1), [explicit page](foo.md#request-flow), [setext heading](/foo#details), and [local heading](#overview) for verified navigation.\n\n```md\n[example only](/missing#not-real)\n```\n",
        );
        root.write(
            "docs/foo.md",
            "---\ncodocia_managed: true\ncovers:\n  - src/**\n---\n\n# Foo\n\nThis page explains the `src/lib.rs` runtime design, request boundaries, and contributor-facing behavior in enough detail for verification.\n\n## Request Flow\n\nThe first flow describes accepted input and state transitions.\n\n## Request Flow\n\nThe duplicate flow describes retries and terminal output.\n\nDetails\n-------\n\nThese details document the remaining operational boundary.\n",
        );
        let plan = plan(
            vec![
                page("index", "docs/index.md", PageKind::Index, &[], &[]),
                page(
                    "foo",
                    "docs/foo.md",
                    PageKind::Architecture,
                    &["src/**"],
                    &["src/lib.rs"],
                ),
            ],
            "source-1",
        );
        let inventory = inventory(&root, &["src/lib.rs"], "source-1");

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();

        assert_eq!(report.status, VerificationStatus::Passed);
        assert!(report.broken_links.is_empty());
    }

    #[test]
    fn reports_missing_site_root_paths_and_heading_fragments() {
        let root = TestDir::new("bad-root-and-fragments");
        root.write(
            "docs/index.md",
            "---\ncodocia_managed: true\ncovers: []\n---\n\n# Index\n\nThese links exercise [a missing route](/absent), [a local fragment](#not-present), [an explicit fragment](foo.md#not-there), and [a fenced-only heading](/foo#code-only).\n",
        );
        root.write(
            "docs/foo.md",
            "---\ncodocia_managed: true\ncovers:\n  - src/**\n---\n\n# Foo\n\nThis page contains enough source-grounded prose about `src/lib.rs` to pass content validation while its heading index remains deliberately small.\n\n```md\n## Code Only\n```\n",
        );
        let plan = plan(
            vec![
                page("index", "docs/index.md", PageKind::Index, &[], &[]),
                page(
                    "foo",
                    "docs/foo.md",
                    PageKind::Architecture,
                    &["src/**"],
                    &["src/lib.rs"],
                ),
            ],
            "source-1",
        );
        let inventory = inventory(&root, &["src/lib.rs"], "source-1");

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();
        let broken_targets = report
            .broken_links
            .iter()
            .map(|link| link.target.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(report.status, VerificationStatus::Failed);
        assert_eq!(report.broken_links.len(), 4);
        assert!(broken_targets.contains("/absent"));
        assert!(broken_targets.contains("#not-present"));
        assert!(broken_targets.contains("foo.md#not-there"));
        assert!(broken_targets.contains("/foo#code-only"));
    }

    #[test]
    fn parses_language_frontmatter_and_rejects_duplicates() {
        let parsed = parse_markdown(
            "---\nlanguage: \"zh-CN\"\ncodocia_managed: true\ncovers: []\n---\n\n# 文档\n",
        )
        .unwrap();
        assert_eq!(parsed.language.as_deref(), Some("zh-CN"));

        let error = parse_markdown(
            "---\nlanguage: en\nlanguage: zh-CN\ncodocia_managed: true\n---\n\n# Docs\n",
        )
        .unwrap_err();
        assert_eq!(error, "frontmatter declares `language` more than once");
    }

    #[test]
    fn documentation_depth_has_increasing_deterministic_body_floors() {
        for kind in [PageKind::Index, PageKind::Architecture] {
            let compact = minimum_body_text_chars(DocumentationDepth::Compact, kind);
            let standard = minimum_body_text_chars(DocumentationDepth::Standard, kind);
            let deep = minimum_body_text_chars(DocumentationDepth::Deep, kind);
            assert!(compact < standard);
            assert!(standard < deep);
        }
    }

    #[test]
    fn bilingual_verification_rejects_cross_language_markdown_links() {
        let root = TestDir::new("cross-language-link");
        let mut secondary_index =
            page("zh-index", "docs/zh-cn/index.md", PageKind::Index, &[], &[]);
        secondary_index.language = "zh-CN".to_string();
        let mut secondary_architecture = page(
            "zh-architecture",
            "docs/zh-cn/architecture.md",
            PageKind::Architecture,
            &["src/**"],
            &["src/lib.rs"],
        );
        secondary_architecture.language = "zh-CN".to_string();
        let mut secondary_atlas = page(
            "zh-file-atlas",
            "docs/zh-cn/file-atlas.md",
            PageKind::FileAtlas,
            &["**"],
            &["src/lib.rs"],
        );
        secondary_atlas.language = "zh-CN".to_string();

        let mut plan = plan(
            vec![
                page("index", "docs/index.md", PageKind::Index, &[], &[]),
                page(
                    "architecture",
                    "docs/architecture.md",
                    PageKind::Architecture,
                    &["src/**"],
                    &["src/lib.rs"],
                ),
                page(
                    "file-atlas",
                    "docs/file-atlas.md",
                    PageKind::FileAtlas,
                    &["**"],
                    &["src/lib.rs"],
                ),
                secondary_index,
                secondary_architecture,
                secondary_atlas,
            ],
            "source-1",
        );
        plan.secondary_language = Some("zh-CN".to_string());
        let inventory = inventory(&root, &["src/lib.rs"], "source-1");
        let detail = "This page explains concrete runtime state, invariants, failure behavior, and verification evidence for maintainers. ".repeat(3);
        root.write(
            "docs/index.md",
            &format!("---\nlanguage: en\ncodocia_managed: true\ncovers: []\n---\n\n# Index\n\n[中文](zh-cn/index.md)\n\n{detail}"),
        );
        root.write(
            "docs/architecture.md",
            &format!("---\nlanguage: en\ncodocia_managed: true\ncovers:\n  - src/**\n---\n\n# Architecture\n\n`src/lib.rs` is the exact source evidence. {detail}"),
        );
        root.write(
            "docs/file-atlas.md",
            &format!("---\nlanguage: en\ncodocia_managed: true\ncovers:\n  - \"**\"\n---\n\n# File atlas\n\n`src/lib.rs` is mapped to its conceptual page. {detail}"),
        );
        root.write(
            "docs/zh-cn/index.md",
            &format!("---\nlanguage: zh-CN\ncodocia_managed: true\ncovers: []\n---\n\n# Zh index\n\n{detail}"),
        );
        root.write(
            "docs/zh-cn/architecture.md",
            &format!("---\nlanguage: zh-CN\ncodocia_managed: true\ncovers:\n  - src/**\n---\n\n# Zh architecture\n\n`src/lib.rs` is the exact source evidence. {detail}"),
        );
        root.write(
            "docs/zh-cn/file-atlas.md",
            &format!("---\nlanguage: zh-CN\ncodocia_managed: true\ncovers:\n  - \"**\"\n---\n\n# Zh file atlas\n\n`src/lib.rs` is mapped to its conceptual page. {detail}"),
        );

        let report = verify_staged_docs(&root.0, root.0.join("docs"), &inventory, &plan).unwrap();

        assert!(report.broken_links.iter().any(|link| {
            link.page_id == "index"
                && link.target == "zh-cn/index.md"
                && link.message.contains("different documentation language")
        }));
    }

    #[test]
    fn extracts_commonmark_reference_image_autolink_and_html_targets() {
        let body = r#"
[reference][runtime]
![diagram](assets/flow.svg "Flow")
<https://example.com/docs>
<a class="nav" href='../index.md#overview'>home</a>
<img SRC=assets/runtime.png>

[runtime]: guides/runtime.md#state

`[inline code](ignored-inline.md)`

```md
[fenced](ignored-fence.md)
<a href="ignored-html.md">ignored</a>
```
"#;
        let targets = markdown_link_targets(body)
            .into_iter()
            .collect::<BTreeSet<_>>();

        assert!(targets.contains("guides/runtime.md#state"));
        assert!(targets.contains("assets/flow.svg"));
        assert!(targets.contains("https://example.com/docs"));
        assert!(targets.contains("../index.md#overview"));
        assert!(targets.contains("assets/runtime.png"));
        assert!(!targets.contains("ignored-inline.md"));
        assert!(!targets.contains("ignored-fence.md"));
        assert!(!targets.contains("ignored-html.md"));
    }

    #[test]
    fn unsafe_renderable_markdown_rejects_html_and_active_url_schemes() {
        assert_eq!(
            unsafe_renderable_markdown("<script>alert(1)</script>\n").as_deref(),
            Some("raw HTML is not allowed")
        );
        assert!(
            unsafe_renderable_markdown("[run](javascript:payload)\n")
                .is_some_and(|reason| reason.contains("unsafe URL"))
        );
        assert!(
            unsafe_renderable_markdown("![payload](data:image/svg+xml;base64,AAAA)\n")
                .is_some_and(|reason| reason.contains("unsafe URL"))
        );
        assert!(
            unsafe_renderable_markdown("[run](jav&#x09;ascript:payload)\n")
                .is_some_and(|reason| reason.contains("unsafe URL"))
        );
        assert!(
            unsafe_renderable_markdown("[payload](data:application/xhtml+xml,hello)\n")
                .is_some_and(|reason| reason.contains("unsafe URL"))
        );
        assert_eq!(
            unsafe_renderable_markdown("[safe](https://example.com)\n"),
            None
        );
    }

    #[test]
    fn managed_h1_contract_requires_one_exact_leading_heading() {
        assert_eq!(
            managed_h1_violation("# Runtime\n\nBody.\n", "Runtime"),
            None
        );
        assert!(managed_h1_violation("# Wrong\n\nBody.\n", "Runtime").is_some());
        assert!(managed_h1_violation("Intro.\n\n# Runtime\n", "Runtime").is_some());
        assert!(
            managed_h1_violation("# Runtime\n\n# Duplicate\n", "Runtime")
                .is_some_and(|reason| reason.contains("exactly one semantic H1"))
        );
        assert!(managed_h1_violation("Runtime\n=======\n", "Runtime").is_some());
    }
}
