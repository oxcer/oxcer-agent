//! Capability-scoped Shell Service: executes only catalog-defined commands
//! by `command_id` with validated parameters. No free-form shell execution.
//!
//! ## End state (Sprint 3)
//!
//! Oxcer has a **safe Shell Service** that:
//! - Supports a **small, explicit set of shell-based tools** via `command_id`.
//! - Validates parameters and constrains cwd / env / timeouts.
//! - Enforces **command-level Security Policy** (hard-deny patterns).
//! - Logs executions in a structured way compatible with the FS logs.
//!
//! ## Non-goals for Sprint 3
//!
//! - **No free-form shells** — no `/bin/bash -c "..."` or arbitrary command strings.
//! - **No long-lived interactive sessions** — one-shot execution only.
//! - **No dynamic command catalog from user input** — catalog is static (code only) for now.
//! - **No human-in-the-loop approval** — that comes in a later Security sprint.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

use crate::env_filter;
use crate::fs::{
    AppFsContext, BaseDirKind, DenyReason, FsError, normalize_and_resolve, SecurityDecision,
    SecurityDecisionKind, WorkspaceRoot,
};

// -----------------------------------------------------------------------------
// Caller and context (aligned with fs.rs pattern)
// -----------------------------------------------------------------------------

/// Logical caller for shell operations; used for policy and logging.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellCaller {
    Ui,
    Agent,
    System,
}

/// Context for shell execution: known workspaces and default.
#[derive(Clone, Debug)]
pub struct ShellContext {
    pub workspace_roots: Vec<WorkspaceRoot>,
    pub default_workspace_id: String,
}

// -----------------------------------------------------------------------------
// Command catalog types
// -----------------------------------------------------------------------------

/// Type of a single command parameter for validation.
#[derive(Clone, Debug)]
pub enum CommandParamType {
    String,
    Enum(Vec<String>),
    PathRelativeToWorkspace,
    Bool,
    Integer {
        min: Option<i64>,
        max: Option<i64>,
    },
}

/// Schema for one parameter of a command.
#[derive(Clone, Debug)]
pub struct CommandParamSpec {
    pub name: String,
    pub required: bool,
    pub param_type: CommandParamType,
}

/// Specification of a single allowed command: fixed binary + arg template.
#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub id: String,
    pub binary: PathBuf,
    pub args_template: Vec<String>,
    pub params: Vec<CommandParamSpec>,
    pub description: String,
}

/// Catalog of allowed commands: the only source of truth for what can run.
/// Uses HashMap for O(1) lookup by command id. Plugin IDs override built-ins on merge.
pub struct CommandCatalog {
    commands: HashMap<String, CommandSpec>,
}

/// Fully bound command ready for execution: binary, argv, and cwd.
/// Produced by validate_and_bind_params; no further string interpolation.
#[derive(Clone, Debug)]
pub struct BoundCommand {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

// -----------------------------------------------------------------------------
// Execution result and errors
// -----------------------------------------------------------------------------

/// Successful shell run result (output trimmed/truncated to max size, duration in ms).
#[derive(Debug, Serialize)]
pub struct ShellResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellErrorKind {
    UnknownCommand,
    InvalidParams,
    Forbidden,
    Io,
    Timeout,
}

#[derive(Debug, Serialize)]
pub struct ShellError {
    pub kind: ShellErrorKind,
    pub message: String,
}

// -----------------------------------------------------------------------------
// Shell log entry (aligned with FS logging for trace viewer)
// -----------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ShellLogEntry<'a> {
    pub timestamp: String,
    pub caller: ShellCaller,
    pub command_id: &'a str,
    pub binary: String,
    pub args_redacted: Vec<String>,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub policy_decision: SecurityDecisionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ShellErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

fn now_iso8601() -> String {
    let now = SystemTime::now();
    match now.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(dur) => {
            let secs = dur.as_secs();
            let millis = dur.subsec_millis();
            format!("{}.{:03}Z", secs, millis)
        }
        Err(_) => "0.000Z".to_string(),
    }
}

/// Logs a shell event as JSON to stdout (same as FS logs). Sprint 3: single log
/// after completion. args_redacted is raw args for now; masking can be added later.
fn log_shell_event<'a>(
    caller: ShellCaller,
    command_id: &'a str,
    bound: Option<&BoundCommand>,
    decision: &SecurityDecision,
    result: Option<&ShellResult>,
    error: Option<&ShellError>,
) {
    let (binary, args_redacted, cwd) = match bound {
        Some(b) => (
            b.binary.display().to_string(),
            b.args.clone(),
            b.cwd.display().to_string(),
        ),
        None => (
            String::new(),
            Vec::new(),
            String::new(),
        ),
    };
    let (exit_code, duration_ms) = result
        .map(|r| (Some(r.exit_code), Some(r.duration_ms)))
        .unwrap_or((None, None));
    let entry = ShellLogEntry {
        timestamp: now_iso8601(),
        caller,
        command_id,
        binary,
        args_redacted,
        cwd,
        exit_code,
        duration_ms,
        policy_decision: decision.decision.clone(),
        error_code: error.map(|e| e.kind.clone()),
        error_message: error.map(|e| e.message.clone()),
    };
    if let Ok(json) = serde_json::to_string(&entry) {
        println!("{json}");
    }
}

// -----------------------------------------------------------------------------
// Default catalog (hard-coded for Sprint 3; YAML/JSON later)
// -----------------------------------------------------------------------------

/// Returns the default command catalog with a small set of safe commands.
pub fn default_catalog() -> CommandCatalog {
    let mut commands = HashMap::new();

    // list_git_status: git -C <workspace> status --short
    commands.insert(
        "list_git_status".to_string(),
        CommandSpec {
            id: "list_git_status".to_string(),
            binary: PathBuf::from("git"),
            args_template: vec![
                "-C".to_string(),
                "{{workspace}}".to_string(),
                "status".to_string(),
                "--short".to_string(),
            ],
            params: vec![CommandParamSpec {
                name: "workspace_id".to_string(),
                required: true,
                param_type: CommandParamType::String,
            }],
            description: "List git status (short format) for a workspace.".to_string(),
        },
    );

    // run_tests: cargo test in workspace (or npm test later)
    commands.insert(
        "run_tests".to_string(),
        CommandSpec {
            id: "run_tests".to_string(),
            binary: PathBuf::from("cargo"),
            args_template: vec!["test".to_string()],
            params: vec![CommandParamSpec {
                name: "workspace_id".to_string(),
                required: true,
                param_type: CommandParamType::String,
            }],
            description: "Run tests (cargo test) in the given workspace.".to_string(),
        },
    );

    // format_code: cargo fmt in workspace
    commands.insert(
        "format_code".to_string(),
        CommandSpec {
            id: "format_code".to_string(),
            binary: PathBuf::from("cargo"),
            args_template: vec!["fmt".to_string()],
            params: vec![CommandParamSpec {
                name: "workspace_id".to_string(),
                required: true,
                param_type: CommandParamType::String,
            }],
            description: "Format code (cargo fmt) in the given workspace.".to_string(),
        },
    );

    CommandCatalog { commands }
}

// -----------------------------------------------------------------------------
// FS error mapping
// -----------------------------------------------------------------------------

fn shell_error_from_fs(e: FsError) -> ShellError {
    use crate::fs::FsErrorKind as FsKind;
    let (kind, message) = match e.kind {
        FsKind::InvalidPath | FsKind::NotDirectory | FsKind::TooLarge | FsKind::WorkspaceNotFound => {
            (ShellErrorKind::InvalidParams, e.message)
        }
        FsKind::Forbidden => (ShellErrorKind::Forbidden, e.message),
        FsKind::Io => (ShellErrorKind::Io, e.message),
    };
    ShellError { kind, message }
}

// -----------------------------------------------------------------------------
// Token-based template expansion
// -----------------------------------------------------------------------------

/// Expands `args_template` by replacing only exact `{{name}}` tokens with
/// values from `replacements`. Output is argv tokens only; no shell fragments
/// or concatenation. Unknown placeholders are left unchanged.
pub fn expand_template(
    args_template: &[String],
    replacements: &HashMap<String, String>,
) -> Vec<String> {
    args_template
        .iter()
        .map(|token| {
            if token.len() >= 4 && token.starts_with("{{") && token.ends_with("}}") {
                let key = &token[2..token.len() - 2];
                replacements
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| token.clone())
            } else {
                token.clone()
            }
        })
        .collect()
}

// -----------------------------------------------------------------------------
// Parameter validation and binding → BoundCommand
// -----------------------------------------------------------------------------

/// Validates params against the command spec and resolves workspace-scoped
/// paths via FS `normalize_and_resolve`. Returns a fully bound command
/// (binary, argv, cwd) with no further string interpolation.
pub fn validate_and_bind_params(
    spec: &CommandSpec,
    params: &serde_json::Value,
    ctx: &ShellContext,
) -> Result<BoundCommand, ShellError> {
    let obj = params.as_object().ok_or_else(|| ShellError {
        kind: ShellErrorKind::InvalidParams,
        message: "params must be a JSON object".to_string(),
    })?;

    let fs_ctx = AppFsContext {
        app_config_dir: PathBuf::from("."),
        workspace_roots: ctx.workspace_roots.clone(),
    };

    let mut replacements = HashMap::new();

    for param_spec in &spec.params {
        let value = obj.get(&param_spec.name);
        let value = if param_spec.required {
            value.ok_or_else(|| ShellError {
                kind: ShellErrorKind::InvalidParams,
                message: format!("missing required param: {}", param_spec.name),
            })?
        } else {
            match value {
                Some(v) => v,
                None => continue,
            }
        };

        let s = match &param_spec.param_type {
            CommandParamType::String => value.as_str().ok_or_else(|| ShellError {
                kind: ShellErrorKind::InvalidParams,
                message: format!("param {} must be a string", param_spec.name),
            })?.to_string(),
            CommandParamType::Enum(allowed) => {
                let s = value.as_str().ok_or_else(|| ShellError {
                    kind: ShellErrorKind::InvalidParams,
                    message: format!("param {} must be a string", param_spec.name),
                })?;
                if !allowed.contains(&s.to_string()) {
                    return Err(ShellError {
                        kind: ShellErrorKind::InvalidParams,
                        message: format!(
                            "param {} must be one of: {:?}",
                            param_spec.name, allowed
                        ),
                    });
                }
                s.to_string()
            }
            CommandParamType::PathRelativeToWorkspace => {
                let rel = value.as_str().ok_or_else(|| ShellError {
                    kind: ShellErrorKind::InvalidParams,
                    message: format!("param {} must be a string", param_spec.name),
                })?;
                let workspace_id = obj
                    .get("workspace_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&ctx.default_workspace_id);
                let base = BaseDirKind::Workspace { id: workspace_id.to_string() };
                let normalized = normalize_and_resolve(&fs_ctx, &base, rel).map_err(shell_error_from_fs)?;
                normalized.abs_path.display().to_string()
            }
            CommandParamType::Bool => {
                value.as_bool().ok_or_else(|| ShellError {
                    kind: ShellErrorKind::InvalidParams,
                    message: format!("param {} must be a boolean", param_spec.name),
                })?;
                value.to_string()
            }
            CommandParamType::Integer { min, max } => {
                let n = value.as_i64().ok_or_else(|| ShellError {
                    kind: ShellErrorKind::InvalidParams,
                    message: format!("param {} must be an integer", param_spec.name),
                })?;
                if let Some(m) = min {
                    if n < *m {
                        return Err(ShellError {
                            kind: ShellErrorKind::InvalidParams,
                            message: format!("param {} must be >= {}", param_spec.name, m),
                        });
                    }
                }
                if let Some(m) = max {
                    if n > *m {
                        return Err(ShellError {
                            kind: ShellErrorKind::InvalidParams,
                            message: format!("param {} must be <= {}", param_spec.name, m),
                        });
                    }
                }
                n.to_string()
            }
        };

        replacements.insert(param_spec.name.clone(), s);
    }

    // Resolve workspace_id → workspace path for {{workspace}} placeholder
    if spec.args_template.iter().any(|t| t == "{{workspace}}") {
        let wid = replacements.get("workspace_id").ok_or_else(|| ShellError {
            kind: ShellErrorKind::InvalidParams,
            message: "workspace_id required for this command".to_string(),
        })?;
        let root = ctx.workspace_roots.iter().find(|r| &r.id == wid).ok_or_else(|| ShellError {
            kind: ShellErrorKind::InvalidParams,
            message: format!("workspace not found: {}", wid),
        })?;
        replacements.insert("workspace".to_string(), root.path.display().to_string());
    }

    let args = expand_template(&spec.args_template, &replacements);
    let cwd = replacements
        .get("workspace")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    Ok(BoundCommand {
        binary: spec.binary.clone(),
        args,
        cwd,
    })
}

// -----------------------------------------------------------------------------
// Catalog lookup
// -----------------------------------------------------------------------------

/// Max bytes of combined stdout + stderr to capture (hard cap).
const MAX_OUTPUT_BYTES: usize = 512 * 1024; // 512 KiB

/// Default execution timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

// -----------------------------------------------------------------------------
// Command-level security policy (deny patterns)
// -----------------------------------------------------------------------------

/// Hard-deny tokens: binary names or argument tokens that must not appear.
const DENY_TOKENS: &[&str] = &[
    "rm", "sudo", "su",
    "apt", "apt-get", "brew", "yum", "dnf", "pip", "pip3", "npm", "pnpm", "yarn",
    "nmap", "masscan", "nc", "netcat",
];

fn is_denied_token(token: &str) -> bool {
    let lower = token.to_lowercase();
    DENY_TOKENS.iter().any(|&d| d == lower)
}

/// Evaluates command-level policy: scans `bound.binary` (file stem) and every
/// `bound.args` token against the hard-deny list.
pub fn evaluate_command_policy(
    _spec: &CommandSpec,
    bound: &BoundCommand,
) -> SecurityDecision {
    let binary_stem = bound
        .binary
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if is_denied_token(binary_stem) {
        return SecurityDecision {
            decision: SecurityDecisionKind::Deny,
            reason: Some(DenyReason::BlocklistedCommand),
        };
    }
    for arg in &bound.args {
        if is_denied_token(arg) {
            return SecurityDecision {
                decision: SecurityDecisionKind::Deny,
                reason: Some(DenyReason::BlocklistedCommand),
            };
        }
    }
    SecurityDecision {
        decision: SecurityDecisionKind::Allow,
        reason: None,
    }
}

impl CommandCatalog {
    /// O(1) lookup by command id.
    pub fn get(&self, command_id: &str) -> Option<&CommandSpec> {
        self.commands.get(command_id)
    }

    /// Iterate over all commands (id, spec). Order is unspecified.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &CommandSpec)> {
        self.commands.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Merges plugin command specs into the catalog (Sprint 9).
    /// Plugin ids override built-in commands if they collide.
    pub fn merge_plugin_commands(
        &mut self,
        specs: impl IntoIterator<Item = (String, CommandSpec)>,
    ) {
        for (id, spec) in specs {
            self.commands.insert(id, spec);
        }
    }
}

fn allow_decision() -> SecurityDecision {
    SecurityDecision {
        decision: SecurityDecisionKind::Allow,
        reason: None,
    }
}

/// Core execution: lookup spec, validate/bind params, run security policy, spawn with
/// minimal env + timeout + output cap, capture stdout/stderr and exit code.
pub fn shell_run(
    caller: ShellCaller,
    ctx: &ShellContext,
    catalog: &CommandCatalog,
    command_id: &str,
    params: serde_json::Value,
) -> Result<ShellResult, ShellError> {
    let spec = match catalog.get(command_id) {
        Some(s) => s,
        None => {
            let err = ShellError {
                kind: ShellErrorKind::UnknownCommand,
                message: format!("unknown command: {}", command_id),
            };
            log_shell_event(caller, command_id, None, &allow_decision(), None, Some(&err));
            return Err(err);
        }
    };

    let bound = match validate_and_bind_params(spec, &params, ctx) {
        Ok(b) => b,
        Err(err) => {
            log_shell_event(caller, command_id, None, &allow_decision(), None, Some(&err));
            return Err(err);
        }
    };

    let decision = evaluate_command_policy(spec, &bound);
    if matches!(decision.decision, SecurityDecisionKind::Deny) {
        let message = decision
            .reason
            .as_ref()
            .map(|r| format!("command denied by policy: {:?}", r))
            .unwrap_or_else(|| "command denied by policy".to_string());
        let err = ShellError {
            kind: ShellErrorKind::Forbidden,
            message,
        };
        log_shell_event(caller, command_id, Some(&bound), &decision, None, Some(&err));
        return Err(err);
    }

    let run_start = Instant::now();
    let safe_env = env_filter::safe_env_for_child(
        &restricted_path(),
        "en_US.UTF-8",
        "dumb",
    );
    if env_filter::env_has_high_risk_keys() {
        if let Ok(json) = serde_json::to_string(&serde_json::json!({
            "event": "shell_scrubbed_env",
            "message": "Child process started with filtered env (high-risk keys dropped)",
        })) {
            println!("{json}");
        }
    }
    let mut cmd = std::process::Command::new(&bound.binary);
    cmd.args(&bound.args)
        .current_dir(&bound.cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd.env_clear();
    for (k, v) in safe_env {
        cmd.env(k, v);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let err = ShellError {
                kind: ShellErrorKind::Io,
                message: format!("failed to spawn {}: {}", bound.binary.display(), e),
            };
            log_shell_event(caller, command_id, Some(&bound), &decision, None, Some(&err));
            return Err(err);
        }
    };

    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let err = ShellError {
                    kind: ShellErrorKind::Io,
                    message: format!("wait failed: {}", e),
                };
                log_shell_event(caller, command_id, Some(&bound), &decision, None, Some(&err));
                return Err(err);
            }
        }
        if run_start.elapsed() >= DEFAULT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let err = ShellError {
                kind: ShellErrorKind::Timeout,
                message: format!("command timed out after {:?}", DEFAULT_TIMEOUT),
            };
            log_shell_event(caller, command_id, Some(&bound), &decision, None, Some(&err));
            return Err(err);
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let err = ShellError {
                kind: ShellErrorKind::Io,
                message: "stdout not captured".to_string(),
            };
            log_shell_event(caller, command_id, Some(&bound), &decision, None, Some(&err));
            return Err(err);
        }
    };
    let mut stderr = match child.stderr.take() {
        Some(s) => s,
        None => {
            let err = ShellError {
                kind: ShellErrorKind::Io,
                message: "stderr not captured".to_string(),
            };
            log_shell_event(caller, command_id, Some(&bound), &decision, None, Some(&err));
            return Err(err);
        }
    };

    let mut out_buf = Vec::with_capacity(MAX_OUTPUT_BYTES.min(8192));
    let mut err_buf = Vec::with_capacity(MAX_OUTPUT_BYTES.min(8192));
    let mut out_len = 0usize;
    let mut err_len = 0usize;
    let mut buf = [0u8; 4096];
    loop {
        let n = stdout.read(&mut buf).map_err(|e| ShellError {
            kind: ShellErrorKind::Io,
            message: e.to_string(),
        })?;
        if n == 0 {
            break;
        }
        if out_len + n <= MAX_OUTPUT_BYTES {
            out_buf.extend_from_slice(&buf[..n]);
            out_len += n;
        }
    }
    loop {
        let n = stderr.read(&mut buf).map_err(|e| ShellError {
            kind: ShellErrorKind::Io,
            message: e.to_string(),
        })?;
        if n == 0 {
            break;
        }
        if err_len + n <= MAX_OUTPUT_BYTES {
            err_buf.extend_from_slice(&buf[..n]);
            err_len += n;
        }
    }

    let duration_ms = run_start.elapsed().as_millis() as u64;
    let stdout_str = String::from_utf8_lossy(&out_buf).into_owned();
    let stderr_str = String::from_utf8_lossy(&err_buf).into_owned();
    let result = ShellResult {
        stdout: stdout_str,
        stderr: stderr_str,
        exit_code,
        duration_ms,
    };
    log_shell_event(caller, command_id, Some(&bound), &decision, Some(&result), None);
    Ok(result)
}

fn restricted_path() -> String {
    #[cfg(target_os = "macos")]
    {
        "/usr/bin:/bin:/usr/sbin:/sbin".to_string()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "/usr/bin:/bin".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs as unix_fs;
    use tempfile::tempdir;

    fn catalog_with_echo() -> CommandCatalog {
        let mut commands = HashMap::new();
        commands.insert(
            "echo_test".to_string(),
            CommandSpec {
                id: "echo_test".to_string(),
                binary: PathBuf::from("echo"),
                args_template: vec!["hello".to_string()],
                params: vec![],
                description: "Echo for tests.".to_string(),
            },
        );
        CommandCatalog { commands }
    }

    fn minimal_shell_context(workspace_path: &std::path::Path) -> ShellContext {
        ShellContext {
            workspace_roots: vec![WorkspaceRoot {
                id: "default".to_string(),
                name: "default".to_string(),
                path: workspace_path.to_path_buf(),
            }],
            default_workspace_id: "default".to_string(),
        }
    }

    #[test]
    fn unknown_command_is_rejected() {
        let dir = tempdir().unwrap();
        let ctx = minimal_shell_context(dir.path());
        let catalog = default_catalog();
        let params = serde_json::json!({ "workspace_id": "default" });
        let err = shell_run(ShellCaller::Ui, &ctx, &catalog, "nonexistent_command", params).unwrap_err();
        assert!(matches!(err.kind, ShellErrorKind::UnknownCommand));
    }

    #[test]
    fn invalid_params_are_rejected() {
        let dir = tempdir().unwrap();
        let ctx = minimal_shell_context(dir.path());
        let catalog = default_catalog();
        let err = shell_run(ShellCaller::Ui, &ctx, &catalog, "list_git_status", serde_json::json!({})).unwrap_err();
        assert!(matches!(err.kind, ShellErrorKind::InvalidParams));
        let err = shell_run(ShellCaller::Ui, &ctx, &catalog, "list_git_status", serde_json::json!({ "workspace_id": 123 })).unwrap_err();
        assert!(matches!(err.kind, ShellErrorKind::InvalidParams));
        let err = shell_run(ShellCaller::Ui, &ctx, &catalog, "list_git_status", serde_json::json!("not an object")).unwrap_err();
        assert!(matches!(err.kind, ShellErrorKind::InvalidParams));
    }

    #[test]
    fn workspace_paths_are_normalized_and_scoped() {
        let dir = tempdir().unwrap();
        let ctx = minimal_shell_context(dir.path());
        let spec_with_path = CommandSpec {
            id: "path_cmd".to_string(),
            binary: PathBuf::from("true"),
            args_template: vec!["{{path}}".to_string()],
            params: vec![
                CommandParamSpec { name: "workspace_id".to_string(), required: true, param_type: CommandParamType::String },
                CommandParamSpec { name: "path".to_string(), required: true, param_type: CommandParamType::PathRelativeToWorkspace },
            ],
            description: "Test path resolution.".to_string(),
        };
        let mut commands = HashMap::new();
        commands.insert("path_cmd".to_string(), spec_with_path);
        let catalog_path = CommandCatalog { commands };
        let params = serde_json::json!({ "workspace_id": "default", "path": "../escape" });
        let err = validate_and_bind_params(catalog_path.get("path_cmd").unwrap(), &params, &ctx).unwrap_err();
        assert!(matches!(err.kind, ShellErrorKind::InvalidParams));
        #[cfg(unix)]
        {
            let outside = tempdir().unwrap();
            let secret = outside.path().join("secret");
            fs::write(&secret, b"data").unwrap();
            let link_in_ws = dir.path().join("link_to_outside");
            unix_fs::symlink(&secret, &link_in_ws).unwrap();
            let params_symlink = serde_json::json!({ "workspace_id": "default", "path": "link_to_outside" });
            let err = validate_and_bind_params(catalog_path.get("path_cmd").unwrap(), &params_symlink, &ctx).unwrap_err();
            assert!(matches!(err.kind, ShellErrorKind::Forbidden) || matches!(err.kind, ShellErrorKind::InvalidParams));
        }
    }

    #[test]
    fn hard_deny_commands_are_blocked() {
        let spec = CommandSpec {
            id: "dummy".to_string(),
            binary: PathBuf::from("git"),
            args_template: vec![],
            params: vec![],
            description: "".to_string(),
        };
        let bound_rm = BoundCommand { binary: PathBuf::from("rm"), args: vec!["-rf".to_string(), "/".to_string()], cwd: PathBuf::from("/tmp") };
        let decision = evaluate_command_policy(&spec, &bound_rm);
        assert!(matches!(decision.decision, SecurityDecisionKind::Deny));
        assert!(matches!(decision.reason, Some(DenyReason::BlocklistedCommand)));
        let bound_sudo = BoundCommand { binary: PathBuf::from("sudo"), args: vec!["ls".to_string()], cwd: PathBuf::from("/tmp") };
        let decision = evaluate_command_policy(&spec, &bound_sudo);
        assert!(matches!(decision.decision, SecurityDecisionKind::Deny));
        assert!(matches!(decision.reason, Some(DenyReason::BlocklistedCommand)));
    }

    #[test]
    fn stdout_stderr_exit_code_are_captured() {
        let dir = tempdir().unwrap();
        let ctx = minimal_shell_context(dir.path());
        let catalog = catalog_with_echo();
        let result = shell_run(ShellCaller::Ui, &ctx, &catalog, "echo_test", serde_json::json!({})).unwrap();
        assert!(result.stdout.trim() == "hello" || result.stdout.contains("hello"));
        assert_eq!(result.exit_code, 0);
        assert!(!result.stderr.contains("error") || result.stderr.is_empty());
    }
}
