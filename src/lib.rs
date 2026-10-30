//! Library API for Codocia.

pub mod book;
pub mod structure;
pub mod translate;

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn skill_prompt() -> &'static str {
    include_str!("../SKILL.md")
}

pub(crate) fn current_commit(workspace: &Path) -> Result<String> {
    // A plain directory (no Git) has no commit; never call git for it.
    if !workspace.join(".git").exists() {
        return Ok(String::new());
    }
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

pub(crate) fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub(crate) fn glob_matches(pattern: &str, path: &Path) -> bool {
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

pub(crate) fn write_file(path: PathBuf, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
