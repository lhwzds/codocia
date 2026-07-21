//! Core data model for Codocia's one-command documentation generation flow.
//!
//! Integration note: this module intentionally uses `serde` derives. The main
//! integration change must add `serde = { version = "1", features = ["derive"] }`
//! to `Cargo.toml` and export this module from `lib.rs`.

use crate::integrity::is_sha256_fingerprint;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

/// Version 2 changes every persisted integrity value to canonical,
/// algorithm-qualified SHA-256 (`sha256:<64 lowercase hex digits>`).
pub const GENERATION_SCHEMA_VERSION: u32 = 2;

pub type ValidationResult = Result<(), Vec<String>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerateConfig {
    pub workspace: PathBuf,
    pub docs_root: PathBuf,
    pub goal: Option<String>,
    pub language: String,
    /// Optional second complete documentation language. The primary language
    /// remains rooted directly at `docs_root`; this language is emitted below
    /// `docs_root/<normalized-locale>/`.
    #[serde(default)]
    pub secondary_language: Option<String>,
    pub audience: String,
    pub depth: DocumentationDepth,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub model: Option<String>,
    pub allow_code_upload: bool,
    /// Exact upload-scope fingerprint reviewed by the user before Codex runs.
    #[serde(default)]
    pub upload_scope_fingerprint: Option<String>,
    pub max_repair_rounds: u8,
    pub plan_only: bool,
    pub resume: bool,
    pub prune_unmanaged: bool,
    pub generate_site: bool,
}

impl Default for GenerateConfig {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("."),
            docs_root: PathBuf::from("docs"),
            goal: None,
            language: "en".to_string(),
            secondary_language: None,
            audience: "developers".to_string(),
            depth: DocumentationDepth::Standard,
            include: Vec::new(),
            exclude: Vec::new(),
            model: None,
            allow_code_upload: false,
            upload_scope_fingerprint: None,
            max_repair_rounds: 2,
            plan_only: false,
            resume: false,
            prune_unmanaged: false,
            generate_site: false,
        }
    }
}

impl GenerateConfig {
    pub fn validate(&self) -> ValidationResult {
        let mut errors = Vec::new();

        if self.language.trim().is_empty() {
            errors.push("language must not be empty".to_string());
        }
        let primary_locale = normalized_locale(&self.language);
        if let Err(error) = &primary_locale {
            errors.push(format!("invalid language: {error}"));
        }
        if let Some(secondary_language) = self.secondary_language.as_deref() {
            if secondary_language.trim().is_empty() {
                errors.push("secondary_language must not be empty when present".to_string());
            } else {
                match normalized_locale(secondary_language) {
                    Ok(secondary_locale) => {
                        if let Err(error) = validate_locale_directory(&secondary_locale) {
                            errors.push(format!("invalid secondary_language: {error}"));
                        }
                        if primary_locale
                            .as_ref()
                            .is_ok_and(|primary_locale| primary_locale == &secondary_locale)
                        {
                            errors.push(
                                "secondary_language must differ from the primary language"
                                    .to_string(),
                            );
                        }
                    }
                    Err(error) => errors.push(format!("invalid secondary_language: {error}")),
                }
            }
        }
        if self.audience.trim().is_empty() {
            errors.push("audience must not be empty".to_string());
        }
        if let Err(error) = validate_portable_relative_path(&self.docs_root) {
            errors.push(format!("invalid docs_root: {error}"));
        }
        validate_patterns("include", &self.include, &mut errors);
        validate_patterns("exclude", &self.exclude, &mut errors);
        if let Some(fingerprint) = self.upload_scope_fingerprint.as_deref()
            && !is_sha256_fingerprint(fingerprint)
        {
            errors.push(
                "upload_scope_fingerprint must be a canonical SHA-256 fingerprint".to_string(),
            );
        }
        if self.upload_scope_fingerprint.is_some() && !self.allow_code_upload {
            errors.push(
                "upload_scope_fingerprint requires allow_code_upload acknowledgement".to_string(),
            );
        }

        finish_validation(errors)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentationDepth {
    Compact,
    Standard,
    Deep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub schema_version: u32,
    pub project_name: String,
    pub source_revision: Option<String>,
    pub source_fingerprint: String,
    pub languages: BTreeSet<String>,
    pub files: Vec<FileEntry>,
}

impl Inventory {
    pub fn validate(&self) -> ValidationResult {
        let mut errors = Vec::new();
        let mut paths = BTreeSet::new();

        validate_schema_version(self.schema_version, &mut errors);
        if self.project_name.trim().is_empty() {
            errors.push("inventory project_name must not be empty".to_string());
        }
        validate_fingerprint(
            "inventory source_fingerprint",
            &self.source_fingerprint,
            &mut errors,
        );
        if self.files.is_empty() {
            errors.push("inventory must contain at least one file".to_string());
        }

        for file in &self.files {
            if let Err(error) = file.validate() {
                errors.extend(error);
            }
            if !paths.insert(file.path.clone()) {
                errors.push(format!(
                    "inventory contains duplicate path `{}`",
                    file.path.display()
                ));
            }
        }

        finish_validation(errors)
    }

    pub fn included_paths(&self) -> BTreeSet<PathBuf> {
        self.files
            .iter()
            .filter(|file| matches!(&file.disposition, FileDisposition::Include))
            .map(|file| file.path.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: PathBuf,
    pub kind: FileKind,
    pub language: Option<String>,
    pub size_bytes: u64,
    pub content_hash: Option<String>,
    pub disposition: FileDisposition,
}

impl FileEntry {
    pub fn validate(&self) -> ValidationResult {
        let mut errors = Vec::new();

        if let Err(error) = validate_portable_relative_path(&self.path) {
            errors.push(format!("invalid inventory path: {error}"));
        }
        match self.content_hash.as_deref() {
            Some(hash) if !is_sha256_fingerprint(hash) => errors.push(format!(
                "inventory file `{}` has a non-canonical SHA-256 content_hash",
                self.path.display()
            )),
            None if matches!(&self.disposition, FileDisposition::Include) => errors.push(format!(
                "included file `{}` must have a content_hash",
                self.path.display()
            )),
            _ => {}
        }
        if let FileDisposition::Exclude { reason } = &self.disposition
            && reason.trim().is_empty()
        {
            errors.push(format!(
                "excluded file `{}` must have a reason",
                self.path.display()
            ));
        }

        finish_validation(errors)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Source,
    Test,
    Configuration,
    BuildManifest,
    Script,
    Documentation,
    Asset,
    Generated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileDisposition {
    Include,
    Exclude { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocPlan {
    pub schema_version: u32,
    pub project_name: String,
    pub source_fingerprint: String,
    pub docs_root: PathBuf,
    pub language: String,
    /// Optional second complete language. Missing values preserve compatibility
    /// with schema-v2 single-language plans.
    #[serde(default)]
    pub secondary_language: Option<String>,
    pub audience: String,
    pub depth: DocumentationDepth,
    pub pages: Vec<PageSpec>,
}

impl DocPlan {
    pub fn validate(&self) -> ValidationResult {
        let mut errors = Vec::new();
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut case_folded_paths = BTreeMap::<String, PathBuf>::new();
        let mut site_routes = BTreeMap::<String, PathBuf>::new();
        let mut index_count = BTreeMap::<String, usize>::new();
        let mut file_atlas_count = BTreeMap::<String, usize>::new();

        validate_schema_version(self.schema_version, &mut errors);
        if self.project_name.trim().is_empty() {
            errors.push("plan project_name must not be empty".to_string());
        }
        validate_fingerprint(
            "plan source_fingerprint",
            &self.source_fingerprint,
            &mut errors,
        );
        if self.language.trim().is_empty() {
            errors.push("plan language must not be empty".to_string());
        }
        let primary_locale = normalized_locale(&self.language);
        if let Err(error) = &primary_locale {
            errors.push(format!("invalid plan language: {error}"));
        }
        let secondary_locale = self
            .secondary_language
            .as_deref()
            .map(normalized_locale)
            .transpose();
        if let Err(error) = &secondary_locale {
            errors.push(format!("invalid plan secondary_language: {error}"));
        }
        if let Ok(Some(secondary)) = &secondary_locale
            && let Err(error) = validate_locale_directory(secondary)
        {
            errors.push(format!("invalid plan secondary_language: {error}"));
        }
        if let (Ok(primary), Ok(Some(secondary))) = (&primary_locale, &secondary_locale)
            && primary == secondary
        {
            errors.push("plan secondary_language must differ from plan language".to_string());
        }
        if self.audience.trim().is_empty() {
            errors.push("plan audience must not be empty".to_string());
        }
        if let Err(error) = validate_portable_relative_path(&self.docs_root) {
            errors.push(format!("invalid plan docs_root: {error}"));
        }
        if self.pages.is_empty() {
            errors.push("plan must contain at least one page".to_string());
        }

        for page in &self.pages {
            if let Err(page_errors) = page.validate(&self.docs_root) {
                errors.extend(page_errors);
            }
            let page_language = self.page_language(page);
            match page_language {
                Ok(language) => {
                    let locale = normalized_locale(language)
                        .expect("a page language accepted by page_language has a locale");
                    let is_primary =
                        normalized_locale(&self.language).is_ok_and(|primary| primary == locale);
                    let expected_root = if is_primary {
                        self.docs_root.clone()
                    } else {
                        self.docs_root.join(&locale)
                    };
                    if !page.path.starts_with(&expected_root) {
                        errors.push(format!(
                            "page `{}` for language `{language}` must be under `{}`",
                            page.id,
                            expected_root.display()
                        ));
                    }
                    if is_primary
                        && let Ok(Some(secondary)) = &secondary_locale
                        && page.path.starts_with(self.docs_root.join(secondary))
                    {
                        errors.push(format!(
                            "primary-language page `{}` must remain in `{}` and not use the secondary locale directory",
                            page.id,
                            self.docs_root.display()
                        ));
                    }
                    if page.kind == PageKind::Index {
                        *index_count.entry(locale.clone()).or_default() += 1;
                        let expected_index = expected_root.join("index.md");
                        if page.path != expected_index {
                            errors.push(format!(
                                "index page `{}` for language `{language}` must use `{}`",
                                page.id,
                                expected_index.display()
                            ));
                        }
                    }
                    if page.kind == PageKind::FileAtlas {
                        *file_atlas_count.entry(locale).or_default() += 1;
                    }
                }
                Err(error) => errors.push(format!("page `{}` {error}", page.id)),
            }
            if !ids.insert(page.id.clone()) {
                errors.push(format!("plan contains duplicate page id `{}`", page.id));
            }
            if !paths.insert(page.path.clone()) {
                errors.push(format!(
                    "plan contains duplicate page path `{}`",
                    page.path.display()
                ));
            }
            let case_folded = portable_collision_key(&page.path);
            if let Some(previous) = case_folded_paths.insert(case_folded, page.path.clone())
                && previous != page.path
            {
                errors.push(format!(
                    "plan page paths `{}` and `{}` collide on case-insensitive filesystems",
                    previous.display(),
                    page.path.display()
                ));
            }
            if let Ok(relative) = page.path.strip_prefix(&self.docs_root) {
                let route = starlight_route_key(relative);
                if let Some(previous) = site_routes.insert(route.clone(), page.path.clone())
                    && previous != page.path
                {
                    errors.push(format!(
                        "plan page paths `{}` and `{}` collide on Starlight route `/{route}/`",
                        previous.display(),
                        page.path.display()
                    ));
                }
            }
        }

        for language in self.documentation_languages() {
            let locale = match normalized_locale(language) {
                Ok(locale) => locale,
                Err(_) => continue,
            };
            let indexes = index_count.get(&locale).copied().unwrap_or_default();
            if indexes != 1 {
                errors.push(format!(
                    "plan must contain exactly one index page for language `{language}`, found {indexes}"
                ));
            }
            let atlases = file_atlas_count.get(&locale).copied().unwrap_or_default();
            if atlases != 1 {
                errors.push(format!(
                    "plan must contain exactly one file_atlas page for language `{language}`, found {atlases}"
                ));
            }
        }

        if let (Ok(primary), Ok(Some(secondary))) = (&primary_locale, &secondary_locale)
            && primary != secondary
        {
            self.validate_translation_pairs(primary, secondary, &mut errors);
        }

        for page in &self.pages {
            for dependency in &page.depends_on {
                if dependency == &page.id {
                    errors.push(format!("page `{}` cannot depend on itself", page.id));
                } else if !ids.contains(dependency) {
                    errors.push(format!(
                        "page `{}` depends on unknown page `{dependency}`",
                        page.id
                    ));
                } else if let Some(dependency_page) = self.page(dependency)
                    && self.page_language(page).ok() != self.page_language(dependency_page).ok()
                {
                    errors.push(format!(
                        "page `{}` cannot depend on `{dependency}` from a different documentation language",
                        page.id
                    ));
                }
            }
        }
        validate_dependency_cycles(&self.pages, &mut errors);

        finish_validation(errors)
    }

    pub fn validate_against_inventory(&self, inventory: &Inventory) -> ValidationResult {
        let mut errors = Vec::new();

        if let Err(plan_errors) = self.validate() {
            errors.extend(plan_errors);
        }
        if let Err(inventory_errors) = inventory.validate() {
            errors.extend(inventory_errors);
        }
        if self.source_fingerprint != inventory.source_fingerprint {
            errors.push("plan and inventory source_fingerprint values differ".to_string());
        }

        let included = inventory.included_paths();
        for language in self.documentation_languages() {
            let planned = self
                .pages
                .iter()
                .filter(|page| {
                    self.page_language(page)
                        .is_ok_and(|value| value == language)
                })
                .flat_map(|page| page.source_files.iter().cloned())
                .collect::<BTreeSet<_>>();

            for path in included.difference(&planned) {
                errors.push(format!(
                    "included inventory file `{}` is not assigned to a page in language `{language}`",
                    path.display()
                ));
            }
            for path in planned.difference(&included) {
                errors.push(format!(
                    "page plan for language `{language}` references `{}` but it is not an included inventory file",
                    path.display()
                ));
            }

            let substantive = self
                .pages
                .iter()
                .filter(|page| {
                    self.page_language(page)
                        .is_ok_and(|value| value == language)
                        && page.kind.requires_source_coverage()
                        && page.kind != PageKind::FileAtlas
                })
                .flat_map(|page| page.source_files.iter().cloned())
                .collect::<BTreeSet<_>>();
            for path in included.difference(&substantive) {
                errors.push(format!(
                    "included inventory file `{}` is not assigned to a substantive page in language `{language}`",
                    path.display()
                ));
            }

            if let Some(file_atlas) = self.pages.iter().find(|page| {
                page.kind == PageKind::FileAtlas
                    && self
                        .page_language(page)
                        .is_ok_and(|value| value == language)
            }) {
                let atlas_files = file_atlas
                    .source_files
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                for path in included.difference(&atlas_files) {
                    errors.push(format!(
                        "file_atlas page for language `{language}` is missing included inventory file `{}`",
                        path.display()
                    ));
                }
            }
        }

        finish_validation(errors)
    }

    pub fn page(&self, id: &str) -> Option<&PageSpec> {
        self.pages.iter().find(|page| page.id == id)
    }

    fn validate_translation_pairs(
        &self,
        primary_locale: &str,
        secondary_locale: &str,
        errors: &mut Vec<String>,
    ) {
        let secondary_root = self.docs_root.join(secondary_locale);
        let mut primary_pages = BTreeMap::<PathBuf, &PageSpec>::new();
        let mut secondary_pages = BTreeMap::<PathBuf, &PageSpec>::new();

        for page in &self.pages {
            let Ok(language) = self.page_language(page) else {
                continue;
            };
            let Ok(locale) = normalized_locale(language) else {
                continue;
            };
            if locale == primary_locale {
                if let Ok(relative) = page.path.strip_prefix(&self.docs_root) {
                    primary_pages.insert(relative.to_path_buf(), page);
                }
            } else if locale == secondary_locale
                && let Ok(relative) = page.path.strip_prefix(&secondary_root)
            {
                secondary_pages.insert(relative.to_path_buf(), page);
            }
        }

        for (relative, primary) in &primary_pages {
            let Some(secondary) = secondary_pages.get(relative) else {
                errors.push(format!(
                    "bilingual plan is missing a secondary counterpart for primary page `{}` at locale-relative path `{}`",
                    primary.id,
                    relative.display()
                ));
                continue;
            };
            if primary.kind != secondary.kind {
                errors.push(format!(
                    "bilingual page pair `{}` and `{}` at locale-relative path `{}` must have the same PageKind, found {:?} and {:?}",
                    primary.id,
                    secondary.id,
                    relative.display(),
                    primary.kind,
                    secondary.kind
                ));
            }
            let primary_sources = primary.source_files.iter().collect::<BTreeSet<_>>();
            let secondary_sources = secondary.source_files.iter().collect::<BTreeSet<_>>();
            if primary_sources != secondary_sources {
                errors.push(format!(
                    "bilingual page pair `{}` and `{}` at locale-relative path `{}` must use the same source_files evidence set",
                    primary.id,
                    secondary.id,
                    relative.display()
                ));
            }
        }

        for (relative, secondary) in &secondary_pages {
            if !primary_pages.contains_key(relative) {
                errors.push(format!(
                    "bilingual plan is missing a primary counterpart for secondary page `{}` at locale-relative path `{}`",
                    secondary.id,
                    relative.display()
                ));
            }
        }
    }

    pub fn documentation_languages(&self) -> Vec<&str> {
        let mut languages = vec![self.language.as_str()];
        if let Some(secondary) = self.secondary_language.as_deref() {
            languages.push(secondary);
        }
        languages
    }

    /// Resolve legacy schema-v2 pages with no page-level language to the
    /// primary language, while requiring explicit markers for bilingual plans.
    pub fn page_language<'a>(&'a self, page: &'a PageSpec) -> Result<&'a str, String> {
        if page.language.trim().is_empty() {
            if self.secondary_language.is_none() {
                return Ok(self.language.as_str());
            }
            return Err("must declare a language in a bilingual plan".to_string());
        }
        let page_locale = normalized_locale(&page.language)?;
        for language in self.documentation_languages() {
            if normalized_locale(language).is_ok_and(|locale| locale == page_locale) {
                return Ok(language);
            }
        }
        Err(format!(
            "declares language `{}` outside the plan language set",
            page.language
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlgorithmSection {
    pub facet: AlgorithmFacet,
    pub heading: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlgorithmFacet {
    Problem,
    PrincipleOrEquations,
    DataShapesOrStateTransitions,
    Implementation,
    Complexity,
    InvariantsOrFailureModes,
    TestsOrReferenceEvidence,
}

impl AlgorithmFacet {
    pub const ALL: [Self; 7] = [
        Self::Problem,
        Self::PrincipleOrEquations,
        Self::DataShapesOrStateTransitions,
        Self::Implementation,
        Self::Complexity,
        Self::InvariantsOrFailureModes,
        Self::TestsOrReferenceEvidence,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Problem => "problem",
            Self::PrincipleOrEquations => "principle_or_equations",
            Self::DataShapesOrStateTransitions => "data_shapes_or_state_transitions",
            Self::Implementation => "implementation",
            Self::Complexity => "complexity",
            Self::InvariantsOrFailureModes => "invariants_or_failure_modes",
            Self::TestsOrReferenceEvidence => "tests_or_reference_evidence",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSpec {
    pub id: String,
    pub path: PathBuf,
    /// Exact requested documentation language for this page. The serde default
    /// only exists so older single-language manifests remain readable.
    #[serde(default)]
    pub language: String,
    pub title: String,
    pub kind: PageKind,
    pub purpose: String,
    pub covers: Vec<String>,
    pub source_files: Vec<PathBuf>,
    pub topics: Vec<String>,
    pub algorithms: Vec<String>,
    #[serde(default)]
    pub algorithm_sections: Vec<AlgorithmSection>,
    pub required_sections: Vec<String>,
    pub depends_on: Vec<String>,
    pub managed: bool,
}

impl PageSpec {
    pub fn validate(&self, docs_root: &Path) -> ValidationResult {
        let mut errors = Vec::new();

        if !is_valid_page_id(&self.id) {
            errors.push(format!(
                "page id `{}` must contain only lowercase ASCII letters, digits, `-`, or `_`",
                self.id
            ));
        }
        if self.title.trim().is_empty() {
            errors.push(format!("page `{}` must have a title", self.id));
        }
        if self.purpose.trim().is_empty() {
            errors.push(format!("page `{}` must have a purpose", self.id));
        }
        if self.required_sections.is_empty() {
            errors.push(format!(
                "page `{}` must define at least one required section",
                self.id
            ));
        }
        if let Err(error) = validate_docs_page_path(&self.path, docs_root) {
            errors.push(format!("page `{}` has invalid path: {error}", self.id));
        }
        if self.kind.requires_source_coverage() && self.source_files.is_empty() {
            errors.push(format!(
                "page `{}` of kind {:?} must reference source files",
                self.id, self.kind
            ));
        }
        if !self.source_files.is_empty() && self.covers.is_empty() {
            errors.push(format!(
                "page `{}` references source files but has no covers patterns",
                self.id
            ));
        }

        if self.kind == PageKind::Algorithm {
            if self.topics.is_empty() {
                errors.push(format!(
                    "algorithm page `{}` must declare concrete topics",
                    self.id
                ));
            }
            if self.algorithms.is_empty() {
                errors.push(format!(
                    "algorithm page `{}` must declare at least one implemented algorithm",
                    self.id
                ));
            }
            let mut facets = BTreeSet::new();
            let mut headings = BTreeSet::new();
            for section in &self.algorithm_sections {
                if section.heading.trim().is_empty() {
                    errors.push(format!(
                        "algorithm page `{}` has an empty heading for facet `{}`",
                        self.id,
                        section.facet.as_str()
                    ));
                }
                let normalized_heading = gfm_slug(&section.heading);
                if !normalized_heading.is_empty() && !headings.insert(normalized_heading) {
                    errors.push(format!(
                        "algorithm page `{}` maps multiple facets to heading `{}`",
                        self.id, section.heading
                    ));
                }
                if !facets.insert(section.facet) {
                    errors.push(format!(
                        "algorithm page `{}` declares facet `{}` more than once",
                        self.id,
                        section.facet.as_str()
                    ));
                }
                if !self
                    .required_sections
                    .iter()
                    .any(|heading| heading == &section.heading)
                {
                    errors.push(format!(
                        "algorithm page `{}` facet `{}` maps to heading `{}` that is absent from required_sections",
                        self.id,
                        section.facet.as_str(),
                        section.heading
                    ));
                }
            }
            for facet in AlgorithmFacet::ALL {
                if !facets.contains(&facet) {
                    errors.push(format!(
                        "algorithm page `{}` is missing required facet `{}`",
                        self.id,
                        facet.as_str()
                    ));
                }
            }
        } else if !self.algorithm_sections.is_empty() {
            errors.push(format!(
                "non-algorithm page `{}` must not declare algorithm_sections",
                self.id
            ));
        }

        validate_unique_strings(&self.id, "covers", &self.covers, &mut errors);
        validate_unique_strings(&self.id, "topics", &self.topics, &mut errors);
        validate_unique_strings(&self.id, "algorithms", &self.algorithms, &mut errors);
        validate_unique_strings(
            &self.id,
            "required_sections",
            &self.required_sections,
            &mut errors,
        );
        validate_unique_strings(&self.id, "depends_on", &self.depends_on, &mut errors);
        validate_patterns("covers", &self.covers, &mut errors);

        let mut source_files = BTreeSet::new();
        for path in &self.source_files {
            if let Err(error) = validate_portable_relative_path(path) {
                errors.push(format!(
                    "page `{}` has invalid source file `{}`: {error}",
                    self.id,
                    path.display()
                ));
            }
            if !source_files.insert(path.clone()) {
                errors.push(format!(
                    "page `{}` lists source file `{}` more than once",
                    self.id,
                    path.display()
                ));
            }
        }

        finish_validation(errors)
    }
}

/// Convert a language tag to the portable directory form used for localized
/// documentation. This intentionally accepts common BCP-47-style tags and
/// normalizes underscores/case without pretending to be a full BCP-47 parser.
pub fn normalized_locale(language: &str) -> Result<String, String> {
    let value = language.trim();
    if value.is_empty() {
        return Err("language tag must not be empty".to_string());
    }
    let mut locale = String::with_capacity(value.len());
    let mut previous_separator = false;
    for character in value.chars() {
        match character {
            'A'..='Z' => {
                locale.push(character.to_ascii_lowercase());
                previous_separator = false;
            }
            'a'..='z' | '0'..='9' => {
                locale.push(character);
                previous_separator = false;
            }
            '-' | '_' if !locale.is_empty() && !previous_separator => {
                locale.push('-');
                previous_separator = true;
            }
            _ => {
                return Err(format!(
                    "language tag `{value}` may contain only ASCII letters, digits, `-`, or `_`"
                ));
            }
        }
    }
    if locale.ends_with('-') {
        return Err(format!(
            "language tag `{value}` must not end with a separator"
        ));
    }
    Ok(locale)
}

fn validate_locale_directory(locale: &str) -> Result<(), String> {
    if locale == "root" {
        return Err("normalized locale `root` is reserved by Starlight".to_string());
    }
    validate_portable_relative_path(Path::new(locale))
        .map_err(|error| format!("locale directory `{locale}` is not portable: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    Index,
    Overview,
    Architecture,
    FileAtlas,
    Algorithm,
    Api,
    Cli,
    Configuration,
    Workflow,
    Testing,
    Operations,
    Reference,
    Glossary,
}

impl PageKind {
    pub(crate) fn requires_source_coverage(self) -> bool {
        !matches!(self, Self::Index | Self::Overview | Self::Glossary)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedPage {
    pub page_id: String,
    pub path: PathBuf,
    pub markdown: String,
    pub source_fingerprint: String,
}

impl GeneratedPage {
    pub fn validate_against(&self, spec: &PageSpec, docs_root: &Path) -> ValidationResult {
        let mut errors = Vec::new();

        if self.page_id != spec.id {
            errors.push(format!(
                "generated page id `{}` does not match planned id `{}`",
                self.page_id, spec.id
            ));
        }
        if self.path != spec.path {
            errors.push(format!(
                "generated path `{}` does not match planned path `{}`",
                self.path.display(),
                spec.path.display()
            ));
        }
        if let Err(error) = validate_docs_page_path(&self.path, docs_root) {
            errors.push(format!("generated page has invalid path: {error}"));
        }
        if self.markdown.trim().is_empty() {
            errors.push(format!(
                "generated page `{}` has empty Markdown content",
                self.page_id
            ));
        }
        if !self.markdown.trim_start().starts_with("---") {
            errors.push(format!(
                "generated page `{}` must start with frontmatter",
                self.page_id
            ));
        }
        if !is_sha256_fingerprint(&self.source_fingerprint) {
            errors.push(format!(
                "generated page `{}` must record a canonical SHA-256 source_fingerprint",
                self.page_id
            ));
        }

        finish_validation(errors)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedManifest {
    pub schema_version: u32,
    pub generated_by: String,
    pub plan: DocPlan,
    pub pages: BTreeMap<String, ManagedPage>,
}

impl ManagedManifest {
    pub fn validate(&self) -> ValidationResult {
        let mut errors = Vec::new();

        validate_schema_version(self.schema_version, &mut errors);
        if self.generated_by.trim().is_empty() {
            errors.push("manifest generated_by must not be empty".to_string());
        }
        if let Err(plan_errors) = self.plan.validate() {
            errors.extend(plan_errors);
        }

        let planned_ids = self
            .plan
            .pages
            .iter()
            .map(|page| page.id.as_str())
            .collect::<BTreeSet<_>>();
        let managed_ids = self
            .pages
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();

        for id in planned_ids.difference(&managed_ids) {
            errors.push(format!("manifest is missing managed page state for `{id}`"));
        }
        for id in managed_ids.difference(&planned_ids) {
            errors.push(format!(
                "manifest has unplanned managed page state for `{id}`"
            ));
        }
        for (id, page) in &self.pages {
            if let Some(spec) = self.plan.page(id) {
                if page.path != spec.path {
                    errors.push(format!(
                        "manifest page `{id}` path `{}` does not match plan path `{}`",
                        page.path.display(),
                        spec.path.display()
                    ));
                }
                if page.source_fingerprint != self.plan.source_fingerprint {
                    errors.push(format!(
                        "manifest page `{id}` has a different source_fingerprint from its plan"
                    ));
                }
            }
            if !is_sha256_fingerprint(&page.content_hash) {
                errors.push(format!(
                    "manifest page `{id}` has a non-canonical SHA-256 content_hash"
                ));
            }
            if !is_sha256_fingerprint(&page.source_fingerprint) {
                errors.push(format!(
                    "manifest page `{id}` has a non-canonical SHA-256 source_fingerprint"
                ));
            }
        }

        finish_validation(errors)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPage {
    pub path: PathBuf,
    pub content_hash: String,
    pub source_fingerprint: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunState {
    pub schema_version: u32,
    pub run_id: String,
    pub source_fingerprint: String,
    #[serde(default)]
    pub config_fingerprint: Option<String>,
    pub plan_fingerprint: Option<String>,
    pub phase: RunPhase,
    pub status: RunStatus,
    pub completed_pages: BTreeSet<String>,
    pub pending_pages: BTreeSet<String>,
    pub failed_pages: BTreeMap<String, String>,
    pub attempts: BTreeMap<String, u8>,
    pub last_error: Option<String>,
}

impl RunState {
    pub fn validate(&self, plan: Option<&DocPlan>) -> ValidationResult {
        let mut errors = Vec::new();

        validate_schema_version(self.schema_version, &mut errors);
        if self.run_id.trim().is_empty() {
            errors.push("run_id must not be empty".to_string());
        }
        if self.phase == RunPhase::Discover {
            if self.source_fingerprint != "pending" {
                errors.push(
                    "discover-phase run source_fingerprint must be the exact `pending` sentinel"
                        .to_string(),
                );
            }
        } else {
            validate_fingerprint(
                "run source_fingerprint",
                &self.source_fingerprint,
                &mut errors,
            );
        }
        if let Some(fingerprint) = self.config_fingerprint.as_deref()
            && !is_sha256_fingerprint(fingerprint)
        {
            errors.push(
                "run config_fingerprint must be a canonical SHA-256 fingerprint when present"
                    .to_string(),
            );
        }
        if let Some(fingerprint) = self.plan_fingerprint.as_deref()
            && !is_sha256_fingerprint(fingerprint)
        {
            errors.push(
                "run plan_fingerprint must be a canonical SHA-256 fingerprint when present"
                    .to_string(),
            );
        }
        if self.phase == RunPhase::Complete && self.status != RunStatus::Completed {
            errors.push("complete phase requires completed status".to_string());
        }
        if self.status == RunStatus::Completed
            && (!self.pending_pages.is_empty() || !self.failed_pages.is_empty())
        {
            errors.push("completed run cannot contain pending pages or failed pages".to_string());
        }
        if self.status == RunStatus::Failed
            && self
                .last_error
                .as_deref()
                .is_none_or(|message| message.trim().is_empty())
        {
            errors.push("failed run must record last_error".to_string());
        }

        for id in self.completed_pages.intersection(&self.pending_pages) {
            errors.push(format!("page `{id}` is both completed and pending"));
        }
        for id in self.completed_pages.intersection(
            &self
                .failed_pages
                .keys()
                .cloned()
                .collect::<BTreeSet<String>>(),
        ) {
            errors.push(format!("page `{id}` is both completed and failed"));
        }
        for (id, reason) in &self.failed_pages {
            if reason.trim().is_empty() {
                errors.push(format!("failed page `{id}` must have an error message"));
            }
            if self.attempts.get(id).copied().unwrap_or_default() == 0 {
                errors.push(format!(
                    "failed page `{id}` must record at least one generation attempt"
                ));
            }
        }

        if let Some(plan) = plan {
            if self.source_fingerprint != plan.source_fingerprint {
                errors.push("run and plan source_fingerprint values differ".to_string());
            }
            let plan_ids = plan
                .pages
                .iter()
                .map(|page| page.id.as_str())
                .collect::<BTreeSet<_>>();
            for id in self
                .completed_pages
                .iter()
                .chain(self.pending_pages.iter())
                .chain(self.failed_pages.keys())
            {
                if !plan_ids.contains(id.as_str()) {
                    errors.push(format!("run references unknown page `{id}`"));
                }
            }
        }

        finish_validation(errors)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Discover,
    Plan,
    Generate,
    Integrate,
    Verify,
    Repair,
    Snapshot,
    Check,
    Site,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Failed,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationReport {
    pub schema_version: u32,
    pub source_fingerprint: String,
    pub status: VerificationStatus,
    pub summary: String,
    pub checks: Vec<VerificationCheck>,
    pub page_results: BTreeMap<String, PageVerification>,
    pub uncovered_files: Vec<PathBuf>,
    pub broken_links: Vec<BrokenLink>,
    pub issues: Vec<VerificationIssue>,
}

impl VerificationReport {
    pub fn validate_against(&self, plan: &DocPlan) -> ValidationResult {
        let mut errors = Vec::new();

        validate_schema_version(self.schema_version, &mut errors);
        validate_fingerprint(
            "verification source_fingerprint",
            &self.source_fingerprint,
            &mut errors,
        );
        if self.source_fingerprint != plan.source_fingerprint {
            errors.push("verification and plan source_fingerprint values differ".to_string());
        }

        let plan_ids = plan
            .pages
            .iter()
            .map(|page| page.id.as_str())
            .collect::<BTreeSet<_>>();
        for id in self.page_results.keys() {
            if !plan_ids.contains(id.as_str()) {
                errors.push(format!("verification references unknown page `{id}`"));
            }
        }
        for issue in &self.issues {
            if let Some(id) = issue.page_id.as_deref()
                && !plan_ids.contains(id)
            {
                errors.push(format!("verification issue references unknown page `{id}`"));
            }
        }
        for path in &self.uncovered_files {
            if let Err(error) = validate_portable_relative_path(path) {
                errors.push(format!(
                    "verification contains invalid uncovered path `{}`: {error}",
                    path.display()
                ));
            }
        }

        let has_failed_check = self
            .checks
            .iter()
            .any(|check| check.status == VerificationStatus::Failed);
        let has_error = self
            .issues
            .iter()
            .any(|issue| issue.severity == VerificationSeverity::Error);
        if self.status == VerificationStatus::Passed
            && (has_failed_check
                || has_error
                || !self.uncovered_files.is_empty()
                || !self.broken_links.is_empty())
        {
            errors.push(
                "passed verification cannot contain failed checks, errors, uncovered files, or broken links"
                    .to_string(),
            );
        }

        finish_validation(errors)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCheck {
    pub name: String,
    pub status: VerificationStatus,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageVerification {
    pub status: VerificationStatus,
    pub summary: String,
    pub required_repairs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokenLink {
    pub page_id: String,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationIssue {
    pub code: String,
    pub severity: VerificationSeverity,
    pub page_id: Option<String>,
    pub message: String,
    pub repairable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationSeverity {
    Info,
    Warning,
    Error,
}

pub fn validate_portable_relative_path(path: &Path) -> Result<(), String> {
    let raw = path.to_string_lossy();
    if raw.is_empty() {
        return Err("path must not be empty".to_string());
    }
    if path.is_absolute() || raw.starts_with('/') {
        return Err(format!("`{}` must be relative", path.display()));
    }
    if raw.contains('\\') {
        return Err(format!(
            "`{}` must use forward slashes for portable manifest paths",
            path.display()
        ));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir | Component::CurDir
        )
    }) {
        return Err(format!(
            "`{}` must not contain root, parent, or current-directory components",
            path.display()
        ));
    }
    if raw.split('/').any(|segment| segment.is_empty()) {
        return Err(format!(
            "`{}` contains an empty path segment",
            path.display()
        ));
    }
    for segment in raw.split('/') {
        validate_portable_segment(segment, path)?;
    }
    Ok(())
}

pub fn validate_docs_page_path(path: &Path, docs_root: &Path) -> Result<(), String> {
    validate_portable_relative_path(docs_root)?;
    validate_portable_relative_path(path)?;

    let relative = path.strip_prefix(docs_root).map_err(|_| {
        format!(
            "`{}` must be inside docs root `{}`",
            path.display(),
            docs_root.display()
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Err(format!("`{}` is a directory, not a page", path.display()));
    }
    if path
        .to_string_lossy()
        .chars()
        .any(|character| character.is_uppercase())
    {
        return Err(format!(
            "`{}` must use lowercase page paths for portable routes",
            path.display()
        ));
    }
    if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
        return Err(format!(
            "`{}` must end in .md; managed MDX is executable and is not accepted",
            path.display()
        ));
    }
    Ok(())
}

fn validate_portable_segment(segment: &str, path: &Path) -> Result<(), String> {
    if segment.nfc().collect::<String>() != segment {
        return Err(format!(
            "`{}` must use NFC-normalized Unicode path segments",
            path.display()
        ));
    }
    if segment.ends_with(['.', ' ']) {
        return Err(format!(
            "`{}` has a path segment ending in a dot or space",
            path.display()
        ));
    }
    if segment.chars().any(|character| {
        character.is_control() || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
    }) {
        return Err(format!(
            "`{}` contains characters that are illegal in portable paths",
            path.display()
        ));
    }
    let device_stem = segment
        .split_once('.')
        .map_or(segment, |(stem, _)| stem)
        .to_ascii_lowercase();
    let reserved = matches!(device_stem.as_str(), "con" | "prn" | "aux" | "nul")
        || device_stem
            .strip_prefix("com")
            .or_else(|| device_stem.strip_prefix("lpt"))
            .is_some_and(|number| number.len() == 1 && matches!(number.as_bytes()[0], b'1'..=b'9'));
    if reserved {
        return Err(format!(
            "`{}` contains Windows-reserved device name `{segment}`",
            path.display()
        ));
    }
    Ok(())
}

fn portable_collision_key(path: &Path) -> String {
    path.to_string_lossy()
        .nfc()
        .flat_map(char::to_lowercase)
        .collect()
}

/// Match the route normalization used by the generated Starlight adapter so
/// collisions are rejected before any Markdown is authored or published.
fn starlight_route_key(path: &Path) -> String {
    let mut components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(last) = components.last_mut()
        && let Some((stem, _)) = last.rsplit_once('.')
    {
        *last = stem.to_string();
    }
    if components.last().is_some_and(|value| value == "index") && components.len() > 1 {
        components.pop();
    }
    let route = components
        .into_iter()
        .map(|component| starlight_slug_component(&component))
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if route.is_empty() {
        "index".to_string()
    } else {
        route
    }
}

fn starlight_slug_component(value: &str) -> String {
    let mut output = String::new();
    let mut separated = false;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if separated && !output.is_empty() {
                output.push('-');
            }
            output.extend(character.to_lowercase());
            separated = false;
        } else {
            separated = true;
        }
    }
    output
}

fn validate_schema_version(version: u32, errors: &mut Vec<String>) {
    if version != GENERATION_SCHEMA_VERSION {
        errors.push(format!(
            "unsupported schema_version {version}; expected {GENERATION_SCHEMA_VERSION}"
        ));
    }
}

fn validate_fingerprint(label: &str, value: &str, errors: &mut Vec<String>) {
    if !is_sha256_fingerprint(value) {
        errors.push(format!(
            "{label} must use canonical SHA-256 form `sha256:<64 lowercase hex digits>`"
        ));
    }
}

fn validate_patterns(label: &str, patterns: &[String], errors: &mut Vec<String>) {
    for pattern in patterns {
        if let Err(error) = validate_portable_pattern(pattern) {
            errors.push(format!("invalid {label} pattern `{pattern}`: {error}"));
        }
    }
}

fn validate_portable_pattern(pattern: &str) -> Result<(), String> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err("pattern must not be empty".to_string());
    }
    if trimmed.starts_with('/') || trimmed.contains('\\') {
        return Err("pattern must be a portable relative path".to_string());
    }
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err("pattern contains an empty, parent, or current-directory segment".to_string());
    }
    Ok(())
}

fn validate_unique_strings(
    page_id: &str,
    field: &str,
    values: &[String],
    errors: &mut Vec<String>,
) {
    let mut seen = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() {
            errors.push(format!("page `{page_id}` has an empty {field} value"));
        } else if !seen.insert(value) {
            errors.push(format!(
                "page `{page_id}` contains duplicate {field} value `{value}`"
            ));
        }
    }
}

pub(crate) fn gfm_slug(heading: &str) -> String {
    let visible = heading_visible_text(heading);
    let mut slug = String::new();
    for character in visible.trim().chars().flat_map(char::to_lowercase) {
        if character.is_whitespace() {
            slug.push('-');
        } else if !is_gfm_punctuation(character) {
            slug.push(character);
        }
    }
    slug
}

fn heading_visible_text(heading: &str) -> String {
    let bytes = heading.as_bytes();
    let mut visible = String::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b']' && bytes.get(index + 1) == Some(&b'(') {
            index += 2;
            let mut depth = 1;
            while index < bytes.len() && depth > 0 {
                match bytes[index] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'<'
            && let Some(end) = heading[index + 1..].find('>')
        {
            index += end + 2;
            continue;
        }
        let character = heading[index..]
            .chars()
            .next()
            .expect("valid UTF-8 boundary");
        visible.push(character);
        index += character.len_utf8();
    }
    visible
}

fn is_gfm_punctuation(character: char) -> bool {
    (character.is_ascii_punctuation() && !matches!(character, '-' | '_'))
        || matches!(character, '\u{2000}'..='\u{206f}' | '\u{2e00}'..='\u{2e7f}')
}

fn is_valid_page_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
}

fn validate_dependency_cycles(pages: &[PageSpec], errors: &mut Vec<String>) {
    let dependencies = pages
        .iter()
        .map(|page| {
            (
                page.id.as_str(),
                page.depends_on
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    for page in pages {
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        if dependency_cycle_from(page.id.as_str(), &dependencies, &mut visiting, &mut visited) {
            errors.push(format!(
                "page dependency graph contains a cycle reachable from `{}`",
                page.id
            ));
            break;
        }
    }
}

fn dependency_cycle_from<'a>(
    id: &'a str,
    dependencies: &BTreeMap<&'a str, Vec<&'a str>>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> bool {
    if visited.contains(id) {
        return false;
    }
    if !visiting.insert(id) {
        return true;
    }
    if let Some(children) = dependencies.get(id) {
        for &child in children {
            if dependency_cycle_from(child, dependencies, visiting, visited) {
                return true;
            }
        }
    }
    visiting.remove(id);
    visited.insert(id);
    false
}

fn finish_validation(errors: Vec<String>) -> ValidationResult {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(label: &str) -> String {
        crate::integrity::sha256_fingerprint(label.as_bytes())
    }

    #[test]
    fn upload_scope_fingerprint_requires_explicit_upload_acknowledgement() {
        let mut config = GenerateConfig {
            upload_scope_fingerprint: Some(fingerprint("scope")),
            ..GenerateConfig::default()
        };
        let errors = config.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("requires allow_code_upload"))
        );

        config.allow_code_upload = true;
        assert_eq!(config.validate(), Ok(()));
        config.upload_scope_fingerprint = Some("not-a-sha256".to_string());
        assert!(
            config
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.contains("canonical SHA-256"))
        );
    }

    fn page(id: &str, path: &str, kind: PageKind) -> PageSpec {
        PageSpec {
            id: id.to_string(),
            path: PathBuf::from(path),
            language: "en".to_string(),
            title: id.to_string(),
            kind,
            purpose: format!("Explain {id}"),
            covers: if kind.requires_source_coverage() {
                vec!["src/**".to_string()]
            } else {
                Vec::new()
            },
            source_files: if kind.requires_source_coverage() {
                vec![PathBuf::from("src/lib.rs")]
            } else {
                Vec::new()
            },
            topics: Vec::new(),
            algorithms: Vec::new(),
            algorithm_sections: Vec::new(),
            required_sections: vec!["Purpose".to_string()],
            depends_on: Vec::new(),
            managed: true,
        }
    }

    fn plan(pages: Vec<PageSpec>) -> DocPlan {
        DocPlan {
            schema_version: GENERATION_SCHEMA_VERSION,
            project_name: "example".to_string(),
            source_fingerprint: fingerprint("source-1"),
            docs_root: PathBuf::from("docs"),
            language: "en".to_string(),
            secondary_language: None,
            audience: "developers".to_string(),
            depth: DocumentationDepth::Deep,
            pages,
        }
    }

    #[test]
    fn rejects_path_traversal_and_absolute_page_paths() {
        assert!(validate_portable_relative_path(Path::new("../src/lib.rs")).is_err());
        assert!(validate_portable_relative_path(Path::new("/tmp/page.md")).is_err());
        assert!(validate_docs_page_path(Path::new("src/page.md"), Path::new("docs")).is_err());
        assert!(validate_docs_page_path(Path::new("docs/page.md"), Path::new("docs")).is_ok());
        assert!(validate_docs_page_path(Path::new("docs/page.mdx"), Path::new("docs")).is_err());
    }

    #[test]
    fn rejects_nonportable_reserved_illegal_and_non_normalized_paths() {
        for path in [
            "docs/CON.md",
            "docs/lpt9.txt",
            "docs/trailing. ",
            "docs/bad:name.md",
            "docs/question?.md",
            "docs/cafe\u{301}.md",
        ] {
            assert!(
                validate_portable_relative_path(Path::new(path)).is_err(),
                "{path}"
            );
        }
        assert!(validate_portable_relative_path(Path::new("src/models/Gemma.rs")).is_ok());
        assert!(validate_portable_relative_path(Path::new("docs/café.md")).is_ok());
    }

    #[test]
    fn generated_page_routes_must_be_lowercase() {
        assert!(validate_docs_page_path(Path::new("docs/Guide.md"), Path::new("docs")).is_err());
        assert!(validate_docs_page_path(Path::new("docs/guide.md"), Path::new("docs")).is_ok());
    }

    #[test]
    fn valid_plan_has_one_index_and_unique_pages() {
        let plan = plan(vec![
            page("index", "docs/index.md", PageKind::Index),
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
            page(
                "architecture",
                "docs/architecture.md",
                PageKind::Architecture,
            ),
        ]);

        assert_eq!(plan.validate(), Ok(()));
    }

    #[test]
    fn legacy_single_language_json_defaults_new_bilingual_fields() {
        let original = plan(vec![
            page("index", "docs/index.md", PageKind::Index),
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
            page(
                "architecture",
                "docs/architecture.md",
                PageKind::Architecture,
            ),
        ]);
        let mut value = serde_json::to_value(&original).unwrap();
        value.as_object_mut().unwrap().remove("secondary_language");
        for page in value["pages"].as_array_mut().unwrap() {
            page.as_object_mut().unwrap().remove("language");
        }

        let restored: DocPlan = serde_json::from_value(value).unwrap();

        assert_eq!(restored.secondary_language, None);
        assert!(restored.pages.iter().all(|page| page.language.is_empty()));
        assert_eq!(restored.validate(), Ok(()));

        let mut config_value = serde_json::to_value(GenerateConfig::default()).unwrap();
        config_value
            .as_object_mut()
            .unwrap()
            .remove("secondary_language");
        let restored_config: GenerateConfig = serde_json::from_value(config_value).unwrap();
        assert_eq!(restored_config.secondary_language, None);
        assert_eq!(restored_config.validate(), Ok(()));
    }

    #[test]
    fn normalizes_portable_locale_tags() {
        assert_eq!(normalized_locale("zh-CN"), Ok("zh-cn".to_string()));
        assert_eq!(normalized_locale("PT_br"), Ok("pt-br".to_string()));
        assert!(normalized_locale("简体中文").is_err());
        assert!(normalized_locale("en--US").is_err());
        assert!(normalized_locale("en-").is_err());
    }

    #[test]
    fn secondary_locale_rejects_starlight_and_filesystem_reserved_directories() {
        for language in ["root", "CON", "lpt1"] {
            let config = GenerateConfig {
                secondary_language: Some(language.to_string()),
                ..GenerateConfig::default()
            };
            let errors = config.validate().unwrap_err().join("\n");
            assert!(
                errors.contains("secondary_language"),
                "{language}: {errors}"
            );
        }
    }

    #[test]
    fn bilingual_plan_requires_one_to_one_translation_pairs() {
        let mut bilingual = plan(vec![
            page(
                "architecture",
                "docs/architecture.md",
                PageKind::Architecture,
            ),
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
            page("index", "docs/index.md", PageKind::Index),
            page(
                "zh-cn-architecture",
                "docs/zh-cn/architecture.md",
                PageKind::Architecture,
            ),
            page(
                "zh-cn-file-atlas",
                "docs/zh-cn/file-atlas.md",
                PageKind::FileAtlas,
            ),
            page("zh-cn-index", "docs/zh-cn/index.md", PageKind::Index),
        ]);
        bilingual.secondary_language = Some("zh-CN".to_string());
        for page in bilingual.pages.iter_mut().skip(3) {
            page.language = "zh-CN".to_string();
        }

        assert_eq!(bilingual.validate(), Ok(()));

        let mut mismatched_path = bilingual.clone();
        mismatched_path.pages[3].path = PathBuf::from("docs/zh-cn/runtime/architecture.md");
        let errors = mismatched_path.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("missing a secondary counterpart")
                    && error.contains("architecture.md"))
        );

        let mut mismatched_kind = bilingual.clone();
        mismatched_kind.pages[3].kind = PageKind::Api;
        let errors = mismatched_kind.validate().unwrap_err();
        assert!(errors.iter().any(|error| {
            error.contains("same PageKind")
                && error.contains("architecture")
                && error.contains("zh-cn-architecture")
        }));

        let mut mismatched_sources = bilingual.clone();
        mismatched_sources.pages[3]
            .source_files
            .push(PathBuf::from("tests/reference.rs"));
        let errors = mismatched_sources.validate().unwrap_err();
        assert!(errors.iter().any(|error| {
            error.contains("same source_files evidence set")
                && error.contains("architecture")
                && error.contains("zh-cn-architecture")
        }));

        let mut secondary_only = bilingual;
        let mut extra = page(
            "zh-cn-extra-guide",
            "docs/zh-cn/extra-guide.md",
            PageKind::Overview,
        );
        extra.language = "zh-CN".to_string();
        secondary_only.pages.push(extra);
        let errors = secondary_only.validate().unwrap_err();
        assert!(errors.iter().any(|error| {
            error.contains("missing a primary counterpart") && error.contains("extra-guide.md")
        }));
    }

    #[test]
    fn bilingual_inventory_coverage_is_required_per_language() {
        let mut bilingual = plan(vec![
            page(
                "architecture",
                "docs/architecture.md",
                PageKind::Architecture,
            ),
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
            page("index", "docs/index.md", PageKind::Index),
            page(
                "zh-cn-architecture",
                "docs/zh-cn/architecture.md",
                PageKind::Architecture,
            ),
            page(
                "zh-cn-file-atlas",
                "docs/zh-cn/file-atlas.md",
                PageKind::FileAtlas,
            ),
            page("zh-cn-index", "docs/zh-cn/index.md", PageKind::Index),
        ]);
        bilingual.secondary_language = Some("zh-CN".to_string());
        for page in bilingual.pages.iter_mut().skip(3) {
            page.language = "zh-CN".to_string();
        }
        bilingual.pages[3].source_files.clear();
        bilingual.pages[3].covers.clear();
        bilingual.pages[4].source_files.clear();
        bilingual.pages[4].covers.clear();
        let inventory = Inventory {
            schema_version: GENERATION_SCHEMA_VERSION,
            project_name: "example".to_string(),
            source_revision: None,
            source_fingerprint: bilingual.source_fingerprint.clone(),
            languages: BTreeSet::from(["Rust".to_string()]),
            files: vec![FileEntry {
                path: PathBuf::from("src/lib.rs"),
                kind: FileKind::Source,
                language: Some("Rust".to_string()),
                size_bytes: 1,
                content_hash: Some(fingerprint("lib")),
                disposition: FileDisposition::Include,
            }],
        };

        let errors = bilingual
            .validate_against_inventory(&inventory)
            .unwrap_err();
        assert!(
            errors.iter().any(|error| {
                error.contains("language `zh-CN`") && error.contains("not assigned")
            })
        );
    }

    #[test]
    fn persisted_models_reject_legacy_unqualified_hashes() {
        let mut value = plan(vec![
            page("index", "docs/index.md", PageKind::Index),
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
        ]);
        value.source_fingerprint = "e71fa2190541574b".to_string();

        assert!(
            value
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.contains("canonical SHA-256"))
        );

        let file = FileEntry {
            path: PathBuf::from("src/lib.rs"),
            kind: FileKind::Source,
            language: Some("Rust".to_string()),
            size_bytes: 1,
            content_hash: Some("e71fa2190541574b".to_string()),
            disposition: FileDisposition::Include,
        };
        assert!(
            file.validate()
                .unwrap_err()
                .iter()
                .any(|error| error.contains("canonical SHA-256"))
        );
    }

    #[test]
    fn plan_requires_exactly_one_file_atlas() {
        let missing = plan(vec![page("index", "docs/index.md", PageKind::Index)]);
        assert!(
            missing
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.contains("exactly one file_atlas"))
        );

        let duplicate = plan(vec![
            page("index", "docs/index.md", PageKind::Index),
            page("atlas-a", "docs/atlas-a.md", PageKind::FileAtlas),
            page("atlas-b", "docs/atlas-b.md", PageKind::FileAtlas),
        ]);
        assert!(
            duplicate
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.contains("found 2"))
        );
    }

    #[test]
    fn plan_rejects_case_insensitive_page_path_collisions() {
        let value = plan(vec![
            page("index", "docs/index.md", PageKind::Index),
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
            page("guide-upper", "docs/Guide.md", PageKind::Architecture),
            page("guide-lower", "docs/guide.md", PageKind::Api),
        ]);

        assert!(
            value
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.contains("case-insensitive"))
        );
    }

    #[test]
    fn plan_rejects_starlight_route_collisions() {
        for (left, right, route) in [
            ("docs/foo_bar.md", "docs/foo-bar.md", "/foo-bar/"),
            ("docs/foo.md", "docs/foo/index.md", "/foo/"),
        ] {
            let value = plan(vec![
                page("index", "docs/index.md", PageKind::Index),
                page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
                page("route-left", left, PageKind::Overview),
                page("route-right", right, PageKind::Overview),
            ]);

            let errors = value.validate().unwrap_err();
            assert!(errors.iter().any(|error| {
                error.contains("collide on Starlight route") && error.contains(route)
            }));
        }
    }

    #[test]
    fn bilingual_locale_root_cannot_collide_with_a_primary_route() {
        let mut value = plan(vec![
            page(
                "architecture",
                "docs/architecture.md",
                PageKind::Architecture,
            ),
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
            page("index", "docs/index.md", PageKind::Index),
            page(
                "en-architecture",
                "docs/en/architecture.md",
                PageKind::Architecture,
            ),
            page(
                "en-file-atlas",
                "docs/en/file-atlas.md",
                PageKind::FileAtlas,
            ),
            page("en-index", "docs/en/index.md", PageKind::Index),
            page("primary-en-guide", "docs/en.md", PageKind::Overview),
            page("secondary-en-guide", "docs/en/en.md", PageKind::Overview),
        ]);
        value.language = "zh-CN".to_string();
        value.secondary_language = Some("en".to_string());
        for page in value.pages.iter_mut().take(4) {
            page.language = "zh-CN".to_string();
        }
        for page in value.pages.iter_mut().skip(3) {
            page.language = "en".to_string();
        }
        value.pages[6].language = "zh-CN".to_string();

        let errors = value.validate().unwrap_err();

        assert!(errors.iter().any(|error| {
            error.contains("docs/en.md")
                && error.contains("docs/en/index.md")
                && error.contains("Starlight route `/en/`")
        }));
    }

    #[test]
    fn plan_rejects_duplicate_paths_and_dependency_cycles() {
        let mut first = page("first", "docs/shared.md", PageKind::Index);
        first.depends_on = vec!["second".to_string()];
        let mut second = page("second", "docs/shared.md", PageKind::Architecture);
        second.depends_on = vec!["first".to_string()];
        let errors = plan(vec![
            first,
            second,
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
        ])
        .validate()
        .unwrap_err();

        assert!(
            errors
                .iter()
                .any(|error| error.contains("duplicate page path"))
        );
        assert!(errors.iter().any(|error| error.contains("cycle")));
    }

    #[test]
    fn plan_requires_every_included_inventory_file_to_be_assigned() {
        let inventory = Inventory {
            schema_version: GENERATION_SCHEMA_VERSION,
            project_name: "example".to_string(),
            source_revision: None,
            source_fingerprint: fingerprint("source-1"),
            languages: BTreeSet::from(["Rust".to_string()]),
            files: vec![
                FileEntry {
                    path: PathBuf::from("src/lib.rs"),
                    kind: FileKind::Source,
                    language: Some("Rust".to_string()),
                    size_bytes: 10,
                    content_hash: Some(fingerprint("hash-1")),
                    disposition: FileDisposition::Include,
                },
                FileEntry {
                    path: PathBuf::from("src/main.rs"),
                    kind: FileKind::Source,
                    language: Some("Rust".to_string()),
                    size_bytes: 10,
                    content_hash: Some(fingerprint("hash-2")),
                    disposition: FileDisposition::Include,
                },
            ],
        };
        let plan = plan(vec![
            page("index", "docs/index.md", PageKind::Index),
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
            page(
                "architecture",
                "docs/architecture.md",
                PageKind::Architecture,
            ),
        ]);
        let errors = plan.validate_against_inventory(&inventory).unwrap_err();

        assert!(errors.iter().any(|error| error.contains("src/main.rs")));
    }

    #[test]
    fn file_atlas_must_cover_the_complete_inventory() {
        let inventory = Inventory {
            schema_version: GENERATION_SCHEMA_VERSION,
            project_name: "example".to_string(),
            source_revision: None,
            source_fingerprint: fingerprint("source-1"),
            languages: BTreeSet::from(["Rust".to_string()]),
            files: ["src/lib.rs", "src/main.rs"]
                .into_iter()
                .map(|path| FileEntry {
                    path: PathBuf::from(path),
                    kind: FileKind::Source,
                    language: Some("Rust".to_string()),
                    size_bytes: 10,
                    content_hash: Some(fingerprint(&format!("hash-{path}"))),
                    disposition: FileDisposition::Include,
                })
                .collect(),
        };
        let mut architecture = page(
            "architecture",
            "docs/architecture.md",
            PageKind::Architecture,
        );
        architecture.source_files.push(PathBuf::from("src/main.rs"));
        architecture.covers.push("src/main.rs".to_string());
        let value = plan(vec![
            page("index", "docs/index.md", PageKind::Index),
            page("file-atlas", "docs/file-atlas.md", PageKind::FileAtlas),
            architecture,
        ]);

        assert!(
            value
                .validate_against_inventory(&inventory)
                .unwrap_err()
                .iter()
                .any(|error| error.contains("file_atlas") && error.contains("src/main.rs"))
        );
    }

    #[test]
    fn algorithm_page_requires_named_algorithms_and_all_structured_facets() {
        let mut algorithm = page("attention", "docs/attention.md", PageKind::Algorithm);

        let errors = algorithm.validate(Path::new("docs")).unwrap_err();
        assert!(errors.iter().any(|error| error.contains("concrete topics")));
        assert!(
            errors
                .iter()
                .any(|error| error.contains("implemented algorithm"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("principle_or_equations"))
        );

        algorithm.topics = vec!["causal attention".to_string()];
        algorithm.algorithms = vec!["online softmax".to_string()];
        algorithm.algorithm_sections = AlgorithmFacet::ALL
            .iter()
            .map(|facet| AlgorithmSection {
                facet: *facet,
                heading: facet.as_str().to_string(),
            })
            .collect();
        algorithm.algorithm_sections[0].heading = "Problem".to_string();
        algorithm.algorithm_sections[1].heading = "Problem!".to_string();
        algorithm.required_sections = algorithm
            .algorithm_sections
            .iter()
            .map(|section| section.heading.clone())
            .collect();
        assert!(
            algorithm
                .validate(Path::new("docs"))
                .unwrap_err()
                .iter()
                .any(|error| error.contains("multiple facets"))
        );

        algorithm.required_sections = AlgorithmFacet::ALL
            .iter()
            .map(|facet| facet.as_str().to_string())
            .collect();
        algorithm.algorithm_sections = AlgorithmFacet::ALL
            .iter()
            .map(|facet| AlgorithmSection {
                facet: *facet,
                heading: facet.as_str().to_string(),
            })
            .collect();

        assert_eq!(algorithm.validate(Path::new("docs")), Ok(()));
    }

    #[test]
    fn completed_run_cannot_have_pending_or_failed_pages() {
        let state = RunState {
            schema_version: GENERATION_SCHEMA_VERSION,
            run_id: "run-1".to_string(),
            source_fingerprint: fingerprint("source-1"),
            config_fingerprint: Some(fingerprint("config-1")),
            plan_fingerprint: Some(fingerprint("plan-1")),
            phase: RunPhase::Complete,
            status: RunStatus::Completed,
            completed_pages: BTreeSet::from(["index".to_string()]),
            pending_pages: BTreeSet::from(["architecture".to_string()]),
            failed_pages: BTreeMap::new(),
            attempts: BTreeMap::new(),
            last_error: None,
        };

        assert!(state.validate(None).is_err());
    }

    #[test]
    fn discover_run_accepts_only_the_explicit_pending_sentinel() {
        let mut state = RunState {
            schema_version: GENERATION_SCHEMA_VERSION,
            run_id: "run-1".to_string(),
            source_fingerprint: "pending".to_string(),
            config_fingerprint: Some(fingerprint("config-1")),
            plan_fingerprint: None,
            phase: RunPhase::Discover,
            status: RunStatus::Pending,
            completed_pages: BTreeSet::new(),
            pending_pages: BTreeSet::new(),
            failed_pages: BTreeMap::new(),
            attempts: BTreeMap::new(),
            last_error: None,
        };
        assert_eq!(state.validate(None), Ok(()));

        state.source_fingerprint = "e71fa2190541574b".to_string();
        assert!(
            state
                .validate(None)
                .unwrap_err()
                .iter()
                .any(|error| error.contains("exact `pending` sentinel"))
        );
    }

    #[test]
    fn passed_verification_cannot_contain_error_issues() {
        let plan = plan(vec![page("index", "docs/index.md", PageKind::Index)]);
        let report = VerificationReport {
            schema_version: GENERATION_SCHEMA_VERSION,
            source_fingerprint: fingerprint("source-1"),
            status: VerificationStatus::Passed,
            summary: "looks good".to_string(),
            checks: Vec::new(),
            page_results: BTreeMap::new(),
            uncovered_files: Vec::new(),
            broken_links: Vec::new(),
            issues: vec![VerificationIssue {
                code: "placeholder".to_string(),
                severity: VerificationSeverity::Error,
                page_id: Some("index".to_string()),
                message: "placeholder text remains".to_string(),
                repairable: true,
            }],
        };

        assert!(report.validate_against(&plan).is_err());
    }
}
