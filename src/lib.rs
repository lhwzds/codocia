//! Library API for Codocia.

use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub workspace: PathBuf,
    pub out: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDoc {
    pub name: String,
    pub path: PathBuf,
    pub summary: String,
    pub owns: Vec<String>,
    pub must_not: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub depends_on: Vec<String>,
    pub used_by: Vec<String>,
    pub verify: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedDoc {
    file_name: String,
    content: String,
}

pub fn init(path: impl AsRef<Path>) -> Result<()> {
    let config_path = path.as_ref().join("codocia.toml");
    if config_path.exists() {
        return Ok(());
    }
    fs::write(
        config_path,
        "# Codocia reads `# codocia` Markdown blocks from Rust doc comments.\n\
         \n\
         [docs]\n\
         out = \"docs\"\n\
         \n\
         [workspace]\n\
         path = \".\"\n",
    )?;
    Ok(())
}

pub fn generate(config: &Config) -> Result<()> {
    let docs = render(config)?;
    fs::create_dir_all(&config.out)?;
    for doc in docs {
        fs::write(config.out.join(doc.file_name), doc.content)?;
    }
    Ok(())
}

pub fn check(config: &Config) -> Result<()> {
    let docs = render(config)?;
    let mut missing = Vec::new();
    let mut stale = Vec::new();

    for doc in docs {
        let path = config.out.join(doc.file_name);
        let Ok(existing) = fs::read_to_string(&path) else {
            missing.push(path);
            continue;
        };
        if existing != doc.content {
            stale.push(path);
        }
    }

    if missing.is_empty() && stale.is_empty() {
        return Ok(());
    }

    let mut message = String::from("generated docs are out of date");
    if !missing.is_empty() {
        message.push_str("\nmissing:");
        for path in missing {
            message.push_str(&format!("\n- {}", path.display()));
        }
    }
    if !stale.is_empty() {
        message.push_str("\nstale:");
        for path in stale {
            message.push_str(&format!("\n- {}", path.display()));
        }
    }
    bail!(message);
}

pub fn discover_modules(workspace: impl AsRef<Path>) -> Result<Vec<ModuleDoc>> {
    let workspace = workspace.as_ref();
    let mut cargo_files = Vec::new();
    collect_cargo_files(workspace, &mut cargo_files)?;

    let mut modules = Vec::new();
    for cargo_file in cargo_files {
        let crate_dir = cargo_file
            .parent()
            .context("Cargo.toml should have a parent directory")?;
        let Some(source) = source_file(crate_dir) else {
            continue;
        };
        let package_name = package_name(&cargo_file)?;
        let content = fs::read_to_string(&source)
            .with_context(|| format!("failed to read {}", source.display()))?;
        let Some(block) = extract_codocia_block(&content) else {
            continue;
        };
        let mut module = parse_block(&package_name, crate_dir, &block)?;
        module.path = relative_path(workspace, crate_dir);
        modules.push(module);
    }

    modules.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(modules)
}

fn render(config: &Config) -> Result<Vec<RenderedDoc>> {
    let modules = discover_modules(&config.workspace)?;
    if modules.is_empty() {
        bail!(
            "no Codocia Markdown blocks found under {}",
            config.workspace.display()
        );
    }
    let mut docs = vec![RenderedDoc {
        file_name: "CODOCIA.md".to_string(),
        content: render_index(&modules),
    }];
    docs.extend(modules.iter().map(|module| RenderedDoc {
        file_name: format!("{}.md", module.name),
        content: render_module(module),
    }));
    Ok(docs)
}

fn collect_cargo_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if should_skip_dir(dir) {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_cargo_files(&path, out)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some("Cargo.toml") {
            out.push(path);
        }
    }
    Ok(())
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".git" | "target" | "node_modules" | "__pycache__"))
}

fn source_file(crate_dir: &Path) -> Option<PathBuf> {
    let lib = crate_dir.join("src/lib.rs");
    if lib.exists() {
        return Some(lib);
    }
    let main = crate_dir.join("src/main.rs");
    if main.exists() { Some(main) } else { None }
}

fn package_name(cargo_file: &Path) -> Result<String> {
    let content = fs::read_to_string(cargo_file)
        .with_context(|| format!("failed to read {}", cargo_file.display()))?;
    let mut in_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_package = false;
        }
        if in_package
            && trimmed.starts_with("name")
            && let Some((_, value)) = trimmed.split_once('=')
        {
            return Ok(value.trim().trim_matches('"').to_string());
        }
    }
    bail!("missing package name in {}", cargo_file.display());
}

fn extract_codocia_block(content: &str) -> Option<Vec<String>> {
    let mut docs = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(doc) = trimmed.strip_prefix("//!") {
            docs.push(doc.trim_start().to_string());
        } else if docs.is_empty() && trimmed.is_empty() {
            continue;
        } else if docs.is_empty() {
            continue;
        } else {
            break;
        }
    }

    let start = docs
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case("# codocia"))?;
    Some(docs[start + 1..].to_vec())
}

fn parse_block(name: &str, crate_dir: &Path, lines: &[String]) -> Result<ModuleDoc> {
    let mut summary_lines = Vec::new();
    let mut sections: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current_section: Option<String> = None;

    for line in lines {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            current_section = Some(normalize_heading(heading));
            continue;
        }

        if let Some(section) = &current_section {
            if let Some(item) = trimmed.strip_prefix("- ") {
                sections
                    .entry(section.clone())
                    .or_default()
                    .push(item.trim().to_string());
            }
        } else if !trimmed.is_empty() {
            summary_lines.push(trimmed.to_string());
        }
    }

    let summary = summary_lines.join(" ");
    if summary.is_empty() {
        bail!("codocia block for {name} is missing a summary");
    }

    Ok(ModuleDoc {
        name: normalize_module_name(name),
        path: crate_dir.to_path_buf(),
        summary,
        owns: section(&sections, "owns"),
        must_not: section(&sections, "must_not"),
        inputs: section(&sections, "inputs"),
        outputs: section(&sections, "outputs"),
        depends_on: section(&sections, "depends_on"),
        used_by: section(&sections, "used_by"),
        verify: section(&sections, "verify"),
    })
}

fn normalize_heading(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace(' ', "_")
        .replace('-', "_")
}

fn normalize_module_name(value: &str) -> String {
    value.strip_suffix("-v2").unwrap_or(value).to_string()
}

fn section(sections: &BTreeMap<String, Vec<String>>, name: &str) -> Vec<String> {
    sections.get(name).cloned().unwrap_or_default()
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn render_index(modules: &[ModuleDoc]) -> String {
    let mut output = String::new();
    output.push_str("# CODOCIA\n\n");
    output.push_str("Generated by Codocia from `# codocia` Markdown blocks.\n\n");
    output.push_str("## Module Graph\n\n");
    output.push_str("```mermaid\nflowchart TD\n");

    for module in modules {
        output.push_str(&format!(
            "  {}[\"{}<br/>{}\"]\n",
            mermaid_id(&module.name),
            escape_mermaid(&module.name),
            escape_mermaid(&module.summary)
        ));
    }

    let mut edges = BTreeSet::new();
    for module in modules {
        for dependency in &module.depends_on {
            edges.insert((module.name.clone(), dependency.clone()));
        }
        for user in &module.used_by {
            edges.insert((user.clone(), module.name.clone()));
        }
    }

    for (from, to) in edges {
        output.push_str(&format!(
            "  {} --> {}\n",
            mermaid_id(&from),
            mermaid_id(&to)
        ));
    }

    output.push_str("```\n\n");
    output.push_str("## Modules\n\n");
    for module in modules {
        output.push_str(&format!(
            "- [`{}`](./{}.md): {}\n",
            module.name, module.name, module.summary
        ));
    }
    output
}

fn render_module(module: &ModuleDoc) -> String {
    let mut output = String::new();
    output.push_str(&format!("# {}\n\n", module.name));
    output.push_str(&format!("{}\n\n", module.summary));
    output.push_str(&format!("Path: `{}`\n\n", module.path.display()));
    render_list(&mut output, "Owns", &module.owns);
    render_list(&mut output, "Must Not", &module.must_not);
    render_list(&mut output, "Inputs", &module.inputs);
    render_list(&mut output, "Outputs", &module.outputs);
    render_list(&mut output, "Depends On", &module.depends_on);
    render_list(&mut output, "Used By", &module.used_by);
    render_list(&mut output, "Verify", &module.verify);
    output
}

fn render_list(output: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    output.push_str(&format!("{title}:\n"));
    for item in items {
        output.push_str(&format!("- {item}\n"));
    }
    output.push('\n');
}

fn mermaid_id(value: &str) -> String {
    let id: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("m_{id}")
}

fn escape_mermaid(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codocia_markdown_block() {
        let source = r#"
//! # codocia
//!
//! Skill owns turn planning.
//!
//! ## Owns
//! - TurnPlan
//!
//! ## Must Not
//! - render UI
//!
//! ## Depends On
//! - tool
"#;

        let block = extract_codocia_block(source).expect("block should exist");
        let doc = parse_block("skill", Path::new("crates/skill"), &block).unwrap();

        assert_eq!(doc.name, "skill");
        assert_eq!(doc.summary, "Skill owns turn planning.");
        assert_eq!(doc.owns, vec!["TurnPlan"]);
        assert_eq!(doc.must_not, vec!["render UI"]);
        assert_eq!(doc.depends_on, vec!["tool"]);
    }
}
