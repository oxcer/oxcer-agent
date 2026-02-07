use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use dirs_next::home_dir;
use mime_guess::MimeGuess;
use serde::Serialize;

/// Upper bound for file reads in bytes. This is a hard cap to avoid
/// accidentally loading huge files into memory.
pub const MAX_READ_BYTES: u64 = 8 * 1024 * 1024; // 8 MiB

/// Logical caller classification for FS logging / policy.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FsCaller {
    Ui,
    Agent,
    ShellTool,
}

/// Known base directories we resolve logical paths against.
#[derive(Clone, Debug)]
pub enum BaseDirKind {
    /// Application configuration directory (e.g. `$APPCONFIG`).
    AppConfig,
    /// User-configured workspace root. Identified by a stable id.
    Workspace { id: String },
}

/// Minimal config structure for user-registered workspace roots.
///
/// This will eventually be populated from app configuration and surfaced
/// in a Settings UI, but for Sprint 2 we just need the type and safe
/// handling in the backend.
#[derive(Clone, Debug)]
pub struct WorkspaceRoot {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

/// Context for resolving logical FS operations.
#[derive(Clone, Debug)]
pub struct AppFsContext {
    pub app_config_dir: PathBuf,
    pub workspace_roots: Vec<WorkspaceRoot>,
}

/// Canonical, normalized path information scoped to a known base directory.
#[derive(Clone, Debug)]
pub struct NormalizedPath {
    pub base: BaseDirKind,
    pub abs_path: PathBuf,
    pub rel_to_base: PathBuf,
}

/// Lightweight directory entry metadata returned by `fs_list_dir`.
#[derive(Clone, Debug, Serialize)]
pub struct DirEntryMetadata {
    pub name: String,
    pub is_file: bool,
    pub is_dir: bool,
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<SystemTime>,
}

/// Result type for `fs_read_file`, distinguishing text vs binary content.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FsReadResult {
    Text {
        contents: String,
        size_bytes: u64,
    },
    Binary {
        size_bytes: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_guess: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityDecisionKind {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyReason {
    BlocklistedPath,
    OutsideAllowedBase,
    EscapeAttempt,
    /// Command or argument token matched a hard-deny list (e.g. rm, sudo, nmap).
    BlocklistedCommand,
}

#[derive(Clone, Debug, Serialize)]
pub struct SecurityDecision {
    pub decision: SecurityDecisionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<DenyReason>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FsErrorKind {
    InvalidPath,
    WorkspaceNotFound,
    NotDirectory,
    Forbidden,
    Io,
    TooLarge,
}

/// Structured error for FS operations. This is intentionally simple and
/// JSON-friendly so it can be surfaced to the frontend safely.
#[derive(Debug, Serialize)]
pub struct FsError {
    pub kind: FsErrorKind,
    pub message: String,
}

impl FsError {
    fn invalid_path(msg: impl Into<String>) -> Self {
        FsError {
            kind: FsErrorKind::InvalidPath,
            message: msg.into(),
        }
    }

    fn workspace_not_found(id: &str) -> Self {
        FsError {
            kind: FsErrorKind::WorkspaceNotFound,
            message: format!("Workspace root not found: {id}"),
        }
    }

    fn not_directory(path: &Path) -> Self {
        FsError {
            kind: FsErrorKind::NotDirectory,
            message: format!("Path is not a directory: {}", path.display()),
        }
    }

    fn forbidden(msg: impl Into<String>) -> Self {
        FsError {
            kind: FsErrorKind::Forbidden,
            message: msg.into(),
        }
    }

    fn io(err: std::io::Error) -> Self {
        FsError {
            kind: FsErrorKind::Io,
            message: err.to_string(),
        }
    }

    fn too_large(size: u64) -> Self {
        FsError {
            kind: FsErrorKind::TooLarge,
            message: format!(
                "File is too large to read ({} bytes, max {} bytes)",
                size, MAX_READ_BYTES
            ),
        }
    }
}

/// Log entry for filesystem operations. This is JSON-friendly and avoids
/// including raw file contents.
#[derive(Debug, Serialize)]
pub struct FsLogEntry<'a> {
    pub timestamp: String,
    pub caller: FsCaller,
    pub operation: &'a str,
    pub path_normalized: String,
    pub policy_decision: SecurityDecisionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<FsErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

fn now_iso8601() -> String {
    // Keep it simple and dependency-free: SystemTime -> RFC3339-like.
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

fn log_fs_event<'a>(
    caller: FsCaller,
    operation: &'a str,
    normalized: &NormalizedPath,
    decision: &SecurityDecision,
    size_bytes: Option<u64>,
    error: Option<&FsError>,
) {
    let entry = FsLogEntry {
        timestamp: now_iso8601(),
        caller,
        operation,
        path_normalized: normalized.abs_path.display().to_string(),
        policy_decision: decision.decision.clone(),
        size_bytes,
        error_code: error.map(|e| e.kind.clone()),
        error_message: error.map(|e| e.message.clone()),
    };

    // For Sprint 2 we log to stdout. This can later be redirected to a
    // rotating file under the app config dir in release builds.
    if let Ok(json) = serde_json::to_string(&entry) {
        println!("{json}");
    }
}

/// Normalize and resolve a logical path against a known base directory.
///
/// This rejects:
/// - Any attempt to escape the base via `..` segments.
/// - Symlinks that resolve outside the selected base directory.
pub fn normalize_and_resolve(
    ctx: &AppFsContext,
    base: BaseDirKind,
    rel_path: &str,
) -> Result<NormalizedPath, FsError> {
    if rel_path.is_empty() {
        return Err(FsError::invalid_path("Path must not be empty"));
    }

    let rel = PathBuf::from(rel_path);
    if rel.is_absolute() {
        return Err(FsError::invalid_path(
            "Absolute paths are not allowed; use a path relative to the selected base directory",
        ));
    }

    // Reject explicit parent directory components up front to make intent clear,
    // even though canonicalization below also protects us.
    for comp in rel.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(FsError::invalid_path(
                "Path traversal using `..` is not allowed",
            ));
        }
    }

    let (base_dir, base_clone) = match &base {
        BaseDirKind::AppConfig => (&ctx.app_config_dir, BaseDirKind::AppConfig),
        BaseDirKind::Workspace { id } => {
            let root = ctx
                .workspace_roots
                .iter()
                .find(|w| &w.id == id)
                .ok_or_else(|| FsError::workspace_not_found(id))?;
            (&root.path, BaseDirKind::Workspace { id: id.clone() })
        }
    };

    let candidate = base_dir.join(&rel);

    let canonical = candidate
        .canonicalize()
        .map_err(FsError::io)?;

    // Ensure that the resolved path is still inside the chosen base directory,
    // even in the presence of symlinks.
    if !canonical.starts_with(base_dir) {
        return Err(FsError::forbidden(
            "Resolved path escapes the allowed base directory (possible symlink traversal)",
        ));
    }

    let rel_to_base = canonical
        .strip_prefix(base_dir)
        .unwrap_or(&canonical)
        .to_path_buf();

    Ok(NormalizedPath {
        base: base_clone,
        abs_path: canonical,
        rel_to_base,
    })
}

fn evaluate_path_policy(normalized: &NormalizedPath) -> SecurityDecision {
    let abs = &normalized.abs_path;

    // Hard blocklist for common secret locations on macOS and Unix-like systems.
    if let Some(home) = home_dir() {
        let ssh = home.join(".ssh");
        let aws = home.join(".aws");
        let keychains = home.join("Library/Keychains");
        let passwords = home.join("Library/Passwords");

        if abs.starts_with(&ssh)
            || abs.starts_with(&aws)
            || abs.starts_with(&keychains)
            || abs.starts_with(&passwords)
        {
            return SecurityDecision {
                decision: SecurityDecisionKind::Deny,
                reason: Some(DenyReason::BlocklistedPath),
            };
        }
    }

    // At this stage all NormalizedPath values are already constrained to
    // either a workspace root or the app config directory, so anything that
    // reaches here is a candidate allow.
    SecurityDecision {
        decision: SecurityDecisionKind::Allow,
        reason: None,
    }
}

/// List a single directory level under a logical path.
pub fn fs_list_dir(
    caller: FsCaller,
    ctx: &AppFsContext,
    base: BaseDirKind,
    rel_path: &str,
) -> Result<Vec<DirEntryMetadata>, FsError> {
    let normalized = normalize_and_resolve(ctx, base, rel_path)?;
    let decision = evaluate_path_policy(&normalized);

    if matches!(decision.decision, SecurityDecisionKind::Deny) {
        let err = FsError::forbidden("Access to this directory is forbidden by policy");
        log_fs_event(caller, "list", &normalized, &decision, None, Some(&err));
        return Err(err);
    }

    let meta = fs::metadata(&normalized.abs_path).map_err(FsError::io)?;
    if !meta.is_dir() {
        let err = FsError::not_directory(&normalized.abs_path);
        log_fs_event(caller, "list", &normalized, &decision, None, Some(&err));
        return Err(err);
    }

    let mut entries = Vec::new();
    let read_dir = fs::read_dir(&normalized.abs_path).map_err(FsError::io)?;

    for entry_res in read_dir {
        let entry = match entry_res {
            Ok(e) => e,
            Err(e) => {
                // Skip entries we cannot read; directory listing is best-effort.
                eprintln!("fs_list_dir: failed to read dir entry: {e}");
                continue;
            }
        };

        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        let size = if meta.is_file() { Some(meta.len()) } else { None };
        let modified = meta.modified().ok();

        entries.push(DirEntryMetadata {
            name,
            is_file: file_type.is_file(),
            is_dir: file_type.is_dir(),
            size_bytes: size,
            modified,
        });
    }

    log_fs_event(
        caller,
        "list",
        &normalized,
        &decision,
        None,
        None,
    );

    Ok(entries)
}

/// Read a file under a logical path with size and binary safeguards.
pub fn fs_read_file(
    caller: FsCaller,
    ctx: &AppFsContext,
    base: BaseDirKind,
    rel_path: &str,
) -> Result<FsReadResult, FsError> {
    let normalized = normalize_and_resolve(ctx, base, rel_path)?;
    let decision = evaluate_path_policy(&normalized);

    if matches!(decision.decision, SecurityDecisionKind::Deny) {
        let err = FsError::forbidden("Access to this file is forbidden by policy");
        log_fs_event(caller, "read", &normalized, &decision, None, Some(&err));
        return Err(err);
    }

    let meta = fs::metadata(&normalized.abs_path).map_err(FsError::io)?;
    if !meta.is_file() {
        let err = FsError::invalid_path("Path is not a regular file");
        log_fs_event(caller, "read", &normalized, &decision, None, Some(&err));
        return Err(err);
    }

    let size = meta.len();
    if size > MAX_READ_BYTES {
        let err = FsError::too_large(size);
        log_fs_event(
            caller,
            "read",
            &normalized,
            &decision,
            Some(size),
            Some(&err),
        );
        return Err(err);
    }

    let mut file = fs::File::open(&normalized.abs_path).map_err(FsError::io)?;
    let mut buf = Vec::with_capacity(size as usize);
    file.read_to_end(&mut buf).map_err(FsError::io)?;

    let is_binary = buf.contains(&0u8) || std::str::from_utf8(&buf).is_err();

    let result = if is_binary {
        let guess = MimeGuess::from_path(&normalized.abs_path)
            .first_raw()
            .map(|s| s.to_string());
        FsReadResult::Binary {
            size_bytes: size,
            mime_guess: guess,
        }
    } else {
        let contents = String::from_utf8(buf).map_err(|e| FsError::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.utf8_error(),
        )))?;
        FsReadResult::Text {
            contents,
            size_bytes: size,
        }
    };

    log_fs_event(
        caller,
        "read",
        &normalized,
        &decision,
        Some(size),
        None,
    );

    Ok(result)
}

/// Write a file under a logical path. Only workspace roots are allowed as
/// targets for write operations in Sprint 2.
pub fn fs_write_file(
    caller: FsCaller,
    ctx: &AppFsContext,
    base: BaseDirKind,
    rel_path: &str,
    contents: &[u8],
) -> Result<(), FsError> {
    // For Sprint 2 we only allow writes under workspace roots, not app
    // configuration or arbitrary other bases.
    if matches!(base, BaseDirKind::AppConfig) {
        return Err(FsError::forbidden(
            "Writes to the app config directory are disabled in this version",
        ));
    }

    let normalized = normalize_and_resolve(ctx, base, rel_path)?;
    let decision = evaluate_path_policy(&normalized);

    if matches!(decision.decision, SecurityDecisionKind::Deny) {
        let err = FsError::forbidden("Write to this path is forbidden by policy");
        log_fs_event(
            caller,
            "write",
            &normalized,
            &decision,
            Some(contents.len() as u64),
            Some(&err),
        );
        return Err(err);
    }

    if let Some(parent) = normalized.abs_path.parent() {
        // This cannot create directories above the workspace root because
        // `normalized` has already been checked to sit inside the workspace.
        fs::create_dir_all(parent).map_err(FsError::io)?;
    }

    let mut file = fs::File::create(&normalized.abs_path).map_err(FsError::io)?;
    file.write_all(contents).map_err(FsError::io)?;

    log_fs_event(
        caller,
        "write",
        &normalized,
        &decision,
        Some(contents.len() as u64),
        None,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs as stdfs;
    use std::os::unix::fs as unix_fs;
    use tempfile::tempdir;

    fn make_workspace_ctx(root: &Path) -> AppFsContext {
        AppFsContext {
            app_config_dir: root.join("appconfig"),
            workspace_roots: vec![WorkspaceRoot {
                id: "ws".to_string(),
                name: "ws".to_string(),
                path: root.to_path_buf(),
            }],
        }
    }

    #[test]
    fn inside_workspace_allows_read() {
        let dir = tempdir().unwrap();
        // Use canonical path so normalize_and_resolve's canonicalize() stays under base_dir
        // (on macOS, temp dirs under /var are symlinks to /private/var).
        let root = stdfs::canonicalize(dir.path()).unwrap();
        let ctx = make_workspace_ctx(&root);

        let file_path = root.join("file.txt");
        stdfs::write(&file_path, b"hello").unwrap();

        let res = fs_read_file(
            FsCaller::Ui,
            &ctx,
            BaseDirKind::Workspace { id: "ws".to_string() },
            "file.txt",
        )
        .unwrap();

        match res {
            FsReadResult::Text { contents, .. } => assert_eq!(contents, "hello"),
            _ => panic!("expected text result"),
        }
    }

    #[test]
    fn parent_dir_segments_are_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let ctx = make_workspace_ctx(root);

        let err = normalize_and_resolve(
            &ctx,
            BaseDirKind::Workspace { id: "ws".to_string() },
            "../outside.txt",
        )
        .unwrap_err();

        assert_eq!(matches!(err.kind, FsErrorKind::InvalidPath), true);
    }

    #[test]
    fn symlink_escape_is_denied() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();

        let ws_root = dir.path();
        let ctx = make_workspace_ctx(ws_root);

        let target = outside.path().join("secret.txt");
        stdfs::write(&target, b"secret").unwrap();

        let link = ws_root.join("link.txt");
        unix_fs::symlink(&target, &link).unwrap();

        let err = fs_read_file(
            FsCaller::Ui,
            &ctx,
            BaseDirKind::Workspace { id: "ws".to_string() },
            "link.txt",
        )
        .unwrap_err();

        assert_eq!(matches!(err.kind, FsErrorKind::Forbidden), true);
    }

    #[test]
    fn blocklisted_paths_are_denied_by_policy() {
        if let Some(home) = home_dir() {
            let np = NormalizedPath {
                base: BaseDirKind::AppConfig,
                abs_path: home.join(".ssh/id_rsa"),
                rel_to_base: PathBuf::from(".ssh/id_rsa"),
            };

            let decision = evaluate_path_policy(&np);
            assert!(matches!(decision.decision, SecurityDecisionKind::Deny));
        }
    }

    #[test]
    fn large_file_read_is_blocked() {
        let dir = tempdir().unwrap();
        let root = stdfs::canonicalize(dir.path()).unwrap();
        let ctx = make_workspace_ctx(&root);

        let file_path = root.join("big.bin");
        let size = MAX_READ_BYTES + 1;
        let data = vec![b'a'; size as usize];
        stdfs::write(&file_path, &data).unwrap();

        let res = fs_read_file(
            FsCaller::Ui,
            &ctx,
            BaseDirKind::Workspace { id: "ws".to_string() },
            "big.bin",
        );
        assert!(
            matches!(res, Err(FsError { kind: FsErrorKind::TooLarge, .. })),
            "expected TooLarge, got {:?}",
            res
        );
    }
}
