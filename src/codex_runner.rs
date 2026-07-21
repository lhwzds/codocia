//! Process-backed integration with `codex exec`.
//!
//! This module deliberately implements only the read-only analysis path. A
//! documentation writer should run in an isolated staging workspace instead of
//! weakening this runner's sandbox setting.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const ISOLATED_CODEX_HOME_PREFIX: &str = "codocia-auth-";
const ISOLATED_CODEX_HOME_LEASE: &str = ".codocia-auth-lease";
const STALE_ISOLATED_CODEX_HOME_SECONDS: u64 = 24 * 60 * 60;
const DISABLED_CODEX_FEATURES: &[&str] = &[
    "plugins",
    "plugin_sharing",
    "hooks",
    "shell_snapshot",
    "apps",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "in_app_browser",
    "image_generation",
    "memories",
    "multi_agent",
    "skill_mcp_dependency_install",
    "tool_call_mcp_elicitation",
    "tool_suggest",
    "workspace_dependencies",
    "web_search_request",
    "web_search_cached",
    "standalone_web_search",
    "search_tool",
];

/// Configuration for one non-interactive, read-only Codex turn.
///
/// `output_last_message` must be a new path. Requiring a fresh artifact avoids
/// mistaking output from an earlier failed invocation for the current result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexExecConfig {
    /// Executable name or path. Defaults to `codex` and can point at a fake in
    /// tests or at a wrapper supplied by the caller.
    pub executable: PathBuf,
    /// Repository or source workspace Codex should inspect.
    pub workspace: PathBuf,
    /// JSON Schema passed to `codex exec --output-schema`.
    pub output_schema: PathBuf,
    /// Fresh file populated by `codex exec --output-last-message`.
    pub output_last_message: PathBuf,
    /// Optional model forwarded without interpretation.
    pub model: Option<String>,
    /// Allow analysis of source mirrors that are not Git repositories.
    pub skip_git_repo_check: bool,
    /// Additional environment variables, primarily useful for controlled
    /// wrappers and fake executables.
    pub environment: BTreeMap<OsString, OsString>,
    /// Original repository roots that prove the workspace is an isolated
    /// mirror and activate the mandatory outer OS allowlist. The process
    /// runner refuses an empty list.
    pub denied_read_roots: Vec<PathBuf>,
}

impl CodexExecConfig {
    pub fn new(
        workspace: impl Into<PathBuf>,
        output_schema: impl Into<PathBuf>,
        output_last_message: impl Into<PathBuf>,
    ) -> Self {
        Self {
            executable: PathBuf::from("codex"),
            workspace: workspace.into(),
            output_schema: output_schema.into(),
            output_last_message: output_last_message.into(),
            model: None,
            skip_git_repo_check: true,
            environment: BTreeMap::new(),
            denied_read_roots: Vec::new(),
        }
    }

    pub fn with_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.executable = executable.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    pub fn with_denied_read_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.denied_read_roots.push(root.into());
        self
    }
}

/// Coarse event classification kept stable even if Codex adds new JSONL event
/// fields. The original line is retained on [`CodexEvent`] for forward
/// compatibility and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexEventKind {
    ThreadStarted,
    AgentMessage,
    OutputTextDelta,
    Error,
    TurnCompleted,
    Other,
    Malformed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexUsage {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexEvent {
    pub kind: CodexEventKind,
    pub event_type: Option<String>,
    pub item_type: Option<String>,
    pub thread_id: Option<String>,
    pub text: Option<String>,
    pub usage: Option<CodexUsage>,
    pub raw: String,
}

/// Result of a spawned Codex process.
///
/// A non-zero child exit is represented here rather than converted to an
/// `anyhow::Error`, so an orchestrator can persist events, the thread id, and
/// diagnostics before deciding whether to retry. Spawn, pipe, and filesystem
/// failures still return `Err` from [`run_codex_exec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexExecResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub thread_id: Option<String>,
    pub final_output: Option<String>,
    pub events: Vec<CodexEvent>,
    pub stdout: String,
    pub stderr: String,
}

impl CodexExecResult {
    pub fn error_messages(&self) -> Vec<&str> {
        self.events
            .iter()
            .filter(|event| event.kind == CodexEventKind::Error)
            .filter_map(|event| event.text.as_deref())
            .collect()
    }
}

/// Injection seam for the future generation orchestrator and deterministic
/// tests.
pub trait CodexExecutor {
    fn execute(&self, config: &CodexExecConfig, prompt: &str) -> Result<CodexExecResult>;
}

#[derive(Default)]
pub struct ProcessCodexExecutor {
    isolated_home: Mutex<Option<IsolatedCodexHome>>,
}

impl CodexExecutor for ProcessCodexExecutor {
    fn execute(&self, config: &CodexExecConfig, prompt: &str) -> Result<CodexExecResult> {
        run_codex_exec_impl_with_session(config, prompt, true, Some(&self.isolated_home))
    }
}

/// Run one read-only `codex exec` turn and collect its JSONL event stream and
/// structured final response artifact. Turns are always ephemeral and ignore
/// repository exec-policy rules so generation cannot inherit project-specific
/// command permissions or leave resumable Codex session state behind.
pub fn run_codex_exec(config: &CodexExecConfig, prompt: &str) -> Result<CodexExecResult> {
    run_codex_exec_impl(config, prompt, true)
}

fn run_codex_exec_impl(
    config: &CodexExecConfig,
    prompt: &str,
    require_os_boundary: bool,
) -> Result<CodexExecResult> {
    run_codex_exec_impl_with_session(config, prompt, require_os_boundary, None)
}

fn run_codex_exec_impl_with_session(
    config: &CodexExecConfig,
    prompt: &str,
    require_os_boundary: bool,
    shared_home: Option<&Mutex<Option<IsolatedCodexHome>>>,
) -> Result<CodexExecResult> {
    validate_config(config)?;
    if require_os_boundary && config.denied_read_roots.is_empty() {
        bail!(
            "Codex execution requires an explicit original source root so the outer OS allowlist can fail closed"
        );
    }

    let workspace = absolute_existing_path(&config.workspace)?;
    let output_schema = absolute_existing_path(&config.output_schema)?;
    let output_last_message = absolute_new_path(&config.output_last_message)?;

    let mut shared_home_guard = shared_home
        .map(|home| {
            home.lock()
                .map_err(|_| anyhow::anyhow!("isolated Codex session lock is poisoned"))
        })
        .transpose()?;
    let mut owned_home = None;
    if !config.denied_read_roots.is_empty() {
        if let Some(home) = shared_home_guard.as_mut() {
            if home.is_none() {
                **home = Some(IsolatedCodexHome::create(config)?);
            } else {
                home.as_ref()
                    .expect("shared isolated home was already initialized")
                    .prepare_for_turn(config)?;
            }
        } else {
            owned_home = Some(IsolatedCodexHome::create(config)?);
        }
    }
    let isolated_codex_home = shared_home_guard
        .as_ref()
        .and_then(|home| home.as_ref())
        .or(owned_home.as_ref());

    let mut command = sandboxed_command(
        config,
        &workspace,
        &output_last_message,
        isolated_codex_home,
    )?;
    command
        .arg("exec")
        .arg("--json")
        .arg("--ephemeral")
        .arg("--ignore-user-config")
        .arg("--ignore-rules")
        .arg("--strict-config")
        .arg("--color")
        .arg("never")
        .arg("-c")
        .arg("shell_environment_policy.inherit=none")
        .arg("-c")
        .arg("shell_environment_policy.set.PATH=\"/usr/bin:/bin:/usr/sbin:/sbin\"")
        .arg("-c")
        .arg("web_search=never")
        .arg("--sandbox")
        .arg("read-only")
        .arg("--cd")
        .arg(&workspace)
        .arg("--output-schema")
        .arg(&output_schema)
        .arg("--output-last-message")
        .arg(&output_last_message);
    for feature in DISABLED_CODEX_FEATURES {
        command.arg("--disable").arg(feature);
    }

    if config.skip_git_repo_check {
        command.arg("--skip-git-repo-check");
    }
    if let Some(model) = config
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        command.arg("--model").arg(model);
    }

    command
        .arg("-")
        .current_dir(&workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(home) = isolated_codex_home {
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8")
            .env(
                "__CF_USER_TEXT_ENCODING",
                std::env::var_os("__CF_USER_TEXT_ENCODING")
                    .unwrap_or_else(|| OsString::from("0x0:0x0:0x0")),
            )
            .env("HOME", &home.root)
            .env("TMPDIR", home.root.join("tmp"))
            .env("CODEX_HOME", &home.root);
    } else {
        command.envs(&config.environment);
    }

    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to run Codex executable {}",
            config.executable.display()
        )
    })?;
    if let Some(home) = isolated_codex_home {
        #[cfg(target_os = "macos")]
        let prepare_result = wait_for_private_codex_exec(&mut child, &home.executable)
            .and_then(|()| home.unlink_executable());
        #[cfg(not(target_os = "macos"))]
        let prepare_result: Result<()> =
            bail!("strict Codex execution is unavailable without a private-executable handshake");
        if let Err(error) = prepare_result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context(
                "failed to remove the private Codex executable before exposing the model prompt",
            );
        }
    }
    let mut stdin = child.stdin.take().context("failed to open Codex stdin")?;
    stdin
        .write_all(prompt.as_bytes())
        .context("failed to write prompt to Codex stdin")?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .context("failed waiting for codex exec")?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let events = parse_codex_jsonl(&stdout);
    let thread_id = events.iter().find_map(|event| event.thread_id.clone());
    let final_output = if output_last_message.is_file() {
        Some(fs::read_to_string(&output_last_message).with_context(|| {
            format!(
                "failed to read Codex final output {}",
                output_last_message.display()
            )
        })?)
    } else {
        None
    };

    Ok(CodexExecResult {
        success: output.status.success(),
        exit_code: output.status.code(),
        thread_id,
        final_output,
        events,
        stdout,
        stderr,
    })
}

/// Parse all non-empty JSONL lines without rejecting unknown future event
/// variants. Malformed lines become explicit events instead of disappearing.
pub fn parse_codex_jsonl(output: &str) -> Vec<CodexEvent> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(parse_codex_event)
        .collect()
}

fn validate_config(config: &CodexExecConfig) -> Result<()> {
    if config.executable.as_os_str().is_empty() {
        bail!("Codex executable must not be empty");
    }
    if !config.workspace.is_dir() {
        bail!(
            "Codex workspace is not a directory: {}",
            config.workspace.display()
        );
    }
    if !config.output_schema.is_file() {
        bail!(
            "Codex output schema is not a file: {}",
            config.output_schema.display()
        );
    }
    if config.output_last_message.exists() {
        bail!(
            "Codex final output path already exists: {}",
            config.output_last_message.display()
        );
    }
    let parent = config
        .output_last_message
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        bail!(
            "Codex final output parent is not a directory: {}",
            parent.display()
        );
    }
    let workspace = fs::canonicalize(&config.workspace)?;
    for root in &config.denied_read_roots {
        let root = fs::canonicalize(root)
            .with_context(|| format!("failed to resolve denied read root {}", root.display()))?;
        if workspace.starts_with(&root) {
            bail!(
                "Codex allowlisted workspace `{}` is inside denied source root `{}`",
                workspace.display(),
                root.display()
            );
        }
    }
    Ok(())
}

fn sandboxed_command(
    config: &CodexExecConfig,
    workspace: &Path,
    output_last_message: &Path,
    isolated_codex_home: Option<&IsolatedCodexHome>,
) -> Result<Command> {
    if config.denied_read_roots.is_empty() {
        return Ok(Command::new(&config.executable));
    }

    #[cfg(target_os = "macos")]
    {
        let sandbox_exec = Path::new("/usr/bin/sandbox-exec");
        if !sandbox_exec.is_file() {
            bail!("cannot enforce the Codex read boundary: /usr/bin/sandbox-exec is unavailable");
        }
        let isolated_codex_home = isolated_codex_home
            .context("strict Codex execution requires an isolated CODEX_HOME")?;
        let profile = macos_read_boundary_profile(
            workspace,
            &isolated_codex_home.executable,
            output_last_message,
            &isolated_codex_home.root,
        )?;
        let mut command = Command::new(sandbox_exec);
        command
            .arg("-p")
            .arg(profile)
            .arg(&isolated_codex_home.executable);
        Ok(command)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = config;
        let _ = workspace;
        let _ = output_last_message;
        let _ = isolated_codex_home;
        bail!(
            "Codex generation refuses to run without an OS-enforced source read boundary; this build currently supports that boundary on macOS"
        )
    }
}

#[cfg(target_os = "macos")]
fn macos_read_boundary_profile(
    workspace: &Path,
    executable: &Path,
    output_last_message: &Path,
    isolated_codex_home: &Path,
) -> Result<String> {
    let workspace = fs::canonicalize(workspace)
        .with_context(|| format!("failed to resolve Codex mirror {}", workspace.display()))?;
    let executable = fs::canonicalize(executable).with_context(|| {
        format!(
            "failed to resolve Codex executable {}",
            executable.display()
        )
    })?;
    let output_parent = output_last_message
        .parent()
        .context("Codex final output path must have a parent")?;
    let output_parent = fs::canonicalize(output_parent).with_context(|| {
        format!(
            "failed to resolve Codex final output parent {}",
            output_parent.display()
        )
    })?;
    let output_name = output_last_message
        .file_name()
        .context("Codex final output path must have a file name")?;
    let output_last_message = output_parent.join(output_name);
    let isolated_codex_home = fs::canonicalize(isolated_codex_home).with_context(|| {
        format!(
            "failed to resolve isolated CODEX_HOME {}",
            isolated_codex_home.display()
        )
    })?;
    let workspace = seatbelt_literal(&workspace);
    let executable = seatbelt_literal(&executable);
    let output_last_message = seatbelt_literal(&output_last_message);
    let isolated_codex_home = seatbelt_literal(&isolated_codex_home);
    let profile = format!(
        r#"(version 1)
	(deny default)
	(import "system.sb")
	(import "com.apple.corefoundation.sb")
	; `system.sb` is required for the macOS runtime, but its compatibility
	; grants are too broad for model-launched children. Neutralize them here;
	; the trusted, private Codex executable receives only the named IPC and
	; network capabilities inside its process-path filter below.
	(deny mach-register)
	(deny mach-lookup)
	(deny ipc-posix-shm*)
	(deny user-preference-read)
	(deny network*)
	(deny system-socket)
	(deny signal)
	; Read-only inspection needs the real chain Codex -> shell -> rg/cat. System
	; children may fork and execute only immutable system tools; they still
	; cannot read auth, use host IPC, connect remotely, or execute workspace code.
	(allow process-fork)
	(allow process-exec
	  (literal "{executable}")
	  (subpath "/bin")
	  (subpath "/usr/bin"))
	(allow process-info* (target self))
	(allow signal (target self))
	(allow file-read-metadata file-test-existence
	  (path-ancestors "{workspace}")
	  (path-ancestors "{executable}"))
	(allow file-read* file-map-executable (subpath "/bin"))
(allow file-read* file-map-executable (subpath "/usr/bin"))
(allow file-read* file-map-executable (subpath "/usr/lib"))
(allow file-read* file-map-executable (subpath "/System"))
(allow file-read* file-map-executable (subpath "/Library/Apple"))
(allow file-read* (subpath "/private/etc/ssl"))
(allow file-read* (literal "/private/etc/resolv.conf"))
	(allow file-read* file-map-executable (literal "{executable}"))
	(allow file-read* (subpath "{workspace}"))
	(with-filter (process-path "{executable}")
	  (corefoundation)
	  (system-network)
	  (allow file-read-metadata file-test-existence
	    (path-ancestors "{isolated_codex_home}")
	    (path-ancestors "{output_last_message}"))
	  (allow file-read* file-write* (subpath "{isolated_codex_home}"))
	  (allow file-write* (literal "{output_last_message}"))
	  (allow file-read* (literal "/Library/Preferences/com.apple.security.plist"))
	  (allow user-preference-read
	    (preference-domain "com.openai.codex")
	    (preference-domain "kCFPreferencesAnyApplication"))
	  (allow mach-lookup
	    (global-name "com.apple.SystemConfiguration.configd")
	    (global-name "com.apple.SecurityServer"))
	  (allow ipc-posix-shm-read-data
	         ipc-posix-shm-write-create
	         ipc-posix-shm-write-data
	    (ipc-posix-name "com.apple.AppleDatabaseChanged"))
	  (allow network-outbound
	    (literal "/private/var/run/mDNSResponder")
	    (remote tcp "*:443")))
"#
    );
    Ok(profile)
}

#[cfg(target_os = "macos")]
fn seatbelt_literal(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn resolve_executable(executable: &Path) -> Result<PathBuf> {
    if executable.components().count() > 1 || executable.is_absolute() {
        return fs::canonicalize(executable)
            .with_context(|| format!("failed to resolve executable {}", executable.display()));
    }
    let path = std::env::var_os("PATH").context("PATH is not set; cannot resolve Codex")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(executable);
        if candidate.is_file() {
            return fs::canonicalize(&candidate)
                .with_context(|| format!("failed to resolve executable {}", candidate.display()));
        }
    }
    bail!(
        "Codex executable was not found on PATH: {}",
        executable.display()
    )
}

fn codex_home(config: &CodexExecConfig) -> Option<PathBuf> {
    config
        .environment
        .get(OsStr::new("CODEX_HOME"))
        .cloned()
        .or_else(|| std::env::var_os("CODEX_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            config
                .environment
                .get(OsStr::new("HOME"))
                .cloned()
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
                .map(|home| home.join(".codex"))
        })
}

fn auth_path(config: &CodexExecConfig) -> Result<Option<PathBuf>> {
    let Some(home) = codex_home(config) else {
        return Ok(None);
    };
    let candidate = home.join("auth.json");
    if !candidate.is_file() {
        return Ok(None);
    }
    Ok(Some(fs::canonicalize(&candidate).with_context(|| {
        format!("failed to resolve Codex auth file {}", candidate.display())
    })?))
}

struct IsolatedCodexHome {
    root: PathBuf,
    executable: PathBuf,
    source_executable: PathBuf,
    source_auth: PathBuf,
    source_auth_home: PathBuf,
}

impl IsolatedCodexHome {
    fn create(config: &CodexExecConfig) -> Result<Self> {
        cleanup_stale_isolated_codex_homes()?;
        let source_executable = resolve_executable(&config.executable)?;
        let source_auth = auth_path(config)?.context(
            "strict Codex generation requires an existing CODEX_HOME/auth.json; run `codex login` before generation",
        )?;
        let source_auth_home = canonical_codex_home(config)?;
        if source_auth.parent() != Some(source_auth_home.as_path()) {
            bail!(
                "strict Codex generation refuses an auth.json symlink outside CODEX_HOME: {}",
                source_auth.display()
            );
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "{ISOLATED_CODEX_HOME_PREFIX}{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root)?;
        let home = Self {
            executable: root.join("codex-private"),
            root,
            source_executable,
            source_auth,
            source_auth_home,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&home.root, fs::Permissions::from_mode(0o700))?;
        }
        fs::create_dir(home.root.join("tmp"))?;
        home.refresh_lease()?;
        home.copy_private_executable()?;
        let destination = home.root.join("auth.json");
        fs::copy(&home.source_auth, &destination).with_context(|| {
            format!(
                "failed to copy Codex authentication into isolated CODEX_HOME from {}",
                home.source_auth.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))?;
        }
        Ok(home)
    }

    fn prepare_for_turn(&self, config: &CodexExecConfig) -> Result<()> {
        let source_executable = resolve_executable(&config.executable)?;
        let source_auth_home = canonical_codex_home(config)?;
        if source_executable != self.source_executable || source_auth_home != self.source_auth_home
        {
            bail!(
                "a ProcessCodexExecutor cannot change executable or authentication source during one generation run"
            );
        }
        if self.executable.exists() {
            bail!(
                "private Codex executable unexpectedly still exists before a new turn: {}",
                self.executable.display()
            );
        }
        self.copy_private_executable()?;
        self.refresh_lease()?;
        Ok(())
    }

    fn copy_private_executable(&self) -> Result<()> {
        fs::copy(&self.source_executable, &self.executable).with_context(|| {
            format!(
                "failed to create a private Codex executable from {}",
                self.source_executable.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.executable, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    fn refresh_lease(&self) -> Result<()> {
        fs::write(
            self.root.join(ISOLATED_CODEX_HOME_LEASE),
            format!(
                "{} {}\n",
                std::process::id(),
                unix_seconds(SystemTime::now())?
            ),
        )?;
        Ok(())
    }

    fn unlink_executable(&self) -> Result<()> {
        fs::remove_file(&self.executable).with_context(|| {
            format!(
                "failed to unlink private Codex executable {}",
                self.executable.display()
            )
        })
    }
}

fn canonical_codex_home(config: &CodexExecConfig) -> Result<PathBuf> {
    let home = codex_home(config)
        .context("strict Codex generation requires CODEX_HOME or HOME to locate authentication")?;
    fs::canonicalize(&home)
        .with_context(|| format!("failed to resolve Codex home {}", home.display()))
}

impl Drop for IsolatedCodexHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn unix_seconds(time: SystemTime) -> Result<u64> {
    Ok(time
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

fn cleanup_stale_isolated_codex_homes() -> Result<()> {
    cleanup_stale_isolated_codex_homes_at(unix_seconds(SystemTime::now())?)
}

fn cleanup_stale_isolated_codex_homes_at(now: u64) -> Result<()> {
    let entries = fs::read_dir(std::env::temp_dir())?;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(ISOLATED_CODEX_HOME_PREFIX) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let Ok(lease) = fs::read_to_string(path.join(ISOLATED_CODEX_HOME_LEASE)) else {
            continue;
        };
        let Some((pid, refreshed_at)) = parse_isolated_home_lease(&lease) else {
            continue;
        };
        if now.saturating_sub(refreshed_at) >= STALE_ISOLATED_CODEX_HOME_SECONDS
            && !pid.is_some_and(process_is_alive)
        {
            match fs::remove_dir_all(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "failed to remove stale isolated Codex home {}",
                            path.display()
                        )
                    });
                }
            }
        }
    }
    Ok(())
}

fn parse_isolated_home_lease(value: &str) -> Option<(Option<u32>, u64)> {
    let fields = value.split_whitespace().collect::<Vec<_>>();
    match fields.as_slice() {
        [timestamp] => Some((None, timestamp.parse().ok()?)),
        [pid, timestamp] => Some((Some(pid.parse().ok()?), timestamp.parse().ok()?)),
        _ => None,
    }
}

#[cfg(unix)]
pub(crate) fn process_is_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: signal 0 does not deliver a signal; it only performs the kernel
    // existence/permission check for the numeric PID.
    unsafe { kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
pub(crate) fn process_is_alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: std::ffi::c_int, signal: std::ffi::c_int) -> std::ffi::c_int;
}

#[cfg(target_os = "macos")]
fn wait_for_private_codex_exec(
    child: &mut std::process::Child,
    expected_executable: &Path,
) -> Result<()> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    const PROC_PIDPATH_BUFFER_SIZE: usize = 4096;
    let expected_executable = fs::canonicalize(expected_executable).with_context(|| {
        format!(
            "failed to canonicalize private Codex executable {} before handshake",
            expected_executable.display()
        )
    })?;
    let mut buffer = [0_u8; PROC_PIDPATH_BUFFER_SIZE];
    for _ in 0..300 {
        if let Some(status) = child.try_wait()? {
            bail!("sandbox-exec exited before launching private Codex: {status}");
        }
        buffer.fill(0);
        // SAFETY: `buffer` is valid for `buffer.len()` writable bytes for the
        // duration of the call, and the PID comes from the live child handle.
        let length = unsafe {
            proc_pidpath(
                child.id() as std::ffi::c_int,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
            )
        };
        if length > 0 {
            let length = length as usize;
            let end = buffer[..length]
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(length);
            let observed = Path::new(OsStr::from_bytes(&buffer[..end]));
            if observed == expected_executable {
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    bail!(
        "timed out waiting for sandbox-exec to launch {}",
        expected_executable.display()
    )
}

#[cfg(target_os = "macos")]
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidpath(pid: std::ffi::c_int, buffer: *mut std::ffi::c_void, size: u32) -> i32;
}

fn absolute_existing_path(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("failed to resolve {}", path.display()))
}

fn absolute_new_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent)
        .with_context(|| format!("failed to resolve {}", parent.display()))?;
    let file_name = path
        .file_name()
        .context("Codex final output path must have a file name")?;
    Ok(parent.join(file_name))
}

fn parse_codex_event(line: &str) -> CodexEvent {
    if !is_complete_json_object(line) {
        return CodexEvent {
            kind: CodexEventKind::Malformed,
            event_type: None,
            item_type: None,
            thread_id: None,
            text: None,
            usage: None,
            raw: line.to_string(),
        };
    }

    let event_type = json_top_level_string(line, "type");
    let thread_id = json_top_level_string(line, "thread_id");
    let mut item_type = None;
    let mut text = None;
    let mut usage = None;
    let kind = match event_type.as_deref() {
        Some("thread.started") => CodexEventKind::ThreadStarted,
        Some("item.completed") => {
            if let Some(item) = json_top_level_object(line, "item") {
                item_type = json_top_level_string(item, "type");
                if matches!(
                    item_type.as_deref(),
                    Some("agent_message" | "assistant_message")
                ) {
                    text = json_top_level_string(item, "text")
                        .or_else(|| json_top_level_string(item, "content"));
                    CodexEventKind::AgentMessage
                } else {
                    CodexEventKind::Other
                }
            } else {
                CodexEventKind::Other
            }
        }
        Some("agent_message" | "assistant_message") => {
            text = json_top_level_string(line, "text")
                .or_else(|| json_top_level_string(line, "content"));
            CodexEventKind::AgentMessage
        }
        Some("response.output_text.delta") => {
            text = json_top_level_string(line, "delta");
            CodexEventKind::OutputTextDelta
        }
        Some("error") => {
            text = json_top_level_string(line, "message");
            CodexEventKind::Error
        }
        Some("turn.completed") => {
            usage = json_top_level_object(line, "usage").map(|value| CodexUsage {
                input_tokens: json_top_level_u64(value, "input_tokens"),
                cached_input_tokens: json_top_level_u64(value, "cached_input_tokens"),
                output_tokens: json_top_level_u64(value, "output_tokens"),
            });
            CodexEventKind::TurnCompleted
        }
        _ => CodexEventKind::Other,
    };

    CodexEvent {
        kind,
        event_type,
        item_type,
        thread_id,
        text,
        usage,
        raw: line.to_string(),
    }
}

fn json_top_level_string(input: &str, key: &str) -> Option<String> {
    let start = json_top_level_value_start(input, key)?;
    parse_json_string(input, start).map(|(value, _)| value)
}

fn json_top_level_u64(input: &str, key: &str) -> Option<u64> {
    let start = json_top_level_value_start(input, key)?;
    let bytes = input.as_bytes();
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    (end > start)
        .then(|| input[start..end].parse().ok())
        .flatten()
}

fn json_top_level_object<'a>(input: &'a str, key: &str) -> Option<&'a str> {
    let start = json_top_level_value_start(input, key)?;
    if input.as_bytes().get(start) != Some(&b'{') {
        return None;
    }
    let end = matching_object_end(input, start)?;
    Some(&input[start..end])
}

fn json_top_level_value_start(input: &str, key: &str) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut depth = 0_usize;
    let mut index = 0_usize;

    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'[' => {
                depth += 1;
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                index += 1;
            }
            b'"' => {
                let (candidate, end) = parse_json_string(input, index)?;
                if depth == 1 {
                    let colon = skip_ascii_whitespace(bytes, end);
                    if bytes.get(colon) == Some(&b':') && candidate == key {
                        return Some(skip_ascii_whitespace(bytes, colon + 1));
                    }
                }
                index = end;
            }
            _ => index += 1,
        }
    }
    None
}

fn parse_json_string(input: &str, start: usize) -> Option<(String, usize)> {
    let bytes = input.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }

    let mut output = String::new();
    let mut segment_start = start + 1;
    let mut index = segment_start;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                output.push_str(std::str::from_utf8(&bytes[segment_start..index]).ok()?);
                return Some((output, index + 1));
            }
            b'\\' => {
                output.push_str(std::str::from_utf8(&bytes[segment_start..index]).ok()?);
                index += 1;
                let escaped = *bytes.get(index)?;
                match escaped {
                    b'"' => output.push('"'),
                    b'\\' => output.push('\\'),
                    b'/' => output.push('/'),
                    b'b' => output.push('\u{0008}'),
                    b'f' => output.push('\u{000c}'),
                    b'n' => output.push('\n'),
                    b'r' => output.push('\r'),
                    b't' => output.push('\t'),
                    b'u' => {
                        let (character, end) = parse_unicode_escape(bytes, index + 1)?;
                        output.push(character);
                        index = end - 1;
                    }
                    _ => return None,
                }
                index += 1;
                segment_start = index;
            }
            byte if byte < 0x20 => return None,
            _ => index += 1,
        }
    }
    None
}

fn parse_unicode_escape(bytes: &[u8], digits_start: usize) -> Option<(char, usize)> {
    let first = parse_hex_quad(bytes, digits_start)?;
    let mut scalar = u32::from(first);
    let mut end = digits_start + 4;

    if (0xd800..=0xdbff).contains(&first) {
        if bytes.get(end..end + 2)? != b"\\u" {
            return None;
        }
        let second = parse_hex_quad(bytes, end + 2)?;
        if !(0xdc00..=0xdfff).contains(&second) {
            return None;
        }
        scalar = 0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00);
        end += 6;
    } else if (0xdc00..=0xdfff).contains(&first) {
        return None;
    }

    char::from_u32(scalar).map(|character| (character, end))
}

fn parse_hex_quad(bytes: &[u8], start: usize) -> Option<u16> {
    let mut value = 0_u16;
    for byte in bytes.get(start..start + 4)? {
        value = value.checked_mul(16)?;
        value = value.checked_add(u16::from(hex_value(*byte)?))?;
    }
    Some(value)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

fn matching_object_end(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let mut depth = 0_usize;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => index = parse_json_string(input, index)?.1,
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth = depth.checked_sub(1)?;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => index += 1,
        }
    }
    None
}

fn is_complete_json_object(input: &str) -> bool {
    let trimmed = input.trim();
    let Some(end) = matching_object_end(trimmed, 0) else {
        return false;
    };
    trimmed[end..].trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let suffix = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "codocia-codex-runner-{label}-{}-{suffix}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parses_known_unknown_and_malformed_events() {
        let output = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"thread_1\"}\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Line\n\"quoted\" \u4e2d\ud83d\ude80"}}"#,
            "\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":10,\"cached_input_tokens\":2,\"output_tokens\":3}}\n",
            "{\"type\":\"future.event\",\"value\":true}\n",
            "not json\n",
        );

        let events = parse_codex_jsonl(output);

        assert_eq!(events.len(), 5);
        assert_eq!(events[0].kind, CodexEventKind::ThreadStarted);
        assert_eq!(events[0].thread_id.as_deref(), Some("thread_1"));
        assert_eq!(events[1].kind, CodexEventKind::AgentMessage);
        assert_eq!(events[1].text.as_deref(), Some("Line\n\"quoted\" 中🚀"));
        assert_eq!(events[2].kind, CodexEventKind::TurnCompleted);
        assert_eq!(
            events[2].usage,
            Some(CodexUsage {
                input_tokens: Some(10),
                cached_input_tokens: Some(2),
                output_tokens: Some(3),
            })
        );
        assert_eq!(events[3].kind, CodexEventKind::Other);
        assert_eq!(events[4].kind, CodexEventKind::Malformed);
    }

    #[test]
    fn rejects_an_existing_final_output_artifact() {
        let root = TestDir::new("existing-output");
        let schema = root.path().join("schema.json");
        let final_output = root.path().join("final.json");
        fs::write(&schema, "{}").unwrap();
        fs::write(&final_output, "stale").unwrap();
        let config = CodexExecConfig::new(root.path(), schema, final_output);

        let error = run_codex_exec(&config, "prompt").unwrap_err().to_string();

        assert!(error.contains("already exists"));
    }

    #[cfg(unix)]
    #[test]
    fn invokes_an_injected_fake_with_read_only_structured_output_flags() {
        use std::os::unix::fs::PermissionsExt;

        let root = TestDir::new("fake");
        let fake = root.path().join("fake-codex");
        let schema = root.path().join("schema.json");
        let final_output = root.path().join("final.json");
        let args_file = root.path().join("args.txt");
        let stdin_file = root.path().join("stdin.txt");
        fs::write(&schema, "{\"type\":\"object\"}").unwrap();
        fs::write(
            &fake,
            r#"#!/bin/sh
printf '%s\n' "$@" > "$FAKE_ARGS_FILE"
cat > "$FAKE_STDIN_FILE"
last_message=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = '--output-last-message' ]; then
    shift
    last_message="$1"
  fi
  shift
done
printf '%s' '{"pages":["docs/index.md"]}' > "$last_message"
printf '%s\n' '{"type":"thread.started","thread_id":"thread_fake"}'
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"done"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":3}}'
printf '%s\n' 'fake warning' >&2
exit "${FAKE_EXIT_CODE:-0}"
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake, permissions).unwrap();

        let config = CodexExecConfig::new(root.path(), &schema, &final_output)
            .with_executable(&fake)
            .with_model("test-model")
            .with_env("FAKE_ARGS_FILE", args_file.as_os_str())
            .with_env("FAKE_STDIN_FILE", stdin_file.as_os_str());

        let result = run_codex_exec_impl(&config, "inspect the repository\n", false).unwrap();

        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.thread_id.as_deref(), Some("thread_fake"));
        assert_eq!(
            result.final_output.as_deref(),
            Some("{\"pages\":[\"docs/index.md\"]}")
        );
        assert!(result.stderr.contains("fake warning"));
        assert_eq!(
            fs::read_to_string(&stdin_file).unwrap(),
            "inspect the repository\n"
        );

        let args = fs::read_to_string(&args_file).unwrap();
        let args = args.lines().collect::<Vec<_>>();
        assert_eq!(args.first().copied(), Some("exec"));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--sandbox", "read-only"])
        );
        assert!(args.windows(2).any(|pair| pair[0] == "--output-schema"));
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--output-last-message")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "test-model"])
        );
        assert!(args.contains(&"--json"));
        assert!(args.contains(&"--ephemeral"));
        assert!(args.contains(&"--ignore-user-config"));
        assert!(args.contains(&"--ignore-rules"));
        assert!(args.contains(&"--strict-config"));
        assert!(args.contains(&"--skip-git-repo-check"));
        assert!(
            args.windows(2)
                .any(|pair| { pair == ["-c", "shell_environment_policy.inherit=none",] })
        );
        assert!(args.windows(2).any(|pair| {
            pair == [
                "-c",
                "shell_environment_policy.set.PATH=\"/usr/bin:/bin:/usr/sbin:/sbin\"",
            ]
        }));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-c", "web_search=never"])
        );
        for feature in DISABLED_CODEX_FEATURES {
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "--disable" && pair[1] == *feature),
                "missing --disable {feature}"
            );
        }
        assert_eq!(args.last().copied(), Some("-"));
    }

    #[cfg(unix)]
    #[test]
    fn preserves_events_and_diagnostics_for_a_nonzero_child_exit() {
        use std::os::unix::fs::PermissionsExt;

        let root = TestDir::new("failure");
        let fake = root.path().join("fake-codex");
        let schema = root.path().join("schema.json");
        let final_output = root.path().join("final.json");
        fs::write(&schema, "{}").unwrap();
        fs::write(
            &fake,
            r#"#!/bin/sh
cat >/dev/null
printf '%s\n' '{"type":"thread.started","thread_id":"thread_failed"}'
printf '%s\n' '{"type":"error","message":"rate limited"}'
printf '%s\n' 'retry later' >&2
exit 7
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake, permissions).unwrap();

        let config = CodexExecConfig::new(root.path(), schema, final_output).with_executable(&fake);
        let result = run_codex_exec_impl(&config, "prompt", false).unwrap();

        assert!(!result.success);
        assert_eq!(result.exit_code, Some(7));
        assert_eq!(result.thread_id.as_deref(), Some("thread_failed"));
        assert_eq!(result.error_messages(), vec!["rate limited"]);
        assert!(result.stderr.contains("retry later"));
        assert!(result.final_output.is_none());
    }

    #[test]
    fn executable_is_injectable_on_the_config() {
        let config =
            CodexExecConfig::new(".", "schema.json", "final.json").with_executable("fake-codex");

        assert_eq!(config.executable, PathBuf::from("fake-codex"));
    }

    #[test]
    fn process_runner_rejects_a_missing_outer_boundary() {
        let root = TestDir::new("missing-boundary");
        let schema = root.path().join("schema.json");
        let final_output = root.path().join("final.json");
        fs::write(&schema, "{}").unwrap();
        let config = CodexExecConfig::new(root.path(), schema, final_output);

        let error = run_codex_exec(&config, "prompt").unwrap_err().to_string();

        assert!(error.contains("explicit original source root"));
    }

    #[cfg(unix)]
    #[test]
    fn isolated_codex_home_copies_auth_privately_and_cleans_up() {
        use std::os::unix::fs::PermissionsExt;

        let source_home = TestDir::new("source-codex-home");
        fs::write(source_home.path().join("auth.json"), "private-auth").unwrap();
        let config = CodexExecConfig::new(".", "schema.json", "final.json")
            .with_executable("/usr/bin/true")
            .with_env("CODEX_HOME", source_home.path().as_os_str());

        let isolated = IsolatedCodexHome::create(&config).unwrap();
        let root = isolated.root.clone();
        let copied_auth = root.join("auth.json");
        assert_eq!(fs::read_to_string(&copied_auth).unwrap(), "private-auth");
        assert!(isolated.executable.is_file());
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&copied_auth).unwrap().permissions().mode() & 0o777,
            0o600
        );

        isolated.unlink_executable().unwrap();
        assert!(!isolated.executable.exists());
        fs::write(&copied_auth, "rotated-auth").unwrap();
        isolated.prepare_for_turn(&config).unwrap();
        assert_eq!(fs::read_to_string(&copied_auth).unwrap(), "rotated-auth");
        assert!(isolated.executable.is_file());
        let lease = fs::read_to_string(root.join(ISOLATED_CODEX_HOME_LEASE)).unwrap();
        assert_eq!(
            parse_isolated_home_lease(&lease).unwrap().0,
            Some(std::process::id())
        );

        isolated.unlink_executable().unwrap();

        drop(isolated);
        assert!(!root.exists());
    }

    #[test]
    fn stale_isolated_codex_home_is_removed_only_with_a_valid_lease() {
        let stale = std::env::temp_dir().join(format!(
            "{ISOLATED_CODEX_HOME_PREFIX}stale-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join(ISOLATED_CODEX_HOME_LEASE), "0").unwrap();

        cleanup_stale_isolated_codex_homes_at(2 * STALE_ISOLATED_CODEX_HOME_SECONDS).unwrap();

        assert!(!stale.exists());
    }

    #[test]
    fn cleanup_preserves_a_stale_lease_owned_by_a_live_process() {
        let active = std::env::temp_dir().join(format!(
            "{ISOLATED_CODEX_HOME_PREFIX}active-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&active).unwrap();
        fs::write(
            active.join(ISOLATED_CODEX_HOME_LEASE),
            format!("{} 0\n", std::process::id()),
        )
        .unwrap();

        cleanup_stale_isolated_codex_homes_at(2 * STALE_ISOLATED_CODEX_HOME_SECONDS).unwrap();

        assert!(active.exists());
        fs::remove_dir_all(active).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn private_executable_handshake_canonicalizes_the_var_alias() {
        let root = TestDir::new("private-executable-alias");
        let executable = root.path().join("codex-private");
        fs::write(&executable, "test executable").unwrap();

        let canonical = fs::canonicalize(&executable).unwrap();

        if executable.starts_with("/var") {
            assert!(canonical.starts_with("/private/var"));
            assert_ne!(executable, canonical);
        }
        assert_eq!(
            fs::canonicalize(&executable).unwrap(),
            canonical,
            "proc_pidpath must be compared with the canonical executable path"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn read_boundary_profile_is_a_fail_closed_allowlist() {
        let mirror = TestDir::new("allowlisted-mirror");
        let codex_home = TestDir::new("isolated-codex-home");
        let output = mirror.path().join("final.json");
        let executable = Path::new("/usr/bin/true");
        let profile =
            macos_read_boundary_profile(mirror.path(), executable, &output, codex_home.path())
                .unwrap();

        assert!(profile.contains("(deny default)"));
        assert!(!profile.contains("(allow default)"));
        assert!(profile.contains("(deny mach-lookup)"));
        assert!(profile.contains("(deny ipc-posix-shm*)"));
        assert!(profile.contains("(deny network*)"));
        assert!(!profile.contains("(allow process*)"));
        assert!(profile.contains("(allow process-fork)"));
        assert!(profile.contains("(allow process-exec"));
        assert!(profile.contains("(allow file-read* (subpath"));
        assert!(!profile.contains("(allow network*)"));
        assert!(!profile.contains("/opt/homebrew"));
        assert!(profile.contains("(import \"com.apple.corefoundation.sb\")"));
        let filtered = profile
            .split("(with-filter (process-path")
            .nth(1)
            .expect("private executable filter");
        assert!(filtered.contains("(corefoundation)"));
        assert!(filtered.contains("(system-network)"));
        assert!(profile.contains("(remote tcp \"*:443\")"));
        assert!(profile.contains("com.apple.SecurityServer"));
        assert!(profile.contains("com.apple.AppleDatabaseChanged"));
        assert!(profile.contains("with-filter (process-path \"/usr/bin/true\")"));
        assert!(profile.contains(mirror.path().to_string_lossy().as_ref()));
        assert!(profile.contains(codex_home.path().to_string_lossy().as_ref()));
        assert!(profile.contains(output.to_string_lossy().as_ref()));
        assert!(profile.contains("/usr/bin/true"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires running outside an existing macOS sandbox"]
    fn read_boundary_profile_launches_the_exact_allowlisted_executable() {
        let mirror = TestDir::new("seatbelt-probe-mirror");
        let codex_home = TestDir::new("seatbelt-probe-home");
        let output = mirror.path().join("final.json");
        let executable = Path::new("/usr/bin/true");
        let profile =
            macos_read_boundary_profile(mirror.path(), executable, &output, codex_home.path())
                .unwrap();

        let status = Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(profile)
            .arg(executable)
            .status()
            .unwrap();

        assert!(status.success());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires running outside an existing macOS sandbox"]
    fn read_boundary_profile_allows_codex_shell_and_system_tool_chain() {
        let mirror = TestDir::new("seatbelt-tool-probe-mirror");
        let codex_home = TestDir::new("seatbelt-tool-probe-home");
        let output = mirror.path().join("final.json");
        let executable = Path::new("/usr/bin/env");
        let profile =
            macos_read_boundary_profile(mirror.path(), executable, &output, codex_home.path())
                .unwrap();

        let status = Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(profile)
            .arg(executable)
            .arg("/bin/bash")
            .arg("-c")
            .arg("/bin/echo first >/dev/null; /usr/bin/true")
            .current_dir(mirror.path())
            .status()
            .unwrap();

        assert!(status.success());
    }
}
